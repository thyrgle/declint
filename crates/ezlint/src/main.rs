//! `ezlint` — a YAML-configured regex linter and language server.
//!
//! * `ezlint serve [CONFIG]` — run as an LSP server on stdio.
//! * `ezlint check [--language ID] [--config CONFIG] FILES...` — lint
//!   files from the command line (CI-friendly, `file:line:col:
//!   severity[id]: message` output, exit 1 on any violation).
//!
//! Without an explicit `CONFIG`, both subcommands discover a config site
//! from the current directory: `.ezlint.yaml`, then `.ezlint/`, then the
//! legacy `ezlint.yaml`, walking up through parent directories.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use ezlint_core::{language_from_extension, ConfigSet};

#[derive(Parser)]
#[command(
    name = "ezlint",
    version,
    about = "A YAML-configured regex linter and language server",
    after_help = "Config search order: .ezlint.yaml, .ezlint/, ezlint.yaml — walking up \
                  from the current directory."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run an LSP server on stdio, publishing rule violations as
    /// diagnostics.
    Serve {
        /// Path to a config file or a directory of configs; omitted =
        /// discover one from the current directory upward.
        config: Option<PathBuf>,
    },
    /// Lint files and print violations; exits 1 if any were found.
    Check {
        /// Path to a config file or a directory of configs; omitted =
        /// discover one from the current directory upward.
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// Lint as if the files had this language id (editor filetype);
        /// omitted = guess from the file extension.
        #[arg(short, long)]
        language: Option<String>,
        /// Files to lint.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
}

fn load_set(config: Option<&PathBuf>) -> ConfigSet {
    let loaded = match config {
        Some(path) => ConfigSet::load(path),
        None => ConfigSet::discover(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))),
    };
    match loaded {
        Ok(set) => set,
        Err(e) => {
            eprintln!("ezlint: {e}");
            std::process::exit(2);
        }
    }
}

fn check(config: Option<&PathBuf>, language: Option<&String>, files: &[PathBuf]) -> ExitCode {
    let set = load_set(config);
    let linters: Vec<_> = set
        .configs()
        .iter()
        .map(|named| ezlint_core::Linter::new(named.config.clone()))
        .collect();

    let mut total = 0usize;
    let mut unreadable = 0usize;

    for path in files {
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(e) => {
                eprintln!("ezlint: cannot read {}: {e}", path.display());
                unreadable += 1;
                continue;
            }
        };
        let inferred = language_from_extension(path);
        let detected = language
            .map(String::as_str)
            .or(inferred.as_deref());
        let mut violations = Vec::new();
        for (i, named) in set.configs().iter().enumerate() {
            if !named.config.matches_language(detected.unwrap_or("")) {
                continue;
            }
            violations.extend(linters[i].lint_all(&source));
        }
        violations.sort();
        for violation in &violations {
            let (line, col) = ezlint_core::line_col(&source, violation.span.start);
            println!(
                "{}:{line}:{col}: {}[{}]: {}",
                path.display(),
                violation.severity,
                violation.rule_id,
                violation.message
            );
            total += 1;
        }
    }

    if unreadable > 0 {
        eprintln!("ezlint: {unreadable} file(s) could not be read");
        return ExitCode::from(2);
    }
    if total > 0 {
        eprintln!("ezlint: {total} violation(s)");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve { config } => {
            let set = load_set(config.as_ref());
            match ezlint_lsp::serve(set) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("ezlint: server error: {e}");
                    ExitCode::from(2)
                }
            }
        }
        Command::Check {
            config,
            language,
            files,
        } => check(config.as_ref(), language.as_ref(), &files),
    }
}
