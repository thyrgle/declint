//! `declint` — a YAML-configured regex linter and language server.
//!
//! * `declint serve [CONFIG]` — run as an LSP server on stdio.
//! * `declint check [--language ID] [--config CONFIG] FILES...` — lint
//!   files from the command line (CI-friendly, `file:line:col:
//!   severity[id]: message` output, exit 1 on any violation).
//!
//! Without an explicit `CONFIG`, both subcommands discover a config site
//! from the current directory: `.declint.yaml`, then `.declint/`, then the
//! legacy `declint.yaml`, walking up through parent directories.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use declint_core::{language_from_extension, ConfigSet};

#[derive(Parser)]
#[command(
    name = "declint",
    version,
    about = "A YAML-configured regex linter and language server",
    after_help = "Config search order: .declint.yaml, .declint/, declint.yaml — walking up \
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
            eprintln!("declint: {e}");
            std::process::exit(2);
        }
    }
}

fn load_callbacks(set: &ConfigSet) -> declint_core::Callbacks {
    let mut callbacks = declint_core::Callbacks::new();
    if let Err(e) = declint_lua::attach(set, &mut callbacks) {
        eprintln!("declint: {e}");
        std::process::exit(2);
    }
    callbacks
}

fn check(config: Option<&PathBuf>, language: Option<&String>, files: &[PathBuf]) -> ExitCode {
    let set = load_set(config);
    let callbacks = load_callbacks(&set);
    let linters = set
        .configs()
        .iter()
        .map(|named| declint_core::Linter::new(named.config.clone(), &callbacks))
        .collect::<Result<Vec<_>, _>>();
    let linters = match linters {
        Ok(linters) => linters,
        Err(e) => {
            eprintln!("declint: {e}");
            return ExitCode::from(2);
        }
    };

    let mut total = 0usize;
    let mut unreadable = 0usize;

    for path in files {
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(e) => {
                eprintln!("declint: cannot read {}: {e}", path.display());
                unreadable += 1;
                continue;
            }
        };
        let inferred = language_from_extension(path);
        let detected = language
            .map(String::as_str)
            .or(inferred.as_deref());
        let info = declint_core::DocInfo {
            path: &path.display().to_string(),
            language: detected.unwrap_or(""),
        };
        let mut violations = Vec::new();
        for (i, named) in set.configs().iter().enumerate() {
            if !named.config.matches_language(detected.unwrap_or("")) {
                continue;
            }
            violations.extend(linters[i].lint_all_in(info, &source));
        }
        violations.sort();
        for violation in &violations {
            let (line, col) = declint_core::line_col(&source, violation.span.start);
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
        eprintln!("declint: {unreadable} file(s) could not be read");
        return ExitCode::from(2);
    }
    if total > 0 {
        eprintln!("declint: {total} violation(s)");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve { config } => {
            let set = load_set(config.as_ref());
            let callbacks = load_callbacks(&set);
            match declint_lsp::serve(set, &callbacks) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("declint: server error: {e}");
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
