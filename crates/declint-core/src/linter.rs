//! The lint engine: compiled rules in, [`Violation`]s out.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::callback::{
    Callbacks, Decision, MatchCallback, MatchContext, MatchParser, RawMatch,
};
use crate::config::{Config, ConfigError, Matcher, Rule, Scope};
use crate::scopes;
use crate::Severity;

/// Per-document information callbacks can see: the file's path and
/// language id. Empty strings are fine when the caller has neither.
#[derive(Debug, Clone, Copy, Default)]
pub struct DocInfo<'a> {
    /// The file's path, as shown to the user.
    pub path: &'a str,
    /// The document's language id.
    pub language: &'a str,
}

impl<'a> DocInfo<'a> {
    /// No path, no language.
    pub fn none() -> Self {
        Self {
            path: "",
            language: "",
        }
    }
}

/// A byte range in the linted source.
///
/// Unlike increparse's `Span` there is no revision — a violation is a fact
/// about one snapshot of one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    /// Inclusive start byte offset.
    pub start: usize,
    /// Exclusive end byte offset.
    pub end: usize,
}

impl Span {
    /// Creates a span.
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// The span's length in bytes.
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Whether the span covers no bytes.
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// The span as a `Range<usize>` for slicing.
    pub fn to_range(&self) -> std::ops::Range<usize> {
        self.start..self.end
    }
}

impl From<std::ops::Range<usize>> for Span {
    fn from(range: std::ops::Range<usize>) -> Self {
        Self::new(range.start, range.end)
    }
}

/// One rule hit: where, how bad, and the rendered message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The id of the rule that fired.
    pub rule_id: String,
    /// How serious the hit is.
    pub severity: Severity,
    /// The matched byte range.
    pub span: Span,
    /// The rule's message template rendered for this match.
    pub message: String,
}

impl PartialOrd for Violation {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Ordered by position, then span end, then rule id — the deterministic
/// output order used by every lint entry point.
impl Ord for Violation {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.span.start, self.span.end, &self.rule_id, &self.message).cmp(&(
            other.span.start,
            other.span.end,
            &other.rule_id,
            &other.message,
        ))
    }
}

/// A compiled, ready-to-run rule set.
pub struct Linter {
    rules: Vec<Rule>,
    scopes: Vec<Scope>,
    /// Resolved rule callback implementations, keyed by rule id. Rules
    /// without a callback are absent.
    callbacks: HashMap<String, Arc<dyn MatchCallback>>,
    /// Resolved rule parser implementations, keyed by rule id. Rules
    /// with a regex matcher are absent.
    parsers: HashMap<String, Arc<dyn MatchParser>>,
}

impl Linter {
    /// Compiles a config into a linter, resolving every rule's callback
    /// and parser against `callbacks`. Errors when a rule references a
    /// callback or parser that is not registered — patterns themselves
    /// were validated at config-load time.
    pub fn new(config: Config, callbacks: &Callbacks) -> Result<Self, ConfigError> {
        let mut resolved = HashMap::new();
        let mut resolved_parsers = HashMap::new();
        fn place<T>(
            slot: Option<&Arc<T>>,
            resolved: &mut HashMap<String, Arc<T>>,
            rule: &Rule,
            scope: Option<&Scope>,
            reference: &crate::callback::CallbackRef,
        ) -> Result<(), ConfigError>
        where
            T: ?Sized + 'static,
        {
            match slot {
                Some(implementation) => {
                    resolved.insert(rule.id.clone(), Arc::clone(implementation));
                    Ok(())
                }
                None => match scope {
                    Some(scope) => Err(ConfigError::new(format!(
                        "scope '{}' rule '{}' references {} which is not registered",
                        scope.id,
                        rule.id,
                        reference.describe()
                    ))),
                    None => Err(ConfigError::new(format!(
                        "rule '{}' references {} which is not registered",
                        rule.id,
                        reference.describe()
                    ))),
                },
            }
        }
        for rule in &config.rules {
            if let Some(r) = &rule.callback {
                place(callbacks.resolve(r).as_ref(), &mut resolved, rule, None, r)?;
            }
            if let Some(r) = &rule.parser {
                place(
                    callbacks.resolve_parser(r).as_ref(),
                    &mut resolved_parsers,
                    rule,
                    None,
                    r,
                )?;
            }
        }
        for scope in &config.scopes {
            for rule in &scope.rules {
                if let Some(r) = &rule.callback {
                    place(
                        callbacks.resolve(r).as_ref(),
                        &mut resolved,
                        rule,
                        Some(scope),
                        r,
                    )?;
                }
                if let Some(r) = &rule.parser {
                    place(
                        callbacks.resolve_parser(r).as_ref(),
                        &mut resolved_parsers,
                        rule,
                        Some(scope),
                        r,
                    )?;
                }
            }
        }
        Ok(Self {
            rules: config.rules,
            scopes: config.scopes,
            callbacks: resolved,
            parsers: resolved_parsers,
        })
    }

