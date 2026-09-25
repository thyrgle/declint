//! The lint engine: compiled rules in, [`Violation`]s out.

use crate::config::{Config, Rule};
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
}

impl Linter {
    /// Compiles a config into a linter. Infallible: every rule was
    /// validated at config-load time.
    pub fn new(config: Config) -> Self {
        Self {
            rules: config.rules,
        }
    }

    /// The compiled rules, in config order.
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Lints one snapshot of a source file.
    ///
    /// Every rule scans the whole source; hits are returned sorted by
    /// position (then span end, then rule id) so output is deterministic
    /// regardless of rule order. Zero-width matches are skipped.
    pub fn lint(&self, source: &str) -> Vec<Violation> {
        let mut out = Vec::new();
        for rule in &self.rules {
            for caps in rule.regex.captures_iter(source) {
                let whole = match caps.get(0) {
                    Some(m) if !m.is_empty() => m,
                    _ => continue,
                };
                out.push(Violation {
                    rule_id: rule.id.clone(),
                    severity: rule.severity,
                    span: Span::new(whole.start(), whole.end()),
                    message: rule.message.render(&caps),
                });
            }
        }
        out.sort_by(|a, b| {
            (a.span.start, a.span.end, &a.rule_id, &a.message).cmp(&(
                b.span.start,
                b.span.end,
                &b.rule_id,
                &b.message,
            ))
        });
        out
    }
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
}
