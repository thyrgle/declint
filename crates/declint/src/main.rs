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

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use declint_core::{language_from_extension, ConfigSet, Violation};

mod install;

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
        /// Apply available fixes in place instead of just reporting.
        /// Exit 1 only for violations that remain after fixing.
        #[arg(long)]
        fix: bool,
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
    /// Run the embedded `tests:` fixtures for rules in the config.
    Test {
        /// Path to a config file or a directory of configs; omitted =
        /// discover one from the current directory upward.
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// Only run tests for these rule ids.
        #[arg(value_name = "RULE")]
        rules: Vec<String>,
    },
    /// Explain which configs, rules, and scopes apply to a file.
    Explain {
        /// Path to a config file or a directory of configs; omitted =
        /// discover one from the current directory upward.
        #[arg(short, long)]
        config: Option<PathBuf>,
        /// Pretend the file has this language id instead of inferring.
        #[arg(short, long)]
        language: Option<String>,
        /// The file to explain.
        file: PathBuf,
    },
    /// Install a ruleset from GitHub (vendored, reviewed, committed).
    Install {
        /// Install into the global store (~/.declint/store) instead of
        /// this project's .declint/vendor/.
        #[arg(short = 'g', long)]
        global: bool,
        /// List installed packages (project vendor tree and global
        /// store) instead of installing.
        #[arg(long)]
        list: bool,
        /// The source: gh:<owner>/<repo>[@<ref>][/subpath]; omitted
        /// with --list.
        source: Option<String>,
    },
    /// Remove an installed ruleset.
    Remove {
        /// Remove from the global store instead of the project's
        /// .declint/vendor/.
        #[arg(short = 'g', long)]
        global: bool,
        /// The package: <owner>/<repo> (or <owner>/<repo>@<ref> for
        /// project installs).
        package: String,
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
    /// A JSON array of violation objects.
    Json,
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

/// Applies non-overlapping violation fixes to `source`, returning the
/// fixed content and how many fixes were applied. Fixes are applied in
/// position order; any fix that overlaps an applied one is skipped.
fn apply_fixes(source: &str, violations: &[Violation]) -> (String, usize) {
    let mut fixes: Vec<(usize, usize, &str)> = violations
        .iter()
        .filter_map(|v| v.fix.as_ref().map(|f| (v.span.start, v.span.end, f.as_str())))
        .collect();
    fixes.sort_by_key(|(start, end, _)| (*start, *end));

    let mut out = String::with_capacity(source.len());
    let mut pos = 0usize;
    let mut applied = 0usize;
    for (start, end, replacement) in fixes {
        if start < pos || end < start {
            continue;
        }
        out.push_str(&source[pos..start]);
        out.push_str(replacement);
        pos = end;
        applied += 1;
    }
    out.push_str(&source[pos.min(source.len())..]);
    (out, applied)
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

fn print_json(
    items: &mut Vec<serde_json::Value>,
    path: &std::path::Path,
    source: &str,
    violations: &[Violation],
) {
    for violation in violations {
        let (line, col) = declint_core::line_col(source, violation.span.start);
        let last = violation.span.end.saturating_sub(1).max(violation.span.start);
        let (end_line, end_col) = declint_core::line_col(source, last);
        items.push(serde_json::json!({
            "file": path.display().to_string(),
            "line": line,
            "col": col,
            "endLine": end_line,
            "endCol": end_col,
            "rule": violation.rule_id,
            "severity": violation.severity.as_str(),
            "message": violation.message,
        }));
    }
}

fn check(
    config: Option<&PathBuf>,
    language: Option<&String>,
    format: OutputFormat,
    fail_on: FailOn,
    fix: bool,
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
    let mut json_items = Vec::new();

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

        let mut source = source;
        if fix {
            let (fixed_source, applied) = apply_fixes(&source, &violations);
            if applied > 0 {
                if let Err(e) = std::fs::write(&path, &fixed_source) {
                    eprintln!("declint: cannot write {}: {e}", path.display());
                    unreadable += 1;
                    continue;
                }
                println!("fixed {applied} violation(s) in {}", path.display());
            }
            // What remains after fixing is what fails — re-lint the
            // fixed content.
            let mut remaining = Vec::new();
            for (i, named) in set.configs().iter().enumerate() {
                if !named.config.matches_language(detected.unwrap_or("")) {
                    continue;
                }
                remaining.extend(linters[i].lint_all_in(info, &fixed_source));
            }
            remaining.sort();
            violations = remaining;
            source = fixed_source;
        }

        match format {
            OutputFormat::Text => print_text(&path, &source, &violations),
            OutputFormat::Github => print_github(&path, &source, &violations),
            OutputFormat::Json => print_json(&mut json_items, &path, &source, &violations),
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
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(&json_items).unwrap_or_else(|_| "[]".into()));
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

fn run_tests(config: Option<&PathBuf>, filters: &[String]) -> ExitCode {
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

    if !filters.is_empty() {
        let known: std::collections::HashSet<String> = set
            .configs()
            .iter()
            .flat_map(|named| {
                named
                    .config
                    .rules
                    .iter()
                    .map(|r| r.id.clone())
                    .chain(named.config.scopes.iter().flat_map(|s| {
                        s.rules.iter().map(|r| r.id.clone())
                    }))
            })
            .collect();
        if let Some(missing) = filters.iter().find(|f| !known.contains(*f)) {
            eprintln!("declint: unknown rule `{missing}`");
            return ExitCode::from(2);
        }
    }

    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut any_tests = false;

    for (i, named) in set.configs().iter().enumerate() {
        let mut rules: Vec<&declint_core::Rule> = named.config.rules.iter().collect();
        for scope in &named.config.scopes {
            rules.extend(scope.rules.iter());
        }
        for rule in rules {
            if !filters.is_empty() && !filters.iter().any(|f| f == &rule.id) {
                continue;
            }
            if rule.tests.is_empty() {
                continue;
            }
            any_tests = true;
            for test in &rule.tests {
                let label = test.name.as_deref().unwrap_or("test");
                let violations = linters[i].lint_rule(
                    &rule.id,
                    declint_core::DocInfo::none(),
                    &test.text,
                );
                let mut problems = Vec::new();
                match violations {
                    Err(e) => problems.push(format!("callback/parser error: {e}")),
                    Ok(violations) => {
                        if violations.len() != test.violations {
                            problems.push(format!(
                                "expected {} violation(s), got {}",
                                test.violations,
                                violations.len()
                            ));
                        }
                        for expected in &test.messages {
                            if !violations.iter().any(|v| &v.message == expected) {
                                problems.push(format!("missing message: {expected:?}"));
                            }
                        }
                    }
                }
                if problems.is_empty() {
                    passed += 1;
                    println!("PASS {} [{}] {label}", rule.id, named.path.display());
                } else {
                    failed += 1;
                    println!("FAIL {} [{}] {label}", rule.id, named.path.display());
                    for problem in problems {
                        println!("  {problem}");
                    }
                }
            }
        }
    }

    if !any_tests {
        println!("declint: no embedded tests found — add a `tests:` list to a rule");
        return ExitCode::SUCCESS;
    }
    if failed > 0 {
        eprintln!("declint: {passed} test(s) passed, {failed} failed");
        return ExitCode::from(1);
    }
    println!("declint: {passed} test(s) passed");
    ExitCode::SUCCESS
}

fn explain(
    config: Option<&PathBuf>,
    language: Option<&String>,
    file: &PathBuf,
) -> ExitCode {
    let set = load_set(config);
    let callbacks = load_callbacks(&set);

    let source = match std::fs::read_to_string(file) {
        Ok(source) => source,
        Err(e) => {
            eprintln!("declint: cannot read {}: {e}", file.display());
            return ExitCode::from(2);
        }
    };
    let inferred = language_from_extension(file);
    let detected = language.map(String::as_str).or(inferred.as_deref());
    println!(
        "file: {} (language: {})",
        file.display(),
        detected.unwrap_or("<none>")
    );
    println!(
        "config site: {}",
        set.configs()
            .first()
            .map(|named| named.path.display().to_string())
            .unwrap_or_else(|| "<none>".into())
    );

    let linters = set
        .configs()
        .iter()
        .map(|named| declint_core::Linter::new(named.config.clone(), &callbacks))
        .collect::<Result<Vec<_>, _>>();

    for (i, named) in set.configs().iter().enumerate() {
        let applies = named.config.matches_language(detected.unwrap_or(""));
        let languages = if named.config.languages.is_empty() {
            "all".to_string()
        } else {
            named.config.languages.join(", ")
        };
        println!(
            "\nconfig {} [languages: {languages}] — {}",
            named.path.display(),
            if applies { "applied" } else { "skipped (language)" }
        );
        if !applies {
            continue;
        }

        // Resolution status for every rule, global and scoped.
        let mut rows: Vec<(String, &declint_core::Rule)> = named
            .config
            .rules
            .iter()
            .map(|r| (String::new(), r))
            .collect();
        for scope in &named.config.scopes {
            for rule in &scope.rules {
                rows.push((format!(" [scope {}]", scope.id), rule));
            }
        }
        for (prefix, rule) in &rows {
            let kind = if rule.parser.is_some() {
                "parser"
            } else {
                "regex"
            };
            let extra = match (&rule.callback, &rule.parser) {
                (Some(reference), _) => match callbacks.resolve(reference) {
                    Some(_) => format!(", callback: {}", reference.describe()),
                    None => format!(
                        ", callback: {} — NOT REGISTERED",
                        reference.describe()
                    ),
                },
                (None, Some(reference)) => match callbacks.resolve_parser(reference) {
                    Some(_) => format!(", parser: {}", reference.describe()),
                    None => format!(
                        ", parser: {} — NOT REGISTERED",
                        reference.describe()
                    ),
                },
                (None, None) => String::new(),
            };
            let tests = if rule.tests.is_empty() {
                String::new()
            } else {
                format!(", {} test(s)", rule.tests.len())
            };
            println!(
                "  {}{}: {} [{kind}{extra}{tests}]",
                rule.id,
                prefix,
                rule.severity
            );
        }

        // Match and violation counts, when the linter is constructible.
        match &linters {
            Ok(linters) => {
                let info = declint_core::DocInfo {
                    path: &file.display().to_string(),
                    language: detected.unwrap_or(""),
                };
                let violations = linters[i].lint_all_in(info, &source);
                println!("  violations in this file: {}", violations.len());
                let mut per_rule: HashMap<&str, usize> = HashMap::new();
                for violation in &violations {
                    *per_rule.entry(violation.rule_id.as_str()).or_default() += 1;
                }
                let mut counts: Vec<(&str, usize)> = per_rule.into_iter().collect();
                counts.sort();
                for (id, count) in counts {
                    println!("    {id}: {count} match(es)");
                }
            }
            Err(e) => {
                println!("  match counts unavailable: {e}");
            }
        }
    }
    ExitCode::SUCCESS
}

fn install_list(global: bool) -> ExitCode {
    let mut found = 0usize;
    if !global {
        let vendor = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(".declint/vendor");
        if vendor.is_dir() {
            println!("project (.declint/vendor):");
            for entry in ignore::Walk::new(&vendor).flatten() {
                if entry.file_type().is_some_and(|t| t.is_file()) {
                    println!("  {}", entry.path().display());
                    found += 1;
                }
            }
        } else {
            println!("project (.declint/vendor): nothing installed");
        }
    }
    if let Some(store) = declint_core::store::global_store_dir() {
        println!("global ({}):", store.display());
        if store.is_dir() {
            for entry in ignore::Walk::new(&store).flatten() {
                if entry.file_type().is_some_and(|t| t.is_file()) {
                    println!("  {}", entry.path().display());
                    found += 1;
                }
            }
        } else {
            println!("  nothing installed");
        }
    }
    if found == 0 {
        println!("declint: nothing installed — try `declint install gh:<owner>/<repo>`");
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
            format,
            fail_on,
            fix,
            files,
        } => check(
            config.as_ref(),
            language.as_ref(),
            format,
            fail_on,
            fix,
            &files,
        ),
        Command::Presets { name } => presets(name.as_ref()),
        Command::Init { lang } => init(lang.as_ref()),
        Command::Test { config, rules } => run_tests(config.as_ref(), &rules),
        Command::Explain {
            config,
            language,
            file,
        } => explain(config.as_ref(), language.as_ref(), &file),
        Command::Install { global, list, source } => {
            if list {
                return install_list(global);
            }
            let Some(source) = source else {
                eprintln!("declint: install needs a source (gh:<owner>/<repo>) or --list");
                return ExitCode::from(2);
            };
            let parsed = match install::parse_gh_source(&source) {
                Ok(parsed) => parsed,
                Err(e) => {
                    eprintln!("declint: {e}");
                    return ExitCode::from(2);
                }
            };
            let destination = if global {
                install::Destination::Global
            } else {
                install::Destination::Project {
                    root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                }
            };
            let outcome = match install::run(&install::HttpFetcher, &parsed, &destination) {
                Ok(outcome) => outcome,
                Err(e) => {
                    eprintln!("declint: {e}");
                    return ExitCode::from(2);
                }
            };
            if outcome.floating_reference {
                eprintln!(
                    "declint: installed without a pinned ref — re-run with @<tag-or-sha> \
                     to pin"
                );
            }
            for file in &outcome.files {
                println!("vendored {}", file.display());
            }
            if global {
                println!("import with: import: [{}]", outcome.import_entry);
                return ExitCode::SUCCESS;
            }

            // Project mode: try to wire the import into the active
            // config automatically.
            match ConfigSet::discover(
                &std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            ) {
                Ok(set) if !set.configs().is_empty() => {
                    let config_path = set.configs()[0].path.clone();
                    let text = std::fs::read_to_string(&config_path).unwrap_or_default();
                    let relative = install::import_relative(&config_path, &outcome.entry_file);
                    match install::wire_import(&text, &relative) {
                        Some(updated) => {
                            if let Err(e) = std::fs::write(&config_path, updated) {
                                eprintln!("declint: cannot update {}: {e}", config_path.display());
                                return ExitCode::from(2);
                            }
                            println!("wired into {}", config_path.display());
                        }
                        None => {
                            println!(
                                "add to {}: import: [{}]",
                                config_path.display(),
                                relative
                            );
                        }
                    }
                }
                _ => {
                    println!(
                        "no config found — add to a new .declint.yaml: import: [{}]",
                        outcome.import_entry
                    );
                }
            }
            ExitCode::SUCCESS
        }
        Command::Remove { global, package } => {
            let target = if global {
                declint_core::store::global_store_dir()
                    .map(|store| store.join(&package))
            } else {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(".declint/vendor")
                    .join(&package)
                    .into()
            };
            let Some(target) = target else {
                eprintln!("declint: cannot locate a global store (no HOME directory)");
                return ExitCode::from(2);
            };
            if !target.exists() {
                eprintln!("declint: `{package}` is not installed at {}", target.display());
                return ExitCode::from(2);
            }
            match std::fs::remove_dir_all(&target) {
                Ok(()) => {
                    println!("removed {}", target.display());
                    if !global {
                        println!("remember to remove its import line from your config");
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("declint: cannot remove {}: {e}", target.display());
                    ExitCode::from(2)
                }
            }
        }
        Command::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "declint", &mut std::io::stdout());
            ExitCode::SUCCESS
        }
    }
}
