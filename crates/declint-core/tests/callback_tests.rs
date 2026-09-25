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
