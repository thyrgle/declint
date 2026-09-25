//! Callback resolution and the decision pipeline, with fake callbacks.

use std::sync::Arc;

use declint_core::{Callbacks, CallbackRef, Config, Decision, Linter, MatchCallback, MatchContext, Severity};

struct Fixed(Decision);

impl MatchCallback for Fixed {
    fn evaluate(&self, _ctx: &MatchContext) -> Result<Decision, String> {
        Ok(self.0.clone())
    }
}

struct Echo;

impl MatchCallback for Echo {
    fn evaluate(&self, ctx: &MatchContext) -> Result<Decision, String> {
        Ok(Decision::Violate {
            severity: Some(Severity::Hint),
            message: format!(
                "{}|{}|{}|{}|{}:{}|{}|{}",
                ctx.match_text,
                ctx.path,
                ctx.language,
                ctx.rule_id,
                ctx.line,
                ctx.col,
                ctx.capture("word").unwrap_or(""),
                ctx.capture("1").unwrap_or(""),
            ),
        })
    }
}

fn yaml(rule_body: &str) -> String {
    format!("version: 1\nrules:\n  - id: probe\n{rule_body}\n")
}

const PROBE: &str = "    pattern: 'TODO(?<word>\\w*)'\n";

#[test]
fn callback_reference_classification() {
    assert_eq!(
        CallbackRef::parse("checks/foo.lua"),
        CallbackRef::File { path: "checks/foo.lua".into() }
    );
    assert_eq!(
        CallbackRef::parse("foo.lua"),
        CallbackRef::File { path: "foo.lua".into() }
    );
    assert_eq!(
        CallbackRef::parse("my_name"),
        CallbackRef::Name("my_name".into())
    );
    assert!(matches!(
        CallbackRef::parse("function(c) return nil end"),
        CallbackRef::Inline { .. }
    ));
    assert!(matches!(
        CallbackRef::parse("function(c)\n  return nil\nend"),
        CallbackRef::Inline { .. }
    ));
}

#[test]
fn unregistered_callback_is_a_build_error() {
    let config = Config::from_str(&yaml(&format!("{PROBE}    callback: nope\n"))).unwrap();
    let e = Linter::new(config, &Callbacks::new()).unwrap_err();
    assert!(
        e.to_string().contains("rule 'probe' references callback 'nope'"),
        "{e}"
    );
}

#[test]
fn unregistered_scoped_callback_names_the_scope() {
    let yaml = "\
version: 1
scopes:
  - id: sh
    start: 'X'
    rules:
      - id: probe
        pattern: 'TODO'
        callback: nope
";
    let config = Config::from_str(yaml).unwrap();
    let e = Linter::new(config, &Callbacks::new()).unwrap_err();
    assert!(e.to_string().contains("scope 'sh' rule 'probe'"), "{e}");
}

#[test]
fn allow_decision_suppresses_the_match() {
    let mut callbacks = Callbacks::new();
    callbacks.register("permit", Arc::new(Fixed(Decision::Allow)));
    let config = Config::from_str(&yaml(&format!("{PROBE}    callback: permit\n"))).unwrap();
    let linter = Linter::new(config, &callbacks).unwrap();
    assert!(linter.lint("TODO fix").is_empty());
}

#[test]
fn violate_decision_overrides_message_and_severity() {
    let mut callbacks = Callbacks::new();
    callbacks.register("echo", Arc::new(Echo));
    let config = Config::from_str(&yaml(&format!(
        "{PROBE}    callback: echo\n    message: fallback\n    severity: warning\n"
    )))
    .unwrap();
    let linter = Linter::new(config, &callbacks).unwrap();

    let text = "x\nTODOhere x";
    let v = linter.lint_all_in(
        declint_core::DocInfo { path: "a.txt", language: "text" },
        text,
    );
    assert_eq!(v.len(), 1);
    // "<match>|<path>|<language>|<rule>|<line>:<col>|<named>|<numbered>"
    // The only group is named `word`, so the numbered key is absent.
    assert_eq!(
        v[0].message,
        "TODOhere|a.txt|text|probe|2:1|here|"
    );
    assert_eq!(v[0].severity, Severity::Hint);
    assert_eq!(v[0].span.to_range(), 2..10);
}

#[test]
fn violate_default_renders_the_rule_template() {
    let mut callbacks = Callbacks::new();
    callbacks.register("flag", Arc::new(Fixed(Decision::ViolateDefault)));
    let config = Config::from_str(&yaml(&format!(
        "{PROBE}    callback: flag\n    message: \"found '{{match}}'\"\n"
    )))
    .unwrap();
    let linter = Linter::new(config, &callbacks).unwrap();
    let v = linter.lint("ab TODOcd");
    assert_eq!(v[0].message, "found 'TODOcd'");
}

#[test]
fn callback_error_becomes_an_error_diagnostic() {
    struct Boom;
    impl MatchCallback for Boom {
        fn evaluate(&self, _: &MatchContext) -> Result<Decision, String> {
            Err("exploded".into())
        }
    }
    let mut callbacks = Callbacks::new();
    callbacks.register("boom", Arc::new(Boom));
    let config = Config::from_str(&yaml(&format!("{PROBE}    callback: boom\n"))).unwrap();
    let linter = Linter::new(config, &callbacks).unwrap();
    let v = linter.lint("TODO");
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].severity, Severity::Error);
    assert!(v[0].message.contains("callback error: exploded"), "{}", v[0].message);
}

