//! Inline suppressions: `declint:disable` comments that keep specific
//! violations out of the output.
//!
//! Two markers, recognized anywhere in a line (in a comment, in
//! whatever comment syntax the language uses):
//!
//! * `declint:disable` — suppresses rules **on this line**;
//! * `declint:disable-next-line` — suppresses rules **on the next
//!   line**.
//!
//! Either marker takes an optional comma-separated rule list
//! (`declint:disable=no-tabs,trailing-whitespace`); without it, every
//! rule is suppressed.

use std::collections::HashMap;

const DISABLE: &str = "declint:disable";
const DISABLE_NEXT_LINE: &str = "declint:disable-next-line";

/// The suppressions found in one source snapshot.
#[derive(Debug, Clone, Default)]
pub struct Suppressions {
    /// Line number (1-based) → rule ids suppressed on that line
    /// (empty = all rules).
    by_line: HashMap<usize, Vec<String>>,
}

impl Suppressions {
    /// Scans `source` for suppression markers.
    pub fn scan(source: &str) -> Self {
        let mut by_line: HashMap<usize, Vec<String>> = HashMap::new();
        for (index, line) in source.lines().enumerate() {
            let Some((offset, next_line)) = find_marker(line) else {
                continue;
            };
            let line_no = if next_line { index + 2 } else { index + 1 };
            let rule_ids = parse_ids(&line[offset..]);
            by_line.entry(line_no).or_default().extend(rule_ids);
        }
        Self { by_line }
    }

    /// Whether a violation of `rule_id` spanning `start..end` is
    /// suppressed.
    pub fn is_suppressed(
        &self,
        rule_id: &str,
        source: &str,
        start: usize,
        end: usize,
    ) -> bool {
        let (start_line, _) = crate::line_col(source, start);
        let last = end.saturating_sub(1).max(start);
        let (end_line, _) = crate::line_col(source, last);
        (start_line..=end_line).any(|line| {
            self.by_line
                .get(&line)
                .is_some_and(|ids| ids.is_empty() || ids.iter().any(|id| id == rule_id))
        })
    }

    /// Whether anything is suppressed at all.
    pub fn is_empty(&self) -> bool {
        self.by_line.is_empty()
    }
}

/// Finds the first marker occurrence in a line, returning the offset
/// just past the marker and whether it is the next-line variant.
fn find_marker(line: &str) -> Option<(usize, bool)> {
    let plain = line.find(DISABLE)?;
    if line[plain..].starts_with(DISABLE_NEXT_LINE) {
        Some((plain + DISABLE_NEXT_LINE.len(), true))
    } else {
        Some((plain + DISABLE.len(), false))
    }
}

/// Parses the optional `=id1,id2` tail of a marker. No tail = all
/// rules.
fn parse_ids(rest: &str) -> Vec<String> {
    let Some(list) = rest.strip_prefix('=').map(str::trim) else {
        return Vec::new();
    };
    list.split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect()
}

/// Drops violations suppressed by inline markers in `source`, returning
/// how many were dropped.
pub fn apply(source: &str, violations: &mut Vec<crate::Violation>) -> usize {
    let suppressions = Suppressions::scan(source);
    if suppressions.is_empty() {
        return 0;
    }
    let before = violations.len();
    violations.retain(|violation| {
        !suppressions.is_suppressed(
            &violation.rule_id,
            source,
            violation.span.start,
            violation.span.end,
        )
    });
    before - violations.len()
}
