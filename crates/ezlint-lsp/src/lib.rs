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
//! A [`ConfigSet`] may hold several configs (e.g. a `.ezlint/` directory
//! with one file per language); each document only receives diagnostics
//! from the configs whose `languages` match its `languageId`.
//!
//! This crate's own job is turning [`Violation`]s into `Diagnostic`s with
//! `source: "ezlint"` and `code: <rule-id>` (so editors can filter and
//! link per rule).
//!
//! # Examples
//!
//! ```
//! use ezlint_core::{ConfigSet, CONFIG_FILE};
//! # fn get_config_dir() -> String { String::new() }
//! # fn not_run() {
//! let set = ConfigSet::discover(std::path::Path::new(&get_config_dir())).unwrap();
//! ezlint_lsp::serve(set).unwrap();
//! # }
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::error::Error;

use ezlint_core::{
    segment_all, ConfigSet, Linter, Scope, Severity, Violation,
};
use increparse::{Engine, Outcome, Pass, Span};
use increparse_lsp::{Document, SimpleLanguage};
#[cfg(test)]
use increparse_lsp::Language;
use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};

/// A parse-tree context: the whole file, or one scope's region.
///
/// The `usize` is the scope's index in the [`ConfigSet`]'s scope table
/// (stable across edits — region reuse matches contexts by equality).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ctx {
    /// The whole document, before segmentation.
    Root,
    /// A region produced by the scope at this scope-table index.
    Scoped {
        /// Index into [`ConfigSet`]'s scope table (see
        /// [`ConfigSet::scope_table`]).
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
/// `(scope-table index, byte range)` pairs.
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
/// config set's violations as diagnostics — for embedding ezlint into
/// your own server.
///
/// Only configs whose `languages` match the document's `languageId`
/// contribute diagnostics.
pub fn language(set: ConfigSet) -> SimpleLanguage<Ctx> {
    let table = set.scope_table();
    // Global scope index -> (config index, local scope index).
    let owners: Vec<(usize, usize)> =
        table.iter().map(|e| (e.config, e.local)).collect();
    let scopes: Vec<Scope> = table.into_iter().map(|e| e.scope).collect();
    let linters: Vec<Linter> = set
        .configs()
        .iter()
        .map(|named| Linter::new(named.config.clone()))
        .collect();
    let languages: Vec<Vec<String>> = set
        .configs()
        .iter()
        .map(|named| named.config.languages.clone())
        .collect();

    let engine = Engine::with((Segmenter { scopes }, Accept));
    SimpleLanguage::new(engine, Ctx::Root).extra_diagnostics(move |doc| {
        let text = doc.text();
        // Group the tree's regions per config, remapping global scope
        // indices to each config's local ones.
        let mut per_config: Vec<Vec<(usize, ezlint_core::Span)>> =
            vec![Vec::new(); linters.len()];
        for (global, span) in tree_segments(doc) {
            if let Some(&(config, local)) = owners.get(global) {
                per_config[config].push((local, span));
            }
        }

        let mut violations: Vec<Violation> = Vec::new();
        for (i, linter) in linters.iter().enumerate() {
            let applies = languages[i].is_empty()
                || languages[i].iter().any(|l| l == doc.language_id());
            if !applies {
                continue;
            }
            violations.extend(linter.lint_merged(text, &per_config[i]));
        }
        violations.sort();
        violations
            .iter()
            .map(|violation| diagnostic(doc, violation))
            .collect()
    })
}

/// Runs an ezlint language server on stdio until the client sends
/// `shutdown` + `exit`.
pub fn serve(set: ConfigSet) -> Result<(), Box<dyn Error + Send + Sync>> {
    increparse_lsp::serve(language(set))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ezlint_core::Config;
    use increparse::{CancelToken, SerialExecutor};
    use increparse_lsp::PositionEncoding;
    use lsp_types::Uri;

    const SH_YAML: &str = "\
version: 1
languages: [sh]
rules:
  - id: no-sudo
    pattern: '\\bsudo\\b'
    message: no sudo
    severity: error
";

    const ANY_YAML: &str = "\
version: 1
rules:
  - id: no-tabs
    pattern: '\\t+'
    message: tab
scopes:
  - id: fence
    start: '^```sh$'
    end: '^```$'
    rules:
      - id: scoped-rule
        pattern: 'danger'
        message: danger
        severity: warning
";

    fn set_from(yamls: &[&str]) -> ConfigSet {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("ezlint-lsp-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".ezlint")).unwrap();
        for (i, yaml) in yamls.iter().enumerate() {
            std::fs::write(dir.join(".ezlint").join(format!("c{i}.yaml")), yaml).unwrap();
        }
        ConfigSet::discover(&dir).unwrap()
    }

    fn doc_with(language_id: &str, text: &str) -> Document<Ctx> {
        let uri: Uri = "file:///t.txt".parse().unwrap();
        let mut doc = Document::open(
            uri,
            0,
            language_id.into(),
            text.into(),
            PositionEncoding::Utf16,
            Ctx::Root,
        );
        let lang = language(set_from(&[SH_YAML, ANY_YAML]));
        doc.apply_changes(
            lang.engine(),
            0,
            &[],
            &SerialExecutor,
            &CancelToken::new(),
        );
        doc
    }

    fn codes(diags: &[Diagnostic]) -> Vec<&str> {
        diags
            .iter()
            .map(|d| match d.code.as_ref().unwrap() {
                NumberOrString::String(s) => s.as_str(),
                _ => unreachable!(),
            })
            .collect()
    }

    #[test]
    fn language_id_selects_configs() {
        let lang = language(set_from(&[SH_YAML, ANY_YAML]));
        // A shell doc gets rules from both configs; a python doc only the
        // unrestricted one.
        let sh_doc = doc_with("sh", "sudo\tx\n");
        let py_doc = doc_with("python", "sudo\tx\n");

        let sh_diags = Language::extra_diagnostics(&lang, &sh_doc);
        let sh_codes = codes(&sh_diags);
        assert_eq!(sh_codes, ["no-sudo", "no-tabs"]);

        let py_diags = Language::extra_diagnostics(&lang, &py_doc);
        let py_codes = codes(&py_diags);
        assert_eq!(py_codes, ["no-tabs"]);
    }

    #[test]
    fn scoped_regions_from_any_config_are_linted() {
        let lang = language(set_from(&[ANY_YAML]));
        let text = "danger\t\n```sh\ndanger\n```\n";
        let doc = doc_with("", text);
        let diags = Language::extra_diagnostics(&lang, &doc);
        // The tab (global, line 0) and the fenced `danger` (scoped, line 2).
        let got = codes(&diags);
        assert_eq!(got, ["no-tabs", "scoped-rule"]);
        assert_eq!(diags[0].range.start.line, 0);
        assert_eq!(diags[1].range.start.line, 2);
    }

    #[test]
    fn edits_outside_regions_keep_them() {
        let lang = language(set_from(&[ANY_YAML]));
        let mut doc = doc_with("", "danger\t\n```sh\ndanger\n```\n");
        let before = tree_segments(&doc);

        // Replace the tab with a space (line 0, chars 6..7).
        let change = lsp_types::TextDocumentContentChangeEvent {
            range: Some(lsp_types::Range {
                start: lsp_types::Position { line: 0, character: 6 },
                end: lsp_types::Position { line: 0, character: 7 },
            }),
            range_length: None,
            text: " ".into(),
        };
        doc.apply_changes(lang.engine(), 1, &[change], &SerialExecutor, &CancelToken::new());

        assert_eq!(before, tree_segments(&doc), "the region kept its identity");
        let diags = Language::extra_diagnostics(&lang, &doc);
        assert_eq!(codes(&diags), ["scoped-rule"], "the tab healed, fence remains");
    }

    #[test]
    fn single_config_still_works() {
        let config = Config::from_str(SH_YAML).unwrap();
        let lang = language(ConfigSet::single(config));
        let doc = doc_with("sh", "sudo\n");
        assert_eq!(codes(&Language::extra_diagnostics(&lang, &doc)), ["no-sudo"]);
    }
}