    /// The global rules, in config order.
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// The scopes, in config order.
    pub fn scopes(&self) -> &[Scope] {
        &self.scopes
    }

    /// Lints one snapshot of a source file with the **global rules**
    /// only — scoped rules are ignored. Hits are sorted by position (then
    /// span end, then rule id) so output is deterministic regardless of
    /// rule order. Zero-width matches are skipped.
    pub fn lint(&self, source: &str) -> Vec<Violation> {
        self.lint_in(DocInfo::none(), source)
    }

    /// Like [`Linter::lint`], with file path and language exposed to
    /// callbacks.
    pub fn lint_in(&self, info: DocInfo<'_>, source: &str) -> Vec<Violation> {
        let mut out = Vec::new();
        self.collect_global(info, source, &mut out);
        sort_violations(&mut out);
        out
    }

    /// Lints pre-computed scope regions — the subpasses. `segments` are
    /// `(scope index, region)` pairs, typically from [`scopes::segment_all`]
    /// or a parse tree's scoped nodes.
    pub fn lint_segments(&self, source: &str, segments: &[(usize, Span)]) -> Vec<Violation> {
        self.lint_segments_in(DocInfo::none(), source, segments)
    }

    /// Like [`Linter::lint_segments`], with file path and language
    /// exposed to callbacks.
    pub fn lint_segments_in(
        &self,
        info: DocInfo<'_>,
        source: &str,
        segments: &[(usize, Span)],
    ) -> Vec<Violation> {
        let mut out = Vec::new();
        self.collect_segments(info, source, segments, &mut out);
        sort_violations(&mut out);
        out
    }

    /// Global rules plus scoped rules over `segments`, merged and sorted —
    /// one call for consumers that already have regions (e.g. a parse
    /// tree).
    pub fn lint_merged(&self, source: &str, segments: &[(usize, Span)]) -> Vec<Violation> {
        self.lint_merged_in(DocInfo::none(), source, segments)
    }

    /// Like [`Linter::lint_merged`], with file path and language exposed
    /// to callbacks.
    pub fn lint_merged_in(
        &self,
        info: DocInfo<'_>,
        source: &str,
        segments: &[(usize, Span)],
    ) -> Vec<Violation> {
        let mut out = Vec::new();
        self.collect_global(info, source, &mut out);
        self.collect_segments(info, source, segments, &mut out);
        sort_violations(&mut out);
        out
    }

    /// Segments the source itself, then lints everything — the one-liner
    /// for CLI use.
    pub fn lint_all(&self, source: &str) -> Vec<Violation> {
        self.lint_all_in(DocInfo::none(), source)
    }

    /// Like [`Linter::lint_all`], with file path and language exposed to
    /// callbacks.
    pub fn lint_all_in(&self, info: DocInfo<'_>, source: &str) -> Vec<Violation> {
        let segments = scopes::segment_all(source, &self.scopes);
        self.lint_merged_in(info, source, &segments)
    }

    /// Runs a single rule over a source snapshot — global or scoped
    /// automatically, by where the rule lives. Unknown ids are an error.
    /// Used by `declint test` to exercise one rule's fixtures.
    pub fn lint_rule(
        &self,
        rule_id: &str,
        info: DocInfo<'_>,
        source: &str,
    ) -> Result<Vec<Violation>, String> {
        let matchers = |rule: &Rule| Matchers {
            callback: self.callbacks.get(rule.id.as_str()).map(|a| a.as_ref()),
            parser: self.parsers.get(rule.id.as_str()).map(|a| a.as_ref()),
        };
        if let Some(rule) = self.rules.iter().find(|rule| rule.id == rule_id) {
            let mut out = Vec::new();
            collect_rule(rule, matchers(rule), info, source, source, 0, &mut out);
            return Ok(out);
        }
        for scope in &self.scopes {
            if let Some(rule) = scope.rules.iter().find(|rule| rule.id == rule_id) {
                let mut out = Vec::new();
                for segment in scopes::segment(source, scope) {
                    let region = &source[segment.to_range()];
                    collect_rule(
                        rule,
                        matchers(rule),
                        info,
                        source,
                        region,
                        segment.start,
                        &mut out,
                    );
                }
                return Ok(out);
            }
        }
        Err(format!("unknown rule `{rule_id}`"))
    }

    fn collect_global(&self, info: DocInfo<'_>, source: &str, out: &mut Vec<Violation>) {
        for rule in &self.rules {
            let matchers = Matchers {
                callback: self.callbacks.get(&rule.id).map(|a| a.as_ref()),
                parser: self.parsers.get(&rule.id).map(|a| a.as_ref()),
            };
            collect_rule(rule, matchers, info, source, source, 0, out);
        }
    }

