//! Config and linting engine for [`ezlint`] — no LSP dependencies.
//!
//! An ezlint configuration is a YAML file of regex rules:
//!
//! ```yaml
//! version: 1
//! rules:
//!   - id: no-tabs
//!     pattern: '\t+'
//!     message: "Use spaces, found '{match}'"
//!     severity: warning
//! ```
//!
//! Load it, lint text, get violations:
//!
//! ```
//! use ezlint_core::{Config, Linter};
//!
//! # fn main() -> Result<(), ezlint_core::ConfigError> {
//! let config = Config::from_str(
//!     "version: 1\nrules:\n  - id: no-tabs\n    pattern: '\\t+'\n    message: \"Use \
//!      spaces\"\n    severity: warning\n",
//! )?;
//! let linter = Linter::new(config);
//!
//! let violations = linter.lint("a\tb");
//! assert_eq!(violations.len(), 1);
//! assert_eq!(violations[0].rule_id, "no-tabs");
//! assert_eq!(violations[0].span.to_range(), 1..2);
//! # Ok(())
//! # }
//! ```
//!
//! Everything is validated at load time — regex syntax, placeholder names,
//! duplicate rule ids, unknown severities — so a rule file is either fully
//! usable or rejected with the rule id and file line of the problem.
//!
//! [`ezlint`]: https://crates.io/crates/ezlint

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod config;
mod linter;
mod template;

pub use config::{Config, ConfigError, Rule, SUPPORTED_VERSION};
pub use linter::{line_col, Linter, Span, Violation};
pub use template::Template;

/// How serious a violation is.
///
/// Rendered one-to-one as an LSP diagnostic severity by `ezlint-lsp`, and
/// printed verbatim by `ezlint check`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    /// A definite problem.
    Error,
    /// A probable problem — the default for rules that omit `severity`.
    Warning,
    /// A suggestion.
    Info,
    /// A nitpick, rendered faintly by most editors.
    Hint,
}

impl Severity {
    /// Parses a severity from its config-file spelling.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "error" => Some(Self::Error),
            "warning" => Some(Self::Warning),
            "info" => Some(Self::Info),
            "hint" => Some(Self::Hint),
            _ => None,
        }
    }

    /// The config-file spelling of this severity.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::Hint => "hint",
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
