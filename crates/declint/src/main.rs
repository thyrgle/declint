//! `declint` — a YAML-configured regex linter and language server.
//!
//! * `declint serve [CONFIG]` — run as an LSP server on stdio.
//! * `declint check [--language ID] [--config CONFIG] [--format FMT]
//!   FILES...` — lint files (or directories, walked recursively) and
//!   print violations; exit 1 if any were found. `--format github`
//!   emits GitHub Actions workflow commands, so violations appear as
//!   inline annotations on pull requests.
//!
//! Without an explicit `CONFIG`, both subcommands discover a config site
//! from the current directory: `.declint.yaml`, then `.declint/`, then
//! `declint.yaml`, walking up through parent directories.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use declint_core::{language_from_extension, ConfigSet, Violation};

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
        /// Output format: plain text, or GitHub Actions workflow
        /// commands (inline PR annotations).
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
        /// Minimum severity that affects the exit code: violations below
        /// it are still reported, but no longer fail the run.
        #[arg(long, value_enum, default_value_t = FailOn::Hint)]
        fail_on: FailOn,
        /// Files to lint. Directories are walked recursively
        /// (respecting .gitignore); files that are not valid UTF-8 are
        /// skipped.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// List the embedded preset library, or show one preset's YAML.
    Presets {
        /// Show this preset's full YAML instead of listing all presets.
        name: Option<String>,
    },
    /// Scaffold a starter `.declint.yaml` in the current directory.
    Init {
        /// Start from a preset (`python`, `ini`, `markdown`): the config
        /// imports it and pins the language.
        #[arg(long)]
        lang: Option<String>,
    },
    /// Print a shell completion script (bash, zsh, fish, or powershell).
    Completions {
        /// The shell to generate completions for.
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum OutputFormat {
    /// `file:line:col: severity[id]: message`
    Text,
    /// GitHub Actions workflow commands — violations become inline
    /// annotations on the pull request.
    Github,
}

/// The severity threshold that trips the exit code. All violations are
/// always reported; `FailOn` only decides which ones count as failing.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum FailOn {
    /// Exit 1 on any violation (the default).
    Hint,
    /// Exit 1 only on info-or-higher violations.
    Info,
    /// Exit 1 only on warning-or-higher violations.
    Warning,
    /// Exit 1 only on error violations.
    Error,
}

