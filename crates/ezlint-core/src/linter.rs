//! The lint engine: compiled rules in, [`Violation`]s out.

use crate::config::{Config, Rule, Scope};
use crate::scopes;
use crate::Severity;

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

/// A compiled, ready-to-run rule set.
#[derive(Debug, Clone)]
pub struct Linter {
    rules: Vec<Rule>,
    scopes: Vec<Scope>,
}

impl Linter {
    /// Compiles a config into a linter. Infallible: every rule was
    /// validated at config-load time.
    pub fn new(config: Config) -> Self {
        Self {
            rules: config.rules,
            scopes: config.scopes,
        }
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
        let mut out = Vec::new();
        self.collect_global(source, &mut out);
        sort_violations(&mut out);
        out
    }

    /// Lints pre-computed scope regions — the subpasses. `segments` are
    /// `(scope index, region)` pairs, typically from [`scopes::segment_all`]
    /// or a parse tree's scoped nodes.
    pub fn lint_segments(&self, source: &str, segments: &[(usize, Span)]) -> Vec<Violation> {
        let mut out = Vec::new();
        self.collect_segments(source, segments, &mut out);
        sort_violations(&mut out);
        out
    }

    /// Global rules plus scoped rules over `segments`, merged and sorted —
    /// one call for consumers that already have regions (e.g. a parse
    /// tree).
    pub fn lint_merged(&self, source: &str, segments: &[(usize, Span)]) -> Vec<Violation> {
        let mut out = Vec::new();
        self.collect_global(source, &mut out);
        self.collect_segments(source, segments, &mut out);
        sort_violations(&mut out);
        out
    }

    /// Segments the source itself, then lints everything — the one-liner
    /// for CLI use.
    pub fn lint_all(&self, source: &str) -> Vec<Violation> {
        let segments = scopes::segment_all(source, &self.scopes);
        self.lint_merged(source, &segments)
    }

    fn collect_global(&self, source: &str, out: &mut Vec<Violation>) {
        for rule in &self.rules {
            collect_rule(rule, source, 0, out);
        }
    }

    fn collect_segments(&self, source: &str, segments: &[(usize, Span)], out: &mut Vec<Violation>) {
        for &(scope_index, segment) in segments {
            let Some(scope) = self.scopes.get(scope_index) else {
                continue;
            };
            let region = &source[segment.to_range()];
            for rule in &scope.rules {
                collect_rule(rule, region, segment.start, out);
            }
        }
    }
}

fn collect_rule(rule: &Rule, text: &str, offset: usize, out: &mut Vec<Violation>) {
    for caps in rule.regex.captures_iter(text) {
        let whole = match caps.get(0) {
            Some(m) if !m.is_empty() => m,
            _ => continue,
        };
        out.push(Violation {
            rule_id: rule.id.clone(),
            severity: rule.severity,
            span: Span::new(offset + whole.start(), offset + whole.end()),
            message: rule.message.render(&caps),
        });
    }
}

fn sort_violations(violations: &mut [Violation]) {
    violations.sort_by(|a, b| {
        (a.span.start, a.span.end, &a.rule_id, &a.message).cmp(&(
            b.span.start,
            b.span.end,
            &b.rule_id,
            &b.message,
        ))
    });
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
        Linter::new(Config::from_str(yaml).unwrap())
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
