//! The ezlint language server: YAML lint rules in, LSP diagnostics out.
//!
//! Builds on [`increparse_lsp`]'s `serve()` skeleton — document
//! bookkeeping, incremental change translation, position encoding, and
//! diagnostics publishing are inherited; this crate's whole job is turning
//! [`Violation`]s into `Diagnostic`s with `source: "ezlint"` and
//! `code: <rule-id>` (so editors can filter and link per rule).
//!
//! The parse itself is trivial — linting is a flat regex scan, not a
//! multi-pass parse — but the plumbing around it (edits, encodings,
//! publishing) is exactly what `increparse-lsp` already does well.
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

use ezlint_core::{Config, Linter, Severity, Violation};
use increparse::{Engine, Outcome, Pass, Span};
use increparse_lsp::{Document, SimpleLanguage};
use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};

/// The trivial root context: a linted document is one flat region.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File;

/// The pass that accepts every region — linting happens outside the tree.
#[derive(Debug, Clone, Copy)]
struct Accept;

impl Pass for Accept {
    type Ctx = File;

    fn parse(&self, _source: &str, _span: Span, _ctx: &File) -> Outcome<File> {
        Outcome::Done
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
pub fn diagnostic(doc: &Document<File>, violation: &Violation) -> Diagnostic {
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

/// Builds a `SimpleLanguage` that publishes the config's violations as
/// diagnostics — for embedding ezlint into your own server.
pub fn language(config: Config) -> SimpleLanguage<File> {
    let linter = Linter::new(config);
    let engine = Engine::with((Accept,));
    SimpleLanguage::new(engine, File).extra_diagnostics(move |doc| {
        linter
            .lint(doc.text())
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