    fn collect_segments(
        &self,
        info: DocInfo<'_>,
        source: &str,
        segments: &[(usize, Span)],
        out: &mut Vec<Violation>,
    ) {
        for &(scope_index, segment) in segments {
            let Some(scope) = self.scopes.get(scope_index) else {
                continue;
            };
            let region = &source[segment.to_range()];
            for rule in &scope.rules {
                let matchers = Matchers {
                    callback: self.callbacks.get(&rule.id).map(|a| a.as_ref()),
                    parser: self.parsers.get(&rule.id).map(|a| a.as_ref()),
                };
                collect_rule(rule, matchers, info, source, region, segment.start, out);
            }
        }
    }
}

/// A rule's resolved implementations for one lint run.
struct Matchers<'a> {
    callback: Option<&'a dyn MatchCallback>,
    parser: Option<&'a dyn MatchParser>,
}

impl fmt::Debug for Linter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Linter")
            .field("rules", &self.rules.len())
            .field("scopes", &self.scopes.len())
            .field("callbacks", &self.callbacks.len())
            .field("parsers", &self.parsers.len())
            .finish()
    }
}

fn collect_rule(
    rule: &Rule,
    matchers: Matchers<'_>,
    info: DocInfo<'_>,
    source: &str,
    text: &str,
    offset: usize,
    out: &mut Vec<Violation>,
) {
    // Find phase: every matcher produces the same relative-match shape.
    let found: Vec<RawMatch> = match (&rule.matcher, matchers.parser) {
        (Matcher::Regex(regex), _) => regex
            .captures_iter(text)
            .filter_map(|caps| {
                let whole = caps.get(0)?;
                if whole.is_empty() {
                    return None;
                }
                let mut raw = RawMatch::new(whole.start(), whole.end());
                let group_names: Vec<Option<&str>> = rule.capture_names();
                for (i, group) in caps.iter().enumerate().skip(1) {
                    let Some(group) = group else { continue };
                    let name = group_names
                        .get(i)
                        .cloned()
                        .flatten()
                        .map_or_else(|| i.to_string(), str::to_string);
                    raw = raw.with_capture(name, group.as_str());
                }
                Some(raw)
            })
            .collect(),
        (Matcher::Parser, Some(parser)) => match parser.find(text, offset) {
            Ok(found) => found,
            Err(e) => {
                out.push(Violation {
                    rule_id: rule.id.clone(),
                    severity: Severity::Error,
                    span: Span::new(offset, offset),
                    message: format!("rule '{}': parser error: {e}", rule.id),
                });
                return;
            }
        },
        (Matcher::Parser, None) => {
            unreachable!("parser rules always resolve to a registered parser")
        }
    };

    // Decide phase: one shared path for both matchers.
    for raw in found {
        if raw.start >= raw.finish {
            continue; // zero-width matches are noise
        }
        let start = offset + raw.start;
        let finish = offset + raw.finish;
        let (severity, message) = match matchers.callback {
            None => {
                let match_text = source.get(start..finish).unwrap_or_default();
                (
                    rule.severity,
                    rule.message
                        .as_ref()
                        .map_or_else(String::new, |t| t.render_with(match_text, &raw.captures)),
                )
            }
            Some(callback) => {
                let (line, col) = crate::line_col(source, start);
                let ctx = MatchContext {
                    path: info.path.to_string(),
                    language: info.language.to_string(),
                    rule_id: rule.id.clone(),
                    start,
                    finish,
                    line,
                    col,
                    match_text: source.get(start..finish).unwrap_or_default().to_string(),
                    captures: raw.captures.clone(),
                };
                match callback.evaluate(&ctx) {
                    Err(e) => (
                        Severity::Error,
                        format!("rule '{}': callback error: {e}", rule.id),
                    ),
                    Ok(Decision::Allow) => continue,
                    Ok(Decision::Violate { severity, message }) => {
                        (severity.unwrap_or(rule.severity), message)
                    }
                    Ok(Decision::ViolateDefault) => match rule.message.as_ref() {
                        Some(template) => (rule.severity, template.render_with(&ctx.match_text, &raw.captures)),
                        None => (
                            Severity::Error,
                            format!(
                                "rule '{}': callback violated without a default message",
                                rule.id
                            ),
                        ),
                    },
                }
            }
        };
        out.push(Violation {
            rule_id: rule.id.clone(),
            severity,
            span: Span::new(start, finish),
            message,
        });
    }
}

fn sort_violations(violations: &mut [Violation]) {
    violations.sort();
}

