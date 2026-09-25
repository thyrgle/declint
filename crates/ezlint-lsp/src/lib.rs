//! The ezlint language server: YAML lint rules in, LSP diagnostics out.
//!
//! Builds on [`increparse_lsp`]'s `serve()` skeleton — document
//! bookkeeping, incremental change translation, position encoding, and
//! diagnostics publishing are inherited. The parse runs as **two passes**:
//! a segmenter expands the file into scope regions (pass 1), and an
//! acceptor settles them (the subpasses' home). Because regions are
//! matched by span + context on re-parse, an edit inside one region
//! re-lints only that region — the increparse payoff.
//!
//! This crate's own job is turning [`Violation`]s into `Diagnostic`s with
//! `source: "ezlint"` and `code: <rule-id>` (so editors can filter and
//! link per rule).
//!
//! # Examples
//!
//! ```
//! use ezlint_core::Config;
//! # fn get_yaml() -> String { String::new() }
//! # fn not_run() {
//! let config = Config::from_str(&get_yaml()).unwrap();
//! ezlint_lsp::serve(config).unwrap();
//! # }
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::error::Error;

use ezlint_core::{segment_all, Config, Linter, Scope, Severity, Violation};
use increparse::{Engine, Outcome, Pass, Span};
use increparse_lsp::{Document, SimpleLanguage};
#[cfg(test)]
use increparse_lsp::Language;
use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};

/// A parse-tree context: the whole file, or one scope's region.
///
/// The `usize` is the scope's index in the config (stable across edits —
/// region reuse matches contexts by equality).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ctx {
    /// The whole document, before segmentation.
    Root,
    /// A region produced by the scope at this config index.
    Scoped {
        /// Index into [`Config::scopes`] (via [`Linter::scopes`]).
        scope: usize,
    },
}

/// Pass 1: expands the root into each scope's regions.
struct Segmenter {
    scopes: Vec<Scope>,
}

impl Pass for Segmenter {
    type Ctx = Ctx;

    fn parse(&self, source: &str, span: Span, ctx: &Ctx) -> Outcome<Ctx> {
        match ctx {
            Ctx::Root => {
                let text = &source[span.to_range()];
                let regions = segment_all(text, &self.scopes);
                if regions.is_empty() {
                    return Outcome::Done;
                }
                Outcome::Expand(
                    regions
                        .into_iter()
                        .map(|(scope, region)| {
                            (
                                Span::new(
                                    span.start + region.start,
                                    span.start + region.end,
                                    span.rev,
                                ),
                                Ctx::Scoped { scope },
                            )
                        })
                        .collect(),
                )
            }
            Ctx::Scoped { .. } => Outcome::Done,
        }
    }

    fn name(&self) -> &'static str {
        "Segmenter"
    }
}

/// Pass 2: accepts every region — linting happens outside the tree.
#[derive(Debug, Clone, Copy)]
struct Accept;

impl Pass for Accept {
    type Ctx = Ctx;

    fn parse(&self, _source: &str, _span: Span, _ctx: &Ctx) -> Outcome<Ctx> {
        Outcome::Done
    }

    fn name(&self) -> &'static str {
        "Accept"
    }
}

/// Maps an ezlint severity to an LSP diagnostic severity.
pub fn lsp_severity(severity: Severity) -> DiagnosticSeverity {
    match severity {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Info => DiagnosticSeverity::INFORMATION,
        Severity::Hint => DiagnosticSeverity::HINT,
    }
}

/// Renders one violation as an LSP diagnostic in `doc`.
pub fn diagnostic(doc: &Document<Ctx>, violation: &Violation) -> Diagnostic {
    let span = Span::new(violation.span.start, violation.span.end, doc.revision());
    Diagnostic {
        range: doc.range(span),
        severity: Some(lsp_severity(violation.severity)),
        code: Some(NumberOrString::String(violation.rule_id.clone())),
        code_description: None,
        source: Some("ezlint".into()),
        message: violation.message.clone(),
        related_information: None,
        tags: None,
        data: None,
    }
}

/// Collects the scope regions currently in the tree as
/// `(scope index, byte range)` pairs.
fn tree_segments(doc: &Document<Ctx>) -> Vec<(usize, ezlint_core::Span)> {
    let tree = doc.session().tree();
    tree.nodes()
        .filter_map(|id| match tree.ctx(id) {
            Ctx::Scoped { scope } => {
                let span = tree.span(id);
                Some((*scope, ezlint_core::Span::new(span.start, span.end)))
            }
            Ctx::Root => None,
        })
        .collect()
}

