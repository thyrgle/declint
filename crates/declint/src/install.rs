//! `declint install`: fetches rulesets from GitHub and vendors them —
//! into the project (`.declint/vendor/`, committed and reviewed) or the
//! global store (`~/.declint/store/`).
//!
//! The fetcher is a trait so tests run against a fixture instead of the
//! network. Core never fetches: `gh:` is an *install* source, not an
//! import scheme.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use declint_core::Config;

/// A parsed `gh:<owner>/<repo>[@<ref>][/subpath]` source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhSource {
    pub owner: String,
    pub repo: String,
    /// Tag, branch, or commit. `None` = floating (HEAD of the default
    /// branch).
    pub reference: Option<String>,
    /// Directory or `.yaml` file inside the repository.
    pub subpath: Option<String>,
}

/// Parses a `gh:` install source.
pub fn parse_gh_source(spec: &str) -> Result<GhSource, String> {
    let rest = spec
        .strip_prefix("gh:")
        .ok_or_else(|| format!("`{spec}` is not a gh: source"))?;
    let (owner, rest) = rest
        .split_once('/')
        .ok_or_else(|| format!("`{spec}` must be gh:<owner>/<repo>"))?;
    if owner.is_empty() {
        return Err(format!("`{spec}` is missing the owner"));
    }
    let (repo_and_ref, subpath) = match rest.split_once('/') {
        Some((repo_and_ref, sub)) => (repo_and_ref, Some(sub.to_string())),
        None => (rest, None),
    };
    let (repo, reference) = match repo_and_ref.split_once('@') {
        Some((repo, reference)) => {
            if reference.is_empty() {
                return Err(format!("`{spec}` has an empty @ref"));
            }
            (repo, Some(reference.to_string()))
        }
        None => (repo_and_ref, None),
    };
    if repo.is_empty() {
        return Err(format!("`{spec}` is missing the repository name"));
    }
    Ok(GhSource {
        owner: owner.to_string(),
        repo: repo.to_string(),
        reference,
        subpath,
    })
}

/// What fetching can fail with: `NotFound` lets entry resolution fall
/// through to the next candidate name; everything else is fatal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    NotFound,
    Other(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("not found"),
            Self::Other(message) => write!(f, "{message}"),
        }
    }
}

/// Retrieves file contents for a repository path.
pub trait Fetcher {
    fn fetch(&self, url: &str) -> Result<String, FetchError>;
}

/// The real fetcher: GitHub raw content over HTTPS.
pub struct HttpFetcher;

impl Fetcher for HttpFetcher {
    fn fetch(&self, url: &str) -> Result<String, FetchError> {
        match ureq::get(url).call() {
            Ok(response) => response
                .into_string()
                .map_err(|e| FetchError::Other(e.to_string())),
            Err(ureq::Error::Status(404, _)) => Err(FetchError::NotFound),
            Err(other) => Err(FetchError::Other(other.to_string())),
        }
    }
}

/// Where vendored files go.
#[derive(Debug, Clone)]
pub enum Destination {
    /// The project root: files vendor to `<root>/.declint/vendor/…`.
    Project { root: PathBuf },
    /// The global store (`~/.declint/store/<owner>/<repo>/…`).
    Global,
}

/// The result of a successful install.
#[derive(Debug)]
pub struct InstallOutcome {
    /// Absolute path of the vendored entry config.
    pub entry_file: PathBuf,
    /// The import entry to use, relative to the destination root.
    pub import_entry: String,
    /// Every vendored file, as absolute paths.
    pub files: Vec<PathBuf>,
    /// Printed when the source had no `@ref`.
    pub floating_reference: bool,
}

/// Normalizes a repo-relative URL path: collapses `.` and `..`, rejects
/// escapes above the repository root.
fn normalize_repo_path(directory: &str, relative: &str) -> Result<String, String> {
    if relative.starts_with('/') {
        return Err(format!("import `{relative}` is absolute"));
    }
    let joined = format!("{directory}/{relative}");
    let mut parts: Vec<&str> = Vec::new();
    for component in joined.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(format!("import `{relative}` escapes the repository"));
                }
            }
            other => parts.push(other),
        }
    }
    Ok(parts.join("/"))
}

