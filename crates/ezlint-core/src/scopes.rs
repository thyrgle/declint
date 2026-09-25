//! Scope segmentation: where each scope's regions begin and end.
//!
//! This is the "pass 1" of the two-pass shape: segment the file into
//! regions, then run each scope's rules inside its regions (the
//! "subpasses").

use crate::config::Scope;
use crate::linter::Span;

/// Finds the regions of one scope in `source`.
///
/// Regions are sequential: the search for the next `start` resumes where
/// the previous region ended, so a scope's regions never overlap each
/// other. A region spans from its `start` match to the next `end` match
/// after that (or end of file when the scope has no `end`). Empty regions
/// are dropped.
pub fn segment(source: &str, scope: &Scope) -> Vec<Span> {
    let mut regions = Vec::new();
    let mut pos = 0usize;
    while let Some(start_match) = scope.start_re.find_at(source, pos) {
        let end = scope
            .end_re
            .as_ref()
            .and_then(|re| re.find_at(source, start_match.end()))
            .map_or(source.len(), |m| m.start())
            .max(start_match.end());
        let span = Span::new(start_match.start(), end);
        if !span.is_empty() {
            regions.push(span);
        }
        // Always advance, even past a degenerate (empty) start match, so
        // the loop cannot stall.
        pos = if start_match.is_empty() {
            span.end.max(start_match.end() + 1)
        } else {
            span.end
        };
    }
    regions
}

/// Segments `source` for every scope, as `(scope index, region)` pairs
/// sorted by position (scope order breaks ties). Regions from different
/// scopes may overlap — each scope's family is independent.
pub fn segment_all(source: &str, scope_list: &[Scope]) -> Vec<(usize, Span)> {
    let mut out: Vec<(usize, Span)> = scope_list
        .iter()
        .enumerate()
        .flat_map(|(scope_index, scope)| {
            segment(source, scope)
                .into_iter()
                .map(move |region| (scope_index, region))
        })
        .collect();
    out.sort_by_key(|(scope_index, region)| (region.start, region.end, *scope_index));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    fn segment_with(yaml: &str, text: &str, scope_id: &str) -> Vec<Span> {
        let config = Config::from_str(yaml).expect("config parses");
        let scope = config
            .scopes
            .iter()
            .find(|s| s.id == scope_id)
            .expect("scope exists");
        segment(text, scope)
    }

    const SHELL_FENCE: &str = "\
version: 1
scopes:
  - id: sh
    start: '^```sh$'
    end: '^```$'
    rules:
      - id: r
        pattern: 'x'
        message: m
";

    #[test]
    fn fenced_regions() {
        let text = "a\n```sh\nb\n```\nc\n";
        let regions = segment_with(SHELL_FENCE, text, "sh");
        assert_eq!(regions.len(), 1);
        let inner = &text[regions[0].to_range()];
        assert_eq!(inner, "```sh\nb\n");
    }

    #[test]
    fn no_end_means_eof() {
        let yaml = "\
version: 1
scopes:
  - id: tail
    start: '^BEGIN$'
    rules:
      - id: r
        pattern: x
        message: m
";
        let text = "x\nBEGIN\nrest of file";
        let regions = segment_with(yaml, text, "tail");
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].start, 2);
        assert_eq!(regions[0].end, text.len());
    }

    #[test]
    fn regions_are_sequential() {
        let text = "```sh\na\n```\nmid\n```sh\nb\n```\n";
        let regions = segment_with(SHELL_FENCE, text, "sh");
        let inner: Vec<&str> = regions.iter().map(|r| &text[r.to_range()]).collect();
        assert_eq!(inner, ["```sh\na\n", "```sh\nb\n"]);
    }

    #[test]
    fn no_matches_yields_no_regions() {
        assert!(segment_with(SHELL_FENCE, "nothing here\n", "sh").is_empty());
    }

    #[test]
    fn end_right_after_start_gives_start_only_region() {
        // `end` on the very next character: the region is just the start
        // match (non-empty, kept). Genuinely empty regions (empty start
        // match) are dropped — see the stall test below.
        let yaml = "\
version: 1
scopes:
  - id: s
    start: 'X'
    end: 'X'
    rules:
      - id: r
        pattern: x
        message: m
";
        let regions = segment_with(yaml, "XX\n", "s");
        let ranges: Vec<_> = regions.iter().map(|r| r.to_range()).collect();
        // The first X's region ends where the second X begins, so the
        // second X starts its own region — regions stay disjoint.
        assert_eq!(ranges, [0..1, 1..3]);
    }

    #[test]
    fn empty_start_match_cannot_stall() {
        let yaml = "\
version: 1
scopes:
  - id: s
    start: 'x*'
    end: 'y'
    rules:
      - id: r
        pattern: x
        message: m
";
        // `x*` matches empty at every position; segmentation must still
        // terminate. "a y b": first region ends at the `y`. The empty
        // match *at* the `y` yields an immediately-empty region (dropped),
        // so the tail region starts after it.
        let regions = segment_with(yaml, "a y b", "s");
        let ranges: Vec<_> = regions.iter().map(|r| r.to_range()).collect();
        assert_eq!(ranges, [0..2, 3..5]);
    }

    #[test]
    fn different_scopes_may_overlap() {
        let yaml = "\
version: 1
scopes:
  - id: whole
    start: '^A$'
    rules:
      - id: r1
        pattern: x
        message: m
  - id: line
    start: 'B'
    end: '$'
    rules:
      - id: r2
        pattern: x
        message: m
";
        let config = Config::from_str(yaml).unwrap();
        let text = "A\nB\nC\n";
        let all = segment_all(text, &config.scopes);
        assert_eq!(all.len(), 2);
        // `whole` covers from line A to EOF; `line` covers just the B line.
        assert_eq!(&text[all[0].1.to_range()], "A\nB\nC\n");
        assert_eq!(&text[all[1].1.to_range()], "B");
    }
}
