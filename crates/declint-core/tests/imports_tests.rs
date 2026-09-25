//! The `import:` mechanism: preset and file imports, cycles, collisions.

use std::path::Path;

use declint_core::{Callbacks, Config, ConfigSet, Linter};

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn project(tag: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("declint-imports-{}-{n}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

use std::path::PathBuf;

fn linter_for(dir: &Path, config: &str) -> Linter {
    write(&dir.join(".declint.yaml"), config);
    let set = ConfigSet::discover(dir).unwrap();
    Linter::new(set.configs()[0].config.clone(), &Callbacks::new()).unwrap()
}

#[test]
fn preset_import_brings_its_rules() {
    // The ini preset contains a Lua callback rule, so this asserts at
    // the config level — a lua-enabled build would also lint with it
    // (see declint-lua's tests).
    let dir = project("preset");
    write(
        &dir.join(".declint.yaml"),
        "version: 1\nimport:\n  - preset:ini\nlanguages: [ini]\n",
    );
    let set = ConfigSet::discover(&dir).unwrap();
    let ids: Vec<&str> = set.configs()[0]
        .config
        .rules
        .iter()
        .map(|r| r.id.as_str())
        .collect();
    assert!(ids.contains(&"no-tabs"));
    assert!(ids.contains(&"trailing-whitespace"));
    assert!(ids.contains(&"empty-value"));
    let scope_ids: Vec<&str> =
        set.configs()[0].config.scopes.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(scope_ids, ["server"]);
}

#[test]
fn own_rules_merge_after_imports() {
    let dir = project("merge");
    let linter = linter_for(
        &dir,
        "\
version: 1
import:
  - preset:markdown
rules:
  - id: shouty
    pattern: '(?m)^[A-Z]{5,}!$'
    message: 'no shouting'
    severity: warning
",
    );
    let v = linter.lint("SHOUT!\n\ttabs\n");
    let ids: Vec<&str> = v.iter().map(|x| x.rule_id.as_str()).collect();
    assert!(ids.contains(&"shouty"), "{v:?}");
    assert!(ids.contains(&"no-tabs"), "{v:?}");
}

#[test]
fn relative_imports_resolve_against_the_importing_file() {
    let dir = project("relative");
    write(
        &dir.join("shared").join("team.yaml"),
        "version: 1\nrules:\n  - id: team-rule\n    pattern: 'TEAM'\n    message: team\n",
    );
    let linter = linter_for(
        &dir,
        "version: 1\nimport:\n  - shared/team.yaml\nrules:\n  - id: own\n    pattern: x\n    message: m\n",
    );
    let v = linter.lint("TEAM x");
    assert_eq!(v.len(), 2);
}

#[test]
fn missing_import_file_is_an_error() {
    let dir = project("missing");
    write(
        &dir.join(".declint.yaml"),
        "version: 1\nimport:\n  - ./nope.yaml\nrules:\n  - id: r\n    pattern: x\n    message: m\n",
    );
    let e = Config::load(dir.join(".declint.yaml")).unwrap_err();
    assert!(e.to_string().contains("cannot read imported config"), "{e}");
}

#[test]
fn non_preset_schemes_need_file_loading() {
    // A URL is just a `.yaml` path as far as classification goes: from
    // memory it is rejected for having no base directory, and from a
    // file it fails the read (remote fetches are not supported).
    let e = Config::from_str(
        "version: 1\nimport:\n  - http://evil.example/x.yaml\nrules:\n  - id: r\n    pattern: x\n    message: m\n",
    )
    .unwrap_err();
    assert!(
        e.to_string().contains("requires loading the config from a file"),
        "{e}"
    );

    let dir = project("scheme");
    write(
        &dir.join(".declint.yaml"),
        "version: 1\nimport:\n  - http://evil.example/x.yaml\nrules:\n  - id: r\n    pattern: x\n    message: m\n",
    );
    let e = Config::load(dir.join(".declint.yaml")).unwrap_err();
    assert!(e.to_string().contains("cannot read imported config"), "{e}");
}

#[test]
fn unknown_preset_lists_the_library() {
    let e = Config::from_str(
        "version: 1\nimport:\n  - preset:fortran\nrules:\n  - id: r\n    pattern: x\n    message: m\n",
    )
    .unwrap_err();
    let text = e.to_string();
    assert!(text.contains("unknown preset `fortran`"), "{text}");
    assert!(text.contains("python, ini, markdown"), "{text}");
}

#[test]
fn import_cycles_are_detected() {
    let dir = project("cycle");
    write(
        &dir.join("a.yaml"),
        "version: 1\nimport:\n  - b.yaml\nrules:\n  - id: ra\n    pattern: x\n    message: m\n",
    );
    write(
        &dir.join("b.yaml"),
        "version: 1\nimport:\n  - a.yaml\nrules:\n  - id: rb\n    pattern: y\n    message: m\n",
    );
    write(
        &dir.join(".declint.yaml"),
        "version: 1\nimport:\n  - a.yaml\nrules:\n  - id: r\n    pattern: z\n    message: m\n",
    );
    let e = Config::load(dir.join(".declint.yaml")).unwrap_err();
    assert!(e.to_string().contains("import cycle"), "{e}");
}

#[test]
fn id_collisions_across_imports_are_errors() {
    let dir = project("collision");
    write(
        &dir.join("one.yaml"),
        "version: 1\nrules:\n  - id: shared\n    pattern: x\n    message: one\n",
    );
    write(
        &dir.join("two.yaml"),
        "version: 1\nrules:\n  - id: shared\n    pattern: y\n    message: two\n",
    );
    write(
        &dir.join(".declint.yaml"),
        "version: 1\nimport:\n  - one.yaml\n  - two.yaml\nrules:\n  - id: own\n    pattern: z\n    message: m\n",
    );
    let e = Config::load(dir.join(".declint.yaml")).unwrap_err();
    assert!(
        e.to_string()
            .contains("duplicate rule id `shared` (defined in"),
        "{e}"
    );
}

#[test]
fn imported_configs_cannot_declare_languages() {
    let dir = project("langs");
    write(
        &dir.join("frag.yaml"),
        "version: 1\nlanguages: [python]\nrules:\n  - id: r\n    pattern: x\n    message: m\n",
    );
    write(
        &dir.join(".declint.yaml"),
        "version: 1\nimport:\n  - frag.yaml\nrules:\n  - id: own\n    pattern: y\n    message: m\n",
    );
    let e = Config::load(dir.join(".declint.yaml")).unwrap_err();
    assert!(
        e.to_string()
            .contains("imported config declares `languages`"),
        "{e}"
    );
}

#[test]
fn relative_imports_require_file_loading() {
    let e = Config::from_str(
        "version: 1\nimport:\n  - ./team.yaml\nrules:\n  - id: r\n    pattern: x\n    message: m\n",
    )
    .unwrap_err();
    assert!(e.to_string().contains("requires loading the config from a file"), "{e}");
}

#[test]
fn preset_imports_work_without_a_file() {
    let config =
        Config::from_str("version: 1\nimport:\n  - preset:markdown\n").unwrap();
    assert!(config.rules.iter().any(|r| r.id == "bare-url"));
}

#[test]
fn ini_preset_patterns_work_on_crlf_files() {
    // The preset's trailing-whitespace rule must be CRLF-tolerant.
    let ini = declint_core::presets::lookup("ini").unwrap().content;
    assert!(
        ini.contains(r"[ \t]+\r?$"),
        "preset trailing-whitespace must tolerate \\r before the newline"
    );

    // The pattern shapes themselves, against a CRLF file: a
    // tab-indented line, trailing spaces before \r, a section header.
    let dir = project("crlf");
    let linter = linter_for(
        &dir,
        "version: 1\nrules:\n  - id: no-tabs\n    pattern: '(?m)^\\t+'\n    message: t\n  - id: trailing-whitespace\n    pattern: '(?m)[ \\t]+\\r?$'\n    message: w\nscopes:\n  - id: server\n    start: '^\\[server\\]$'\n    end: '^\\['\n    rules:\n      - id: in-server\n        pattern: '(?m)port'\n        message: p\n",
    );
    let source = "[server]\r\n\tport = 8000\r\nname = x   \r\n";
    let v = linter.lint_all_in(
        declint_core::DocInfo { path: "a.ini", language: "ini" },
        source,
    );
    let ids: Vec<&str> = v.iter().map(|x| x.rule_id.as_str()).collect();
    assert!(ids.contains(&"no-tabs"), "{v:?}");
    assert!(
        ids.contains(&"trailing-whitespace"),
        "trailing whitespace before \\r must be found: {v:?}"
    );
    assert!(ids.contains(&"in-server"), "scope segmentation on CRLF: {v:?}");
}