fn sanitize_ref(reference: &str) -> String {
    let cleaned: String = reference
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned == ".." {
        "-".to_string()
    } else {
        cleaned
    }
}

fn raw_url(source: &GhSource, reference: &str, repo_path: &str) -> String {
    format!(
        "https://raw.githubusercontent.com/{}/{}/{}/{}",
        source.owner,
        source.repo,
        reference,
        repo_path.trim_start_matches('/')
    )
}

/// The candidate entry files for a source, in fallback order.
fn entry_candidates(source: &GhSource) -> Vec<String> {
    let base = source.subpath.as_deref().unwrap_or("");
    if base.ends_with(".yaml") || base.ends_with(".yml") {
        return vec![base.to_string()];
    }
    if base.is_empty() {
        vec!["declint.yaml".to_string(), ".declint.yaml".to_string()]
    } else {
        vec![
            format!("{base}/declint.yaml"),
            format!("{base}/.declint.yaml"),
        ]
    }
}

/// Fetches one repository path, trying the entry candidates in order.
fn fetch_entry(
    fetcher: &dyn Fetcher,
    source: &GhSource,
    reference: &str,
) -> Result<(String, String), String> {
    let mut attempts = Vec::new();
    for candidate in entry_candidates(source) {
        let url = raw_url(source, reference, &candidate);
        match fetcher.fetch(&url) {
            Ok(content) => return Ok((candidate, content)),
            Err(FetchError::NotFound) => attempts.push(candidate),
            Err(FetchError::Other(message)) => {
                return Err(format!(
                    "fetch failed for `{}`: {message}",
                    raw_url(source, reference, &candidate)
                ));
            }
        }
    }
    Err(format!(
        "no entry config found in `{}/{}` (tried {})",
        source.owner,
        source.repo,
        attempts
            .iter()
            .map(|a| format!("`{a}`"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Runs a full install: parse, fetch (entry + recursive relative
/// imports), vendor the tree, and report the import line to use.
pub fn run(
    fetcher: &dyn Fetcher,
    source: &GhSource,
    destination: &Destination,
) -> Result<InstallOutcome, String> {
    let reference = source.reference.clone().unwrap_or_else(|| "HEAD".into());
    let (destination_root, import_prefix) = match destination {
        Destination::Project { root } => (
            root.join(".declint")
                .join("vendor")
                .join(&source.owner)
                .join(&source.repo)
                .join(sanitize_ref(&reference)),
            ".declint/vendor".to_string(),
        ),
        Destination::Global => {
            let store = declint_core::store::global_store_dir()
                .ok_or_else(|| "cannot locate a global store (no HOME directory)".to_string())?;
            (
                store.join(&source.owner).join(&source.repo),
                String::new(),
            )
        }
    };

    let reference_for_fetch = reference.clone();
    let (entry_repo_path, entry_content) =
        fetch_entry(fetcher, source, &reference_for_fetch)?;

    let mut vendored = BTreeSet::new();
    let mut visited = BTreeSet::new();
    fetch_tree(
        fetcher,
        source,
        &reference_for_fetch,
        &entry_repo_path,
        &entry_content,
        &destination_root,
        &mut visited,
        &mut vendored,
        0,
    )?;

    let entry_file = destination_root.join(&entry_repo_path);
    let import_entry = if import_prefix.is_empty() {
        format!("global:{}/{}", source.owner, source.repo)
    } else {
        format!(
            "{import_prefix}/{}/{}/{}/{}",
            source.owner,
            source.repo,
            sanitize_ref(&reference),
            entry_repo_path
        )
    };

    // Validate the complete vendored site — relative imports exist on
    // disk now, so this exercises the same path a lint run would.
    Config::load(entry_file.clone())
        .map_err(|e| format!("vendored ruleset failed validation: {e}"))?;

    Ok(InstallOutcome {
        entry_file,
        import_entry,
        files: vendored.into_iter().collect(),
        floating_reference: source.reference.is_none(),
    })
}

/// Reads a YAML file's raw `import:` entries without full validation.
fn raw_imports(content: &str) -> Result<Vec<String>, String> {
    let value: serde_yaml::Value =
        serde_yaml::from_str(content).map_err(|e| format!("invalid YAML: {e}"))?;
    let Some(serde_yaml::Value::Sequence(entries)) = value.get("import") else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for entry in entries {
        let Some(text) = entry.as_str() else {
            return Err("`import` entries must be strings".into());
        };
        out.push(text.to_string());
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn fetch_tree(
    fetcher: &dyn Fetcher,
    source: &GhSource,
    reference: &str,
    repo_path: &str,
    content: &str,
    destination: &Path,
    visited: &mut BTreeSet<String>,
    vendored: &mut BTreeSet<PathBuf>,
    depth: usize,
) -> Result<(), String> {
    if depth > 16 {
        return Err("imports nested deeper than 16 levels".into());
    }
    if !visited.insert(repo_path.to_string()) {
        return Ok(()); // already vendored (shared fragment)
    }

    let target = destination.join(repo_path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(&target, content)
        .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
    vendored.insert(target.clone());

    // Recurse into the file's own relative imports, read straight from
    // the raw YAML (the full config validation happens after the whole
    // tree is vendored). `preset:` and `global:` references are runtime
    // concerns, not fetches.
    let directory = match repo_path.rsplit_once('/') {
        Some((dir, _)) => dir.to_string(),
        None => String::new(),
    };
    for entry in raw_imports(content)? {
        let fetchable = !entry.starts_with("preset:") && !entry.starts_with("global:");
        if !fetchable {
            continue;
        }
        let child_path = normalize_repo_path(&directory, &entry)?;
        if visited.contains(&child_path) {
            continue;
        }
        let url = raw_url(source, reference, &child_path);
        let child_content = fetcher.fetch(&url).map_err(|e| {
            format!("import '{entry}' of `{repo_path}`: fetch failed: {e}")
        })?;
        fetch_tree(
            fetcher,
            source,
            reference,
            &child_path,
            &child_content,
            destination,
            visited,
            vendored,
            depth + 1,
        )
        .map_err(|e| format!("import '{entry}' of `{repo_path}`: {e}"))?;
    }
    Ok(())
}

/// Computes the import line's path piece, relative to the config file
/// that will use it.
pub fn import_relative(config_file: &Path, entry_file: &Path) -> String {
    let config_dir = config_file.parent().unwrap_or(Path::new("."));
    let base = entry_file.strip_prefix(config_dir).unwrap_or(entry_file);
    format!(
        ".{}",
        std::path::Path::new(".")
            .join(base)
            .display()
    )
    .trim_start_matches("./.")
    .to_string()
    .replace('\\', "/")
}

/// Appends `entry` to an existing top-level `import:` list, if the raw
/// config text has one. Returns whether the edit was made.
pub fn wire_import(config_text: &str, entry: &str) -> Option<String> {
    let mut lines: Vec<String> = config_text.lines().map(str::to_string).collect();
    let mut import_line = None;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("import:") {
            import_line = Some(i);
            break;
        }
    }
    let import_line = import_line?;
    let indent = lines[import_line].len() - lines[import_line].trim_start().len();
    let mut insert_at = import_line + 1;
    while insert_at < lines.len() {
        let trimmed = lines[insert_at].trim_start();
        if trimmed.starts_with("- ") {
            insert_at += 1;
        } else {
            break;
        }
    }
    lines.insert(
        insert_at,
        format!("{}  - {}", " ".repeat(indent), entry),
    );
    let mut out = lines.join("\n");
    if config_text.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct FakeFetcher(HashMap<String, String>);

    impl Fetcher for FakeFetcher {
        fn fetch(&self, url: &str) -> Result<String, FetchError> {
            self.0
                .get(url)
                .cloned()
                .ok_or(FetchError::NotFound)
        }
    }

    fn fake_github() -> FakeFetcher {
        let base = "https://raw.githubusercontent.com/thyrgle/rules/HEAD";
        let mut files = HashMap::new();
        files.insert(
            format!("{base}/declint.yaml"),
            "version: 1\nimport:\n  - ./shared.yaml\nrules:\n  - id: main-rule\n    pattern: 'X'\n    message: m\n"
                .to_string(),
        );
        files.insert(
            format!("{base}/shared.yaml"),
            "version: 1\nrules:\n  - id: shared-rule\n    pattern: 'Y'\n    message: m\n"
                .to_string(),
        );
        // A repo whose entry config is the hidden name.
        files.insert(
            "https://raw.githubusercontent.com/thyrgle/dot/HEAD/.declint.yaml"
                .to_string(),
            "version: 1\nrules:\n  - id: dot-rule\n    pattern: 'Z'\n    message: m\n"
                .to_string(),
        );
        FakeFetcher(files)
    }

    #[test]
    fn parses_gh_sources() {
        let source = parse_gh_source("gh:thyrgle/rules@v1.2/sub").unwrap();
        assert_eq!(source.owner, "thyrgle");
        assert_eq!(source.repo, "rules");
        assert_eq!(source.reference.as_deref(), Some("v1.2"));
        assert_eq!(source.subpath.as_deref(), Some("sub"));

        let plain = parse_gh_source("gh:thyrgle/rules").unwrap();
        assert_eq!(plain.reference, None);
        assert_eq!(plain.subpath, None);

        assert!(parse_gh_source("thyrgle/rules").is_err());
        assert!(parse_gh_source("gh:thyrgle@v1").is_err());
        assert!(parse_gh_source("gh:thyrgle/rules@").is_err());
    }

    #[test]
    fn vendors_entry_and_recursive_imports() {
        let dir = std::env::temp_dir()
            .join(format!("declint-inst-{}-a", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = parse_gh_source("gh:thyrgle/rules").unwrap();
        let outcome = run(&fake_github(), &source, &Destination::Project { root: dir.clone() })
            .unwrap();

        let main = std::fs::read_to_string(dir.join(".declint/vendor/thyrgle/rules/HEAD/declint.yaml"))
            .unwrap();
        assert!(main.contains("main-rule"));
        let shared = std::fs::read_to_string(dir.join(".declint/vendor/thyrgle/rules/HEAD/shared.yaml"))
            .unwrap();
        assert!(shared.contains("shared-rule"));
        assert_eq!(outcome.files.len(), 2);
        assert_eq!(outcome.import_entry, ".declint/vendor/thyrgle/rules/HEAD/declint.yaml");
        assert!(outcome.floating_reference);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hidden_entry_name_falls_back() {
        let dir = std::env::temp_dir()
            .join(format!("declint-inst-{}-b", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = parse_gh_source("gh:thyrgle/dot").unwrap();
        let outcome = run(&fake_github(), &source, &Destination::Project { root: dir.clone() })
            .unwrap();
        let entry = std::fs::read_to_string(outcome.entry_file).unwrap();
        assert!(entry.contains("dot-rule"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_entry_lists_candidates() {
        let dir = std::env::temp_dir()
            .join(format!("declint-inst-{}-c", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = parse_gh_source("gh:nobody/nothing").unwrap();
        let e = run(&FakeFetcher(HashMap::new()), &source, &Destination::Project { root: dir.clone() })
            .unwrap_err();
        assert!(e.contains("no entry config found"), "{e}");
        assert!(e.contains("declint.yaml"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn global_destination_uses_the_store() {
        // DECLINT_HOME is read at resolve time; keep the window small.
        let dir = std::env::temp_dir()
            .join(format!("declint-inst-{}-g", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("DECLINT_HOME", &dir);
        let source = parse_gh_source("gh:thyrgle/rules").unwrap();
        let outcome = run(&fake_github(), &source, &Destination::Global).unwrap();
        assert_eq!(outcome.import_entry, "global:thyrgle/rules");
        let entry = std::fs::read_to_string(outcome.entry_file.clone()).unwrap();
        assert!(entry.contains("main-rule"));
        // And a config can now resolve it through the store.
        let config = Config::from_str(
            "version: 1\nimport:\n  - global:thyrgle/rules\n",
        )
        .unwrap();
        assert!(config.rules.iter().any(|r| r.id == "main-rule"));
        std::env::remove_var("DECLINT_HOME");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn wire_import_appends_to_existing_list() {
        let original = "version: 1\nimport:\n  - preset:ini\nrules: []\n";
        let wired = wire_import(original, ".declint/vendor/x/declint.yaml").unwrap();
        assert!(wired.contains("  - preset:ini\n  - .declint/vendor/x/declint.yaml\n"), "{wired}");
        assert!(wired.ends_with("rules: []\n"));

        // No import key: no edit.
        assert!(wire_import("version: 1\nrules: []\n", "x.yaml").is_none());
    }
}
