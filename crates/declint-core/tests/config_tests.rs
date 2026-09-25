//! Config loading and validation errors.

use declint_core::{Config, Severity};

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
fn rules_or_scopes_are_required() {
    assert_eq!(
        err("version: 1"),
        "config must define `rules` or `scopes` (directly or via imports)"
    );
    assert!(err("version: 1\nrules: nope").contains("`rules` must be a list"));
    assert!(err("version: 1\nscopes: nope").contains("`scopes` must be a list"));
}

#[test]
fn missing_rule_keys_are_reported() {
    let e = err("version: 1\nrules:\n  - pattern: x\n    message: m\n");
    assert!(e.contains("rule 0: missing `id` key"), "{e}");
    let e = err("version: 1\nrules:\n  - id: r\n    message: m\n");
    assert!(
        e.contains("rule 0 ('r'): rule must have a `pattern` or a `parser`"),
        "{e}"
    );
    let e = err("version: 1\nrules:\n  - id: r\n    pattern: x\n");
    assert!(
        e.contains("rule 0 ('r'): rule must have a `message`"),
        "{e}"
    );
}

#[test]
fn pattern_and_parser_are_mutually_exclusive() {
    let e = err(
        "version: 1\nrules:\n  - id: r\n    pattern: x\n    parser: p\n    message: m\n",
    );
    assert!(e.contains("mutually exclusive"), "{e}");
}

#[test]
fn parser_must_be_a_non_empty_string() {
    let e = err("version: 1\nrules:\n  - id: r\n    parser: ''\n    message: m\n");
    assert!(e.contains("`parser` must be a non-empty string"), "{e}");
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
    let dir = std::env::temp_dir().join(format!("declint-cfg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("bad.yaml");
    std::fs::write(&path, "version: 2\nrules: []").unwrap();
    let e = Config::load(&path).unwrap_err();
    assert!(e.to_string().starts_with(&path.display().to_string()), "{e}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn missing_file_is_reported() {
    let e = Config::load("/nonexistent/declint.yaml").unwrap_err();
    assert!(e.to_string().contains("cannot read config file"), "{e}");
}

const SHELL_SCOPE: &str = "\
version: 1
scopes:
  - id: sh
    start: '^```sh$'
    end: '^```$'
    rules:
      - id: no-sudo
        pattern: '\\bsudo\\b'
        message: \"no sudo\"
        severity: error
";

#[test]
fn scope_config_parses() {
    let config = Config::from_str(SHELL_SCOPE).unwrap();
    assert!(config.rules.is_empty());
    assert_eq!(config.scopes.len(), 1);
    let scope = &config.scopes[0];
    assert_eq!(scope.id, "sh");
    assert_eq!(scope.end.as_deref(), Some("^```$"));
    assert_eq!(scope.rules.len(), 1);
    assert_eq!(scope.rules[0].id, "no-sudo");
}

#[test]
fn scope_without_end_is_allowed() {
    let yaml = "version: 1\nscopes:\n  - id: s\n    start: 'X'\n    rules:\n      - id: r\n        pattern: x\n        message: m\n";
    let config = Config::from_str(yaml).unwrap();
    assert!(config.scopes[0].end.is_none());
}

#[test]
fn scope_errors() {
    // missing start
    let e = err("version: 1\nscopes:\n  - id: s\n    rules:\n      - id: r\n        pattern: x\n        message: m\n");
    assert!(e.contains("scope 0 ('s'): missing `start` key"), "{e}");

    // invalid start regex
    let e = err("version: 1\nscopes:\n  - id: s\n    start: '('\n    rules:\n      - id: r\n        pattern: x\n        message: m\n");
    assert!(e.contains("scope 0 ('s'): invalid start pattern"), "{e}");

    // unknown key
    let e = err("version: 1\nscopes:\n  - id: s\n    start: x\n    scoped: true\n    rules:\n      - id: r\n        pattern: x\n        message: m\n");
    assert!(e.contains("unknown key `scoped`"), "{e}");

    // scope with no rules
    let e = err("version: 1\nscopes:\n  - id: s\n    start: x\n");
    assert!(e.contains("scope has no `rules`"), "{e}");

    // empty scope id
    let e = err("version: 1\nscopes:\n  - id: ''\n    start: x\n    rules:\n      - id: r\n        pattern: x\n        message: m\n");
    assert!(e.contains("`id` must be a non-empty string"), "{e}");
}

#[test]
fn scope_and_rule_ids_share_one_namespace() {
    // scope id colliding with a global rule id
    let e = err("version: 1\nrules:\n  - id: dup\n    pattern: x\n    message: m\nscopes:\n  - id: dup\n    start: x\n    rules:\n      - id: r\n        pattern: x\n        message: m\n");
    assert!(e.contains("scope 0 ('dup'): duplicate id `dup`"), "{e}");

    // rule id colliding with a previous scope's rule id
    let e = err("version: 1\nscopes:\n  - id: s\n    start: x\n    rules:\n      - id: dup\n        pattern: x\n        message: m\nrules:\n  - id: dup\n    pattern: x\n    message: m\n");
    assert!(e.contains("rule 0 ('dup'): duplicate id `dup`"), "{e}");
}

#[test]
fn scoped_rule_errors_name_the_scope() {
    let e = err("version: 1\nscopes:\n  - id: sh\n    start: x\n    rules:\n      - id: r\n        pattern: '('\n        message: m\n");
    assert!(e.contains("scope 'sh' rule 0 ('r'): invalid pattern"), "{e}");
}

#[test]
fn global_rules_stay_valid_with_scopes_present() {
    let yaml = format!("{SHELL_SCOPE}\nrules:\n  - id: g\n    pattern: y\n    message: m\n");
    let config = Config::from_str(&yaml).unwrap();
    assert_eq!(config.rules.len(), 1);
    assert_eq!(config.scopes.len(), 1);
}

#[test]
fn languages_key_parses_and_dedupes() {
    let config = Config::from_str(
        "version: 1\nlanguages: [markdown, sh, markdown]\nrules:\n  - id: r\n    pattern: x\n    message: m\n",
    )
    .unwrap();
    assert_eq!(config.languages, ["markdown", "sh"]);
    assert!(config.matches_language("markdown"));
    assert!(config.matches_language("sh"));
    assert!(!config.matches_language("python"));
}

#[test]
fn no_languages_key_matches_everything() {
    let config = Config::from_str(
        "version: 1\nrules:\n  - id: r\n    pattern: x\n    message: m\n",
    )
    .unwrap();
    assert!(config.languages.is_empty());
    assert!(config.matches_language(""));
    assert!(config.matches_language("anything"));
}

#[test]
fn bad_languages_are_rejected() {
    let e = err("version: 1\nlanguages: markdown\nrules:\n  - id: r\n    pattern: x\n    message: m\n");
    assert!(e.contains("`languages` must be a list"), "{e}");
    let e = err("version: 1\nlanguages: [markdown, '']\nrules:\n  - id: r\n    pattern: x\n    message: m\n");
    assert!(e.contains("non-empty strings"), "{e}");
}
