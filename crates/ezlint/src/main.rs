//! `ezlint` — a YAML-configured regex linter and language server.
//!
//! * `ezlint serve [CONFIG]` — run as an LSP server on stdio.
//! * `ezlint check [--config CONFIG] FILES...` — lint files from the
//!   command line (CI-friendly, `file:line:col: severity[id]: message`
//!   output, exit 1 on any violation).

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use ezlint_core::Config;

/// The config file used when none is given.
const DEFAULT_CONFIG: &str = "ezlint.yaml";

#[derive(Parser)]
#[command(
    name = "ezlint",
    version,
    about = "A YAML-configured regex linter and language server",
    after_help = "Config schema: https://github.com/ezlint/ezlint (see examples/rules.yaml)"
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
        /// Path to the config file.
        #[arg(default_value = DEFAULT_CONFIG)]
        config: PathBuf,
    },
    /// Lint files and print violations; exits 1 if any were found.
    Check {
        /// Path to the config file.
        #[arg(short, long, default_value = DEFAULT_CONFIG)]
        config: PathBuf,
        /// Files to lint.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
}

fn load_config(path: &PathBuf) -> Config {
    match Config::load(path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("ezlint: {e}");
            std::process::exit(2);
        }
    }
}

fn check(config_path: &PathBuf, files: &[PathBuf]) -> ExitCode {
    let linter = ezlint_core::Linter::new(load_config(config_path));
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
        for violation in linter.lint_all(&source) {
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
            let config = load_config(&config);
            match ezlint_lsp::serve(config) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("ezlint: server error: {e}");
                    ExitCode::from(2)
                }
            }
        }
        Command::Check { config, files } => check(&config, &files),
    }
}