impl FailOn {
    fn threshold(self) -> declint_core::Severity {
        match self {
            Self::Hint => declint_core::Severity::Hint,
            Self::Info => declint_core::Severity::Info,
            Self::Warning => declint_core::Severity::Warning,
            Self::Error => declint_core::Severity::Error,
        }
    }
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

/// Expands directories in the file list: walked recursively (hidden
/// paths and `.gitignore`d paths skipped by the walker), results sorted
/// for deterministic output. The flag records which files came from a
/// walk — those are skipped silently when not valid UTF-8, where
/// explicitly named files keep their hard error.
fn collect_files(paths: &[PathBuf]) -> Vec<(PathBuf, bool)> {
    let mut out = Vec::new();
    for path in paths {
        if path.is_dir() {
            for entry in ignore::Walk::new(path).flatten() {
                if entry.file_type().is_some_and(|t| t.is_file()) {
                    out.push((entry.into_path(), true));
                }
            }
        } else {
            out.push((path.clone(), false));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

fn check(
    config: Option<&PathBuf>,
    language: Option<&String>,
    format: OutputFormat,
    fail_on: FailOn,
    files: &[PathBuf],
) -> ExitCode {
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
    let mut failing = 0usize;
    let mut unreadable = 0usize;
    let threshold = fail_on.threshold();

    for (path, from_walk) in collect_files(files) {
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(e) if from_walk && e.kind() == std::io::ErrorKind::InvalidData => {
                continue; // not UTF-8: skip silently in discovery mode
            }
            Err(e) => {
                eprintln!("declint: cannot read {}: {e}", path.display());
                unreadable += 1;
                continue;
            }
        };
        let inferred = language_from_extension(&path);
        let detected = language.map(String::as_str).or(inferred.as_deref());
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
        match format {
            OutputFormat::Text => print_text(&path, &source, &violations),
            OutputFormat::Github => print_github(&path, &source, &violations),
        }
        total += violations.len();
        failing += violations
            .iter()
            .filter(|v| v.severity.rank() >= threshold.rank())
            .count();
    }

    if unreadable > 0 {
        eprintln!("declint: {unreadable} file(s) could not be read");
        return ExitCode::from(2);
    }
    if failing > 0 {
        if failing == total {
            eprintln!("declint: {total} violation(s)");
        } else {
            eprintln!("declint: {failing} failing violation(s) of {total}");
        }
        return ExitCode::from(1);
    }
    if total > 0 {
        eprintln!(
            "declint: {total} violation(s) — all below the --fail-on threshold"
        );
    }
    ExitCode::SUCCESS
}

fn print_text(path: &std::path::Path, source: &str, violations: &[Violation]) {
    for violation in violations {
        let (line, col) = declint_core::line_col(source, violation.span.start);
        println!(
            "{}:{line}:{col}: {}[{}]: {}",
            path.display(),
            violation.severity,
            violation.rule_id,
            violation.message
        );
    }
}

/// Escapes a value for use inside a GitHub Actions workflow command —
/// `%` first, then line breaks.
fn escape_workflow(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

fn print_github(path: &std::path::Path, source: &str, violations: &[Violation]) {
    for violation in violations {
        let (line, col) = declint_core::line_col(source, violation.span.start);
        // The span's end is exclusive; annotate through the last
        // included character.
        let last = violation.span.end.saturating_sub(1).max(violation.span.start);
        let (end_line, _) = declint_core::line_col(source, last);
        let command = match violation.severity {
            declint_core::Severity::Error => "error",
            declint_core::Severity::Warning => "warning",
            declint_core::Severity::Info | declint_core::Severity::Hint => "notice",
        };
        println!(
            "::{command} file={},line={line},col={col},endLine={end_line}::[declint/{}] {}",
            escape_workflow(&path.display().to_string()),
            escape_workflow(&violation.rule_id),
            escape_workflow(&violation.message),
        );
    }
}

fn presets(name: Option<&String>) -> ExitCode {
    match name {
        None => {
            for preset in declint_core::presets::PRESETS {
                println!("{}  —  {}", preset.name, preset.description);
            }
            println!();
            println!("show one:   declint presets <name>");
            println!("import one: `import: [preset:<name>]` in your .declint.yaml");
            ExitCode::SUCCESS
        }
        Some(name) => match declint_core::presets::lookup(name) {
            Some(preset) => {
                print!("{}", preset.content);
                ExitCode::SUCCESS
            }
            None => {
                eprintln!(
                    "declint: unknown preset `{name}` (available: {})",
                    declint_core::presets::names()
                );
                ExitCode::from(2)
            }
        },
    }
}

const STARTER_CONFIG: &str = "\
version: 1
rules:
  # Your first rule. Patterns are single-quoted YAML, and `(?m)` makes
  # ^ and $ anchor to lines.
  - id: no-tabs
    pattern: '(?m)^\\t+'
    message: \"Tab character — use spaces\"
    severity: warning

  # Curated starters are one import away:
  #   declint presets                       # list them
  #   import: [preset:python]               # at the top of this file
";

fn init(lang: Option<&String>) -> ExitCode {
    let target = std::path::Path::new(".declint.yaml");
    if target.exists() {
        eprintln!("declint: refusing to overwrite existing .declint.yaml");
        return ExitCode::from(2);
    }
    let content = match lang {
        None => STARTER_CONFIG.to_string(),
        Some(lang) => {
            if declint_core::presets::lookup(lang).is_none() {
                eprintln!(
                    "declint: unknown preset `{lang}` (available: {})",
                    declint_core::presets::names()
                );
                return ExitCode::from(2);
            }
            format!("version: 1\nlanguages: [{lang}]\nimport:\n  - preset:{lang}\n")
        }
    };
    std::fs::write(target, content).unwrap();
    println!("wrote .declint.yaml — try `declint check .`");
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
            format,
            fail_on,
            files,
        } => check(config.as_ref(), language.as_ref(), format, fail_on, &files),
        Command::Presets { name } => presets(name.as_ref()),
        Command::Init { lang } => init(lang.as_ref()),
        Command::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "declint", &mut std::io::stdout());
            ExitCode::SUCCESS
        }
    }
}
