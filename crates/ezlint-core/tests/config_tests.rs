//! Config loading and validation errors.

use ezlint_core::{Config, Severity};

fn err(yaml: &str) -> String {
    Config::from_str(yaml).unwrap_err().to_string()
}

const GOOD: &str = "\
version: 1
rules:
  - id: no-tabs
    pattern: '\\t+'
    message: \"Use spaces, found '{match}'\"
    severity: warning
";

#[test]
fn good_config_parses() {
    let config = Config::from_str(GOOD).unwrap();
    assert_eq!(config.version, 1);
    assert_eq!(config.rules.len(), 1);
    let rule = &config.rules[0];
    assert_eq!(rule.id, "no-tabs");
    assert_eq!(rule.pattern, "\\t+");
    assert_eq!(rule.severity, Severity::Warning);
}

#[test]
fn severity_defaults_to_warning() {
    let config = Config::from_str("version: 1\nrules:\n  - id: r\n    pattern: x\n    message: m\n")
        .unwrap();
    assert_eq!(config.rules[0].severity, Severity::Warning);
}

#[test]
fn version_is_required_and_pinned() {
    assert_eq!(err("rules: []"), "missing `version` key");
    assert!(err("version: 2\nrules: []").contains("unsupported config version 2"));
    assert!(err("version: two\nrules: []").contains("`version` must be an integer"));
}

#[test]
fn rules_are_required() {
    assert_eq!(err("version: 1"), "missing `rules` key");
    assert!(err("version: 1\nrules: nope").contains("`rules` must be a list"));
}

#[test]
fn missing_rule_keys_are_reported() {
    let e = err("version: 1\nrules:\n  - pattern: x\n    message: m\n");
    assert!(e.contains("rule 0: missing `id` key"), "{e}");
    let e = err("version: 1\nrules:\n  - id: r\n    message: m\n");
    assert!(e.contains("missing `pattern` key"), "{e}");
    let e = err("version: 1\nrules:\n  - id: r\n    pattern: x\n");
    assert!(e.contains("missing `message` key"), "{e}");
}

#[test]
fn duplicate_ids_are_rejected() {
    let e = err(
        "version: 1\nrules:\n  - id: dup\n    pattern: x\n    message: m\n  - id: dup\n    \
         pattern: y\n    message: m\n",
    );
    assert!(e.contains("duplicate id `dup`"), "{e}");
}

#[test]
fn invalid_regex_reports_the_rule() {
    let e = err("version: 1\nrules:\n  - id: bad\n    pattern: '('\n    message: m\n");
    assert!(e.contains("rule 0 ('bad'): invalid pattern"), "{e}");
}

#[test]
fn unknown_template_placeholder_is_rejected() {
    let e = err(
        "version: 1\nrules:\n  - id: r\n    pattern: '(?<word>\\\\w+)'\n    message: '{nope}'\n",
    );
    assert!(e.contains("does not name a capture group"), "{e}");
    assert!(e.contains("known groups: word"), "{e}");
}

#[test]
fn unknown_keys_are_rejected() {
    let e = err("version: 1\nrules:\n  - id: r\n    pattern: x\n    message: m\n    sevrity: hi\n");
    assert!(e.contains("unknown key `sevrity`"), "{e}");
}

#[test]
fn bad_severity_is_rejected() {
    let e = err(
        "version: 1\nrules:\n  - id: r\n    pattern: x\n    message: m\n    severity: fatal\n",
    );
    assert!(e.contains("`severity` must be one of"), "{e}");
}

#[test]
fn errors_carry_line_numbers() {
    let e = Config::from_str(GOOD.replace("severity: warning", "severity: fatal").as_str())
        .unwrap_err();
    let line = e.line().expect("severity line known");
    assert!((2..=6).contains(&line), "got line {line}");
}

#[test]
fn load_attaches_path() {
    let dir = std::env::temp_dir().join(format!("ezlint-cfg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bad.yaml");
    std::fs::write(&path, "version: 2\nrules: []").unwrap();
    let e = Config::load(&path).unwrap_err();
    assert!(e.to_string().starts_with(&path.display().to_string()), "{e}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn missing_file_is_reported() {
    let e = Config::load("/nonexistent/ezlint.yaml").unwrap_err();
    assert!(e.to_string().contains("cannot read config file"), "{e}");
}