/// Converts a byte offset to a 1-based `(line, column)` pair for
/// human-readable output. The column counts characters, not bytes, so it
/// matches what most editors show.
pub fn line_col(source: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(source.len());
    while !source.is_char_boundary(offset) {
        offset -= 1;
    }
    let head = &source[..offset];
    let line = head.matches('\n').count() + 1;
    let line_start = head.rfind('\n').map_or(0, |i| i + 1);
    let col = source[line_start..offset].chars().count() + 1;
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    fn linter(yaml: &str) -> Linter {
        let config = Config::from_str(yaml).unwrap();
        Linter::new(config, &Callbacks::new()).unwrap()
    }

    fn basic_yaml(pattern: &str, id: &str) -> String {
        format!("version: 1\nrules:\n  - id: {id}\n    pattern: '{pattern}'\n    message: hit\n")
    }

    #[test]
    fn basic_violation() {
        let linter = linter(&basic_yaml("\\t+", "no-tabs"));
        let v = linter.lint("a\tb");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].rule_id, "no-tabs");
        assert_eq!(v[0].severity, Severity::Warning);
        assert_eq!(v[0].span.to_range(), 1..2);
        assert_eq!(v[0].message, "hit");
    }

    #[test]
    fn violations_sorted_by_position() {
        let yaml = "\
version: 1
rules:
  - id: zzz
    pattern: 'b'
    message: hit
  - id: aaa
    pattern: 'a'
    message: hit
";
        let linter = linter(yaml);
        let v = linter.lint("abab");
        let ids: Vec<&str> = v.iter().map(|x| x.rule_id.as_str()).collect();
        assert_eq!(ids, ["aaa", "zzz", "aaa", "zzz"]);
    }

    #[test]
    fn zero_width_matches_skipped() {
        let linter = linter(&basic_yaml("x*", "empty"));
        assert!(linter.lint("abc").is_empty());
    }

    #[test]
    fn template_renders_named_group() {
        let linter = linter(&basic_yaml("(?<word>\\w+) =", "var").replace("message: hit", "message: \"rename '{word}'\""));
        let v = linter.lint("foo = 1");
        assert_eq!(v[0].message, "rename 'foo'");
    }

    #[test]
    fn line_col_counts() {
        assert_eq!(line_col("abc", 0), (1, 1));
        assert_eq!(line_col("abc\ndef", 5), (2, 2));
        // "héllo": h = byte 0, é = bytes 1..3 (two bytes), l = byte 3.
        assert_eq!(line_col("héllo", 1), (1, 2));
        assert_eq!(line_col("héllo", 3), (1, 3));
        assert_eq!(line_col("héllo", 4), (1, 4));
        let (line, col) = line_col("héllo", 999);
        assert_eq!((line, col), (1, 6));
    }

    #[test]
    fn mid_char_offset_floors() {
        // Byte 1 is the middle of the two-byte 'é'.
        assert_eq!(line_col("éx", 1), (1, 1));
    }

    const SCOPED: &str = "\
version: 1
rules:
  - id: global
    pattern: 'G'
    message: global hit
scopes:
  - id: sh
    start: '^```sh$'
    end: '^```$'
    rules:
      - id: inner
        pattern: 'sudo'
        message: \"no sudo: '{match}'\"
        severity: error
";

    const SCOPED_TEXT: &str = "G\n```sh\nsudo ls\n```\n";

    #[test]
    fn lint_is_global_only() {
        let linter = linter(SCOPED);
        let v = linter.lint(SCOPED_TEXT);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].rule_id, "global");
        assert_eq!(v[0].span.to_range(), 0..1);
    }

    #[test]
    fn segments_shift_spans_to_absolute_offsets() {
        let linter = linter(SCOPED);
        let segments = crate::scopes::segment_all(SCOPED_TEXT, linter.scopes());
        assert_eq!(segments.len(), 1);
        let v = linter.lint_segments(SCOPED_TEXT, &segments);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].rule_id, "inner");
        assert_eq!(v[0].severity, Severity::Error);
        // "sudo" is at absolute bytes 8..12.
        assert_eq!(v[0].span.to_range(), 8..12);
        assert_eq!(v[0].message, "no sudo: 'sudo'");
    }

    #[test]
    fn merged_combines_and_sorts() {
        let linter = linter(SCOPED);
        let segments = crate::scopes::segment_all(SCOPED_TEXT, linter.scopes());
        let v = linter.lint_merged(SCOPED_TEXT, &segments);
        let ids: Vec<&str> = v.iter().map(|x| x.rule_id.as_str()).collect();
        assert_eq!(ids, ["global", "inner"]);
    }

    #[test]
    fn lint_all_segments_and_lints() {
        let linter = linter(SCOPED);
        let v = linter.lint_all(SCOPED_TEXT);
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn unknown_scope_index_is_skipped() {
        let linter = linter(SCOPED);
        let segments = [(99, Span::new(0, SCOPED_TEXT.len()))];
        assert!(linter.lint_segments(SCOPED_TEXT, &segments).is_empty());
    }
}
