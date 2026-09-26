//! Inline suppressions and fix templates.

use declint_core::{Callbacks, Config, Linter};

fn linter_with(yaml: &str) -> Linter {
    let config = Config::from_str(yaml).unwrap();
    Linter::new(config, &Callbacks::new()).unwrap()
}

fn yaml(rule_body: &str) -> String {
    format!("version: 1\nrules:\n  - id: probe\n{rule_body}\n")
}

#[test]
fn bare_marker_suppresses_on_its_own_line() {
    let linter = linter_with(&yaml(
        "    pattern: 'X'\n    message: found\n",
    ));
    let source = "X\nX # declint:disable\nX\n";
    let v = linter.lint(source);
    assert_eq!(v.len(), 2, "{v:?}");
    assert_eq!(v[0].span.to_range(), 0..1);
    assert_eq!(v[1].span.to_range(), 22..23);
}

#[test]
fn next_line_marker_suppresses_the_following_line() {
    let linter = linter_with(&yaml(
        "    pattern: 'X'\n    message: found\n",
    ));
    let source = "X\n# declint:disable-next-line\nX\nX\n";
    let v = linter.lint(source);
    assert_eq!(v.len(), 2, "{v:?}");
    // The second and third Xs are on the suppressed line? No: the
    // marker is on line 2, so line 3 is suppressed — the first and
    // last Xs survive.
    let starts: Vec<usize> = v.iter().map(|x| x.span.start).collect();
    assert_eq!(starts, [0, source.len() - 2]);
}

#[test]
fn id_list_suppresses_only_named_rules() {
    let yaml = "\
version: 1
rules:
  - id: a
    pattern: 'X'
    message: hit
  - id: b
    pattern: 'X'
    message: hit
";
    let linter = linter_with(yaml);
    let source = "X # declint:disable=a\n";
    let v = linter.lint(source);
    let ids: Vec<&str> = v.iter().map(|x| x.rule_id.as_str()).collect();
    assert_eq!(ids, ["b"]);
}

#[test]
fn suppression_without_a_list_covers_every_rule() {
    let yaml = "\
version: 1
rules:
  - id: a
    pattern: 'X'
    message: hit
  - id: b
    pattern: 'X'
    message: hit
";
    let linter = linter_with(yaml);
    let source = "# declint:disable-next-line\nX\n";
    let v = linter.lint(source);
    assert_eq!(v.len(), 0, "every rule is suppressed on the next line");
}

#[test]
fn fixes_interpolate_numbered_captures() {
    let linter = linter_with(
        "version: 1\nrules:\n  - id: fixme\n    pattern: '(?m)^(\\w+) ='\n    message: m\n    fix: '{1}_x ='\n",
    );
    let v = linter.lint("foo = 1");
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].fix.as_deref(), Some("foo_x ="));
}

#[test]
fn fixes_interpolate_named_captures() {
    let linter = linter_with(
        "version: 1\nrules:\n  - id: fixme\n    pattern: '(?m)^(?<key>\\w+) ='\n    message: m\n    fix: '{key}_x ='\n",
    );
    let v = linter.lint("foo = 1");
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].fix.as_deref(), Some("foo_x ="));
}

#[test]
fn rules_without_fix_have_none() {
    let linter = linter_with(&yaml("    pattern: 'X'\n    message: found\n"));
    let v = linter.lint("X");
    assert_eq!(v[0].fix, None);
}
