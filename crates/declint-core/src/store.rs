//! The global ruleset store: where `declint install -g` vendors
//! packages, and where `import: global:<pkg>` finds them.

use std::path::PathBuf;

/// The global store directory, or `None` when no home can be determined.
///
/// `$DECLINT_HOME` overrides the root (store lives at `$DECLINT_HOME/store`);
/// otherwise the store is `$HOME/.declint/store`.
pub fn global_store_dir() -> Option<PathBuf> {
    if let Ok(root) = std::env::var("DECLINT_HOME") {
        if !root.is_empty() {
            return Some(PathBuf::from(root).join("store"));
        }
    }
    let home = std::env::var("HOME").ok()?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(".declint").join("store"))
}

/// Resolves a `global:<pkg>` reference to its entry config file.
///
/// `pkg` is `<owner>/<repo>` as printed by `declint install -g`; a bare
/// repo name is accepted too (resolved directly under the store). The
/// entry file is `declint.yaml`.
pub fn lookup_global(pkg: &str) -> Option<PathBuf> {
    let store = global_store_dir()?;
    let sanitized = pkg.replace(['\\', ':'], "-");
    let entry = store.join(&sanitized).join("declint.yaml");
    if entry.is_file() {
        Some(entry)
    } else {
        None
    }
}