#[test]
fn message_is_optional_with_a_callback() {
    let config =
        Config::from_str(&yaml(&format!("{PROBE}    callback: permit\n"))).unwrap();
    let mut callbacks = Callbacks::new();
    callbacks.register("permit", Arc::new(Fixed(Decision::Allow)));
    assert!(Linter::new(config, &callbacks).is_ok());
}

#[test]
fn violates_without_message_template_are_flagged() {
    let mut callbacks = Callbacks::new();
    callbacks.register("flag", Arc::new(Fixed(Decision::ViolateDefault)));
    let config = Config::from_str(&yaml(&format!("{PROBE}    callback: flag\n"))).unwrap();
    let linter = Linter::new(config, &callbacks).unwrap();
    let v = linter.lint("TODO");
    assert_eq!(v[0].severity, Severity::Error);
    assert!(v[0].message.contains("without a default message"), "{}", v[0].message);
}

#[test]
fn scoped_rule_callbacks_receive_scope_context() {
    let mut callbacks = Callbacks::new();
    callbacks.register("echo", Arc::new(Echo));
    let yaml = "\
version: 1
scopes:
  - id: sh
    start: '^X$'
    rules:
      - id: probe
        pattern: 'TODO'
        callback: echo
";
    let config = Config::from_str(yaml).unwrap();
    let linter = Linter::new(config, &callbacks).unwrap();
    let v = linter.lint_all_in(
        declint_core::DocInfo { path: "p.sh", language: "sh" },
        "X\nTODO now\n",
    );
    assert_eq!(v.len(), 1);
    assert!(
        v[0].message.starts_with("TODO|p.sh|sh|probe|"),
        "{}",
        v[0].message
    );
}

// ---- parser rules ----

use declint_core::{MatchParser, RawMatch};

/// Scans for `key = value` repeats and reports each key defined more
/// than once — the canonical parser-rule demo.
struct DuplicateKeys;

impl MatchParser for DuplicateKeys {
    fn find(&self, text: &str, _offset: usize) -> Result<Vec<RawMatch>, String> {
        let mut seen: Vec<(&str, usize)> = Vec::new();
        let mut out = Vec::new();
        for (line_no, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            let Some(eq) = trimmed.find('=') else { continue };
            let key = trimmed[..eq].trim();
            if let Some((_, first)) = seen.iter_mut().find(|(k, _)| *k == key) {
                let start = line_no + 1; // fake a plausible offset
                let _ = first;
                let _ = start;
                out.push(
                    RawMatch::new(0, 1)
                        .with_capture("key", key)
                        .with_capture("count", "2"),
                );
            } else {
                seen.push((key, line_no));
            }
        }
        Ok(out)
    }
}

#[test]
fn parser_rule_fires_on_returned_matches() {
    let mut callbacks = Callbacks::new();
    callbacks.register_parser("dup", Arc::new(DuplicateKeys));
    let config = Config::from_str(
        "version: 1\nrules:\n  - id: dup-keys\n    parser: dup\n    message: \"'{key}' defined {count} times\"\n    severity: error\n",
    )
    .unwrap();
    let linter = Linter::new(config, &callbacks).unwrap();
    let v = linter.lint("a = 1\nb = 2\na = 3\n");
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].rule_id, "dup-keys");
    assert_eq!(v[0].message, "'a' defined 2 times");
    assert_eq!(v[0].severity, Severity::Error);
}

#[test]
fn parser_error_becomes_an_error_diagnostic() {
    struct Boom;
    impl MatchParser for Boom {
        fn find(&self, _: &str, _: usize) -> Result<Vec<RawMatch>, String> {
            Err("exploded".into())
        }
    }
    let mut callbacks = Callbacks::new();
    callbacks.register_parser("boom", Arc::new(Boom));
    let config = Config::from_str(
        "version: 1\nrules:\n  - id: probe\n    parser: boom\n    message: m\n",
    )
    .unwrap();
    let linter = Linter::new(config, &callbacks).unwrap();
    let v = linter.lint("anything");
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].severity, Severity::Error);
    assert!(
        v[0].message.contains("parser error: exploded"),
        "{}",
        v[0].message
    );
}

#[test]
fn unregistered_parser_is_a_build_error() {
    let config = Config::from_str(
        "version: 1\nrules:\n  - id: probe\n    parser: nope\n    message: m\n",
    )
    .unwrap();
    let e = Linter::new(config, &Callbacks::new()).unwrap_err();
    assert!(
        e.to_string()
            .contains("rule 'probe' references callback 'nope' which is not registered"),
        "{e}"
    );
}

#[test]
fn parser_rule_in_scope_gets_region_text() {
    struct RegionSpy;

    impl MatchParser for RegionSpy {
        fn find(&self, text: &str, offset: usize) -> Result<Vec<RawMatch>, String> {
            Ok(vec![RawMatch::new(0, 1).with_capture("seen", text).with_capture("off", offset.to_string())])
        }
    }
    let mut callbacks = Callbacks::new();
    callbacks.register_parser("spy", Arc::new(RegionSpy));
    let yaml = "\
version: 1
scopes:
  - id: sh
    start: '^X$'
    rules:
      - id: probe
        parser: spy
        message: 'region {seen} at {off}'
";
    let config = Config::from_str(yaml).unwrap();
    let linter = Linter::new(config, &callbacks).unwrap();
    let v = linter.lint_all("X\nbody\n");
    assert_eq!(v.len(), 1);
    // The region starts at the `start` match (byte 0 here), so the
    // parser saw the whole region with offset 0.
    assert_eq!(v[0].message, "region X\nbody\n at 0");
}