/// Builds a `SimpleLanguage` that segments documents and publishes the
/// config's violations as diagnostics — for embedding ezlint into your own
/// server.
pub fn language(config: Config) -> SimpleLanguage<Ctx> {
    let linter = Linter::new(config);
    let scopes = linter.scopes().to_vec();
    let engine = Engine::with((Segmenter { scopes }, Accept));
    SimpleLanguage::new(engine, Ctx::Root).extra_diagnostics(move |doc| {
        let text = doc.text();
        let segments = tree_segments(doc);
        linter
            .lint_merged(text, &segments)
            .iter()
            .map(|violation| diagnostic(doc, violation))
            .collect()
    })
}

/// Runs an ezlint language server on stdio until the client sends
/// `shutdown` + `exit`.
pub fn serve(config: Config) -> Result<(), Box<dyn Error + Send + Sync>> {
    increparse_lsp::serve(language(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use increparse::{CancelToken, SerialExecutor};
    use increparse_lsp::PositionEncoding;
    use lsp_types::Uri;

    const YAML: &str = "\
version: 1
rules:
  - id: no-tabs
    pattern: '\\t+'
    message: tab
scopes:
  - id: sh
    start: '^```sh$'
    end: '^```$'
    rules:
      - id: no-sudo
        pattern: '\\bsudo\\b'
        message: no sudo
        severity: error
";

    fn doc_with(text: &str) -> Document<Ctx> {
        let uri: Uri = "file:///t.txt".parse().unwrap();
        let mut doc = Document::open(uri, 0, text.into(), PositionEncoding::Utf16, Ctx::Root);
        let language = language(Config::from_str(YAML).unwrap());
        doc.apply_changes(
            language.engine(),
            0,
            &[],
            &SerialExecutor,
            &CancelToken::new(),
        );
        doc
    }

    fn diagnostics_for(text: &str) -> Vec<Diagnostic> {
        let language = language(Config::from_str(YAML).unwrap());
        let doc = doc_with(text);
        Language::extra_diagnostics(&language, &doc)
    }

    #[test]
    fn global_and_scoped_violations_are_published() {
        let text = "a\tb\n```sh\nsudo ls\n```\n";
        let diags = diagnostics_for(text);
        let codes: Vec<&str> = diags
            .iter()
            .map(|d| match d.code.as_ref().unwrap() {
                NumberOrString::String(s) => s.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(codes, ["no-tabs", "no-sudo"]);
        assert_eq!(diags[0].severity, Some(lsp_severity(Severity::Warning)));
        assert_eq!(diags[1].severity, Some(lsp_severity(Severity::Error)));
        // The tab on line 0; sudo on line 2.
        assert_eq!(diags[0].range.start.line, 0);
        assert_eq!(diags[1].range.start.line, 2);
    }

    #[test]
    fn no_scope_matches_means_done_tree() {
        let diags = diagnostics_for("plain text\n");
        assert!(diags.is_empty());
        let doc = doc_with("plain text\n");
        let tree = doc.session().tree();
        assert_eq!(tree.status(tree.root()), increparse::Status::Done);
    }

    #[test]
    fn edits_outside_regions_keep_them() {
        let language = language(Config::from_str(YAML).unwrap());
        let mut doc = doc_with("a\tb\n```sh\nsudo ls\n```\n");
        let before = tree_segments(&doc);

        // Replace the tab with a space (line 0, chars 1..2).
        let change = lsp_types::TextDocumentContentChangeEvent {
            range: Some(lsp_types::Range {
                start: lsp_types::Position { line: 0, character: 1 },
                end: lsp_types::Position { line: 0, character: 2 },
            }),
            range_length: None,
            text: " ".into(),
        };
        doc.apply_changes(language.engine(), 1, &[change], &SerialExecutor, &CancelToken::new());

        let after = tree_segments(&doc);
        assert_eq!(before, after, "the region kept its identity");

        let diags = Language::extra_diagnostics(&language, &doc);
        let codes: Vec<&str> = diags
            .iter()
            .map(|d| match d.code.as_ref().unwrap() {
                NumberOrString::String(s) => s.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(codes, ["no-sudo"], "the tab healed, sudo remains");
    }
}
