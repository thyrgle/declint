//! Config sites: hidden file, hidden directory, and discovery.

use std::path::Path;

use declint_core::{language_from_extension, ConfigSet, CONFIG_FILE};

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

const RULES: &str = "\
version: 1
rules:
  - id: r1
    pattern: x
    message: m
";

#[test]
fn loads_hidden_file() {
    let dir = std::env::temp_dir().join(format!("declint-cs-{}-file", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write(&dir.join(CONFIG_FILE), RULES);
    let set = ConfigSet::discover(&dir).unwrap();
    assert_eq!(set.configs().len(), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn loads_directory_of_configs_sorted() {
    let dir = std::env::temp_dir().join(format!("declint-cs-{}-dir", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let declint = dir.join(".declint");
    write(
        &declint.join("sh.yaml"),
        "version: 1\nlanguages: [sh]\nrules:\n  - id: dup\n    pattern: sudo\n    message: m\n",
    );
    write(
        &declint.join("markdown.yaml"),
        "version: 1\nlanguages: [markdown]\nrules:\n  - id: dup\n    pattern: todo\n    message: m\n",
    );
    let set = ConfigSet::discover(&dir).unwrap();
    assert_eq!(set.configs().len(), 2);
    // Sorted by file name: markdown.yaml first.
    assert!(set.configs()[0]
        .path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("markdown"));
    // Per-file id namespaces: both files may define `dup`.
    assert_eq!(set.configs()[0].config.rules[0].id, "dup");
    assert_eq!(set.configs()[1].config.rules[0].id, "dup");
    // Language matching works per file.
    assert!(set.configs()[0].config.matches_language("markdown"));
    assert!(!set.configs()[0].config.matches_language("sh"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn empty_config_directory_is_an_error() {
    let dir = std::env::temp_dir().join(format!("declint-cs-{}-empty", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(".declint")).unwrap();
    let e = ConfigSet::discover(&dir).unwrap_err();
    assert!(e.to_string().contains("no `.yaml`"), "{e}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn discovery_walks_up_and_prefers_hidden() {
    let root = std::env::temp_dir().join(format!("declint-cs-{}-walk", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    // Hidden file wins over the legacy file in the same directory...
    write(&root.join(CONFIG_FILE), RULES);
    write(
        &root.join("declint.yaml"),
        "version: 1\nrules:\n  - id: r\n    pattern: x\n    message: m\n",
    );
    // ...and a subdir with no config of its own finds the parent's.
    let sub = root.join("a").join("b");
    std::fs::create_dir_all(&sub).unwrap();
    let set = ConfigSet::discover(&sub).unwrap();
    assert_eq!(set.configs()[0].config.rules.len(), 1);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn legacy_file_beats_parent_directory_configs() {
    let root = std::env::temp_dir().join(format!("declint-cs-{}-legacy", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    write(&root.join(CONFIG_FILE), RULES);
    let sub = root.join("legacy-dir");
    write(
        &sub.join("declint.yaml"),
        "version: 1\nrules:\n  - id: r\n    pattern: x\n    message: m\n",
    );
    // The nearer legacy file is the config site for its directory.
    let set = ConfigSet::discover(&sub).unwrap();
    assert_eq!(set.configs()[0].config.rules.len(), 1);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn discovery_nothing_found_is_an_error() {
    let dir = std::env::temp_dir().join(format!("declint-cs-{}-none", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let e = ConfigSet::discover(&dir).unwrap_err();
    assert!(e.to_string().contains("no declint config found"), "{e}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn scope_table_maps_global_to_local() {
    let dir = std::env::temp_dir().join(format!("declint-cs-{}-scopes", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let declint = dir.join(".declint");
    for (name, scope_id) in [("a.yaml", "s1"), ("b.yaml", "s2")] {
        write(
            &declint.join(name),
            &format!(
                "version: 1\nscopes:\n  - id: {scope_id}\n    start: 'X'\n    rules:\n      - id: r-{scope_id}\n        pattern: p\n        message: m\n"
            ),
        );
    }
    let set = ConfigSet::discover(&dir).unwrap();
    let table = set.scope_table();
    assert_eq!(table.len(), 2);
    assert_eq!(table[0].scope.id, "s1");
    assert_eq!((table[0].config, table[0].local), (0, 0));
    assert_eq!(table[1].scope.id, "s2");
    assert_eq!((table[1].config, table[1].local), (1, 0));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn extension_table_names_neovim_filetypes() {
    assert_eq!(
        language_from_extension(Path::new("a/b.md")).as_deref(),
        Some("markdown")
    );
    assert_eq!(language_from_extension(Path::new("x.zzz")), None);
}
