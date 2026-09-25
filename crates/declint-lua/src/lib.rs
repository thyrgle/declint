//! Lua match callbacks for declint.
//!
//! A rule's `callback:` value can be an **inline Lua snippet** (a block
//! scalar) or a **file path** (`checks/foo.lua`, relative to the config
//! file). [`attach`] compiles every such reference, registers it in the
//! callback registry, and from then on the linter calls it per match.
//!
//! # Contract
//!
//! The snippet is a Lua chunk that **returns** the callback function
//! (helpers and locals above the `return` just work):
//!
//! ```lua
//! return function(ctx)
//!   -- ctx.match      the matched text
//!   -- ctx.captures   { name = "..." } / { ["1"] = "..." }
//!   -- ctx.start      absolute byte offset
//!   -- ctx.finish     byte offset past the match
//!   -- ctx.line, ctx.col   1-based position of the match
//!   -- ctx.path, ctx.language, ctx.rule
//! end
//! ```
//!
//! Return values:
//!
//! * `nil` / `false` — **allow**: no diagnostic for this match;
//! * `true` — violate with the rule's own `message` template;
//! * `{ message = "...", severity = "error" }` — violate with overrides
//!   (`severity` optional, falls back to the rule's severity);
//! * a thrown Lua error — surfaced as an `error`-severity diagnostic
//!   naming the rule; the lint run itself is never affected.
//!
//! Each call runs under an instruction budget, so a runaway loop fails
//! instead of hanging the editor.
//!
//! # Examples
//!
//! ```
//! use declint_core::{CallbackRef, Callbacks, ConfigSet};
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let dir = std::env::temp_dir().join("declint-lua-doc");
//! # std::fs::create_dir_all(&dir)?;
//! #     std::fs::write(dir.join(".declint.yaml"),
//! #     "version: 1\nrules:\n  - id: r\n    pattern: 'x'\n    callback: |\n      return function(c) return { message = \"seen \" .. c.match } end\n")?;
//! let set = ConfigSet::discover(&dir)?;
//! let mut callbacks = Callbacks::new();
//! declint_lua::attach(&set, &mut callbacks)?;
//! assert!(!callbacks.is_empty());
//! # std::fs::remove_dir_all(&dir)?;
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use std::sync::Arc;

use declint_core::{
    CallbackRef, Callbacks, ConfigError, ConfigSet, Decision, MatchCallback, MatchContext, Severity,
};
use mlua::{Function, Lua, Value};

/// Instructions a callback may execute per call before it is aborted.
const INSTRUCTION_BUDGET: u32 = 1_000_000;

/// The budget-abort message; used to recognize budget aborts.
const BUDGET_MESSAGE: &str = "callback exceeded its instruction budget";

/// A compiled Lua callback.
pub struct LuaCallback {
    lua: Arc<Lua>,
    function: Function,
}

impl MatchCallback for LuaCallback {
    fn evaluate(&self, ctx: &MatchContext) -> Result<Decision, String> {
        let lua = &self.lua;

        let table = lua.create_table().map_err(|e| e.to_string())?;
        table.set("match", ctx.match_text.as_str()).map_err(|e| e.to_string())?;
        table.set("start", ctx.start).map_err(|e| e.to_string())?;
        table.set("finish", ctx.finish).map_err(|e| e.to_string())?;
        table.set("line", ctx.line).map_err(|e| e.to_string())?;
        table.set("col", ctx.col).map_err(|e| e.to_string())?;
        table.set("path", ctx.path.as_str()).map_err(|e| e.to_string())?;
        table.set("language", ctx.language.as_str()).map_err(|e| e.to_string())?;
        table.set("rule", ctx.rule_id.as_str()).map_err(|e| e.to_string())?;
        let captures = lua.create_table().map_err(|e| e.to_string())?;
        for (name, value) in &ctx.captures {
            captures.set(name.as_str(), value.as_str()).map_err(|e| e.to_string())?;
        }
        table.set("captures", captures).map_err(|e| e.to_string())?;

        lua.set_hook(
            mlua::HookTriggers::new().every_nth_instruction(INSTRUCTION_BUDGET),
            |_lua, _debug| Err(mlua::Error::RuntimeError(BUDGET_MESSAGE.to_string())),
        )
        .map_err(|e| e.to_string())?;
        let result = self.function.call::<Value>(table);
        lua.remove_hook();

        match result {
            Ok(Value::Nil) | Ok(Value::Boolean(false)) => Ok(Decision::Allow),
            Ok(Value::Boolean(true)) => Ok(Decision::ViolateDefault),
            Ok(Value::Table(outcome)) => {
                let message: Option<String> = outcome.get("message").ok();
                let Some(message) = message.filter(|m| !m.is_empty()) else {
                    return Err("returned a table without a `message` string".into());
                };
                let severity = match outcome.get::<Value>("severity") {
                    Ok(Value::Nil) | Err(_) => None,
                    Ok(Value::String(s)) => {
                        let name = s.to_str().map_err(|e| e.to_string())?;
                        Some(Severity::parse(&name).ok_or_else(|| {
                            format!("unknown severity `{name}` (expected error, warning, info, hint)")
                        })?)
                    }
                    Ok(other) => {
                        return Err(format!(
                            "`severity` must be a string (found {})",
                            other.type_name()
                        ))
                    }
                };
                Ok(Decision::Violate { severity, message })
            }
            Ok(other) => Err(format!("returned {}", other.type_name())),
            Err(e) => {
                let text = e.to_string();
                if text.contains(BUDGET_MESSAGE) {
                    Err(format!(
                        "exceeded its instruction budget of {INSTRUCTION_BUDGET} instructions \
                         (possible infinite loop)"
                    ))
                } else {
                    Err(text)
                }
            }
        }
    }
}

/// Compiles every inline and file callback in `set` and registers them
/// in `callbacks`. File paths resolve relative to each config file.
///
/// Syntax errors and unreadable files are reported with the config file
/// and rule id; nothing partial is registered on failure — call sites
/// should treat an `Err` as fatal for this config set.
pub fn attach(set: &ConfigSet, callbacks: &mut Callbacks) -> Result<(), ConfigError> {
    let lua = Arc::new(Lua::new());
    for named in set.configs() {
        let base = named
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from("."));

        let mut visit = |rules: &[declint_core::Rule],
                         label: &dyn Fn(&declint_core::Rule) -> String|
         -> Result<(), ConfigError> {
            for rule in rules {
                let Some(reference) = &rule.callback else {
                    continue;
                };
                let compiled = match reference {
                    CallbackRef::Name(_) => continue,
                    CallbackRef::Inline { source } => {
                        compile(&lua, source.clone()).map_err(|e| invalid(label(rule), &e))?
                    }
                    CallbackRef::File { path } => {
                        let file = base.join(path);
                        let source = std::fs::read_to_string(&file).map_err(|e| {
                            ConfigError::new(format!(
                                "{}: cannot read callback file: {e}",
                                label(rule)
                            ))
                        })?;
                        compile(&lua, source).map_err(|e| invalid(label(rule), &e))?
                    }
                };
                callbacks.register_ref(
                    reference,
                    Arc::new(LuaCallback {
                        lua: Arc::clone(&lua),
                        function: compiled,
                    }),
                );
            }
            Ok(())
        };

        let global_label = |rule: &declint_core::Rule| format!("{}: rule '{}'", shown(&named.path), rule.id);
        visit(&named.config.rules, &global_label)?;
        for scope in &named.config.scopes {
            let scope_label = |rule: &declint_core::Rule| {
                format!(
                    "{}: scope '{}' rule '{}'",
                    shown(&named.path),
                    scope.id,
                    rule.id
                )
            };
            visit(&scope.rules, &scope_label)?;
        }
    }
    Ok(())
}

fn shown(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    if text.is_empty() {
        "<config>".to_string()
    } else {
        text
    }
}

fn invalid(label: String, e: &mlua::Error) -> ConfigError {
    ConfigError::new(format!("{label}: invalid callback: {e}"))
}

/// Compiles a snippet into the callback function. A snippet is a chunk
/// that **returns** the function — `return function(ctx) ... end` — so
/// helpers and locals above it just work; the chunk is evaluated once
/// here, at attach time.
fn compile(lua: &Lua, source: String) -> Result<Function, mlua::Error> {
    let chunk = lua.load(source).into_function()?;
    match chunk.call::<Value>(())? {
        Value::Function(function) => Ok(function),
        other => Err(mlua::Error::RuntimeError(format!(
            "callback snippet must `return function(ctx) ... end` (returned {})",
            other.type_name()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use declint_core::{Config, Linter};

    fn linter_with(yaml: &str) -> Linter {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("declint-lua-{}-{n}-t", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".declint.yaml"), yaml).unwrap();
        let set = ConfigSet::discover(&dir).unwrap();
        let mut callbacks = Callbacks::new();
        attach(&set, &mut callbacks).unwrap();
        Linter::new(set.configs()[0].config.clone(), &callbacks).unwrap()
    }

    fn config_yaml(rule: &str) -> String {
        format!("version: 1\nrules:\n  - id: probe\n    pattern: 'TODO(?<word>\\w*)'\n{rule}")
    }

    #[test]
    fn inline_return_table_violates() {
        let linter = linter_with(&config_yaml(
            "    callback: |\n      return function(c)\n        return { severity = \"hint\", message = \"seen \" .. c.match .. \" at \" .. c.line .. \":\" .. c.col }\n      end\n",
        ));
        let v = linter.lint_in(
            declint_core::DocInfo { path: "f.txt", language: "text" },
            "ab\nTODOhere",
        );
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].severity, Severity::Hint);
        assert_eq!(v[0].message, "seen TODOhere at 2:1");
    }

    #[test]
    fn inline_nil_allows() {
        let linter = linter_with(&config_yaml(
            "    callback: |\n      return function(c) return nil end\n",
        ));
        assert!(linter.lint("TODO").is_empty());
    }

    #[test]
    fn inline_true_uses_rule_message() {
        let linter = linter_with(&config_yaml(
            "    callback: |\n      return function(c) return true end\n    message: \"default for '{match}'\"\n",
        ));
        let v = linter.lint("TODOx");
        assert_eq!(v[0].message, "default for 'TODOx'");
    }

    #[test]
    fn captures_table_is_populated() {
        let yaml = "\
version: 1
rules:
  - id: probe
    pattern: 'TODO(\\w+)'
    callback: |
      return function(c)
        if c.captures[\"1\"] == \"fix\" then
          return { message = \"numbered works\" }
        end
        return nil
      end
";
        let linter = linter_with(yaml);
        let v = linter.lint("TODOfix");
        assert_eq!(v[0].message, "numbered works");
    }

    #[test]
    fn context_fields_are_exposed() {
        let linter = linter_with(&config_yaml(
            "    callback: |\n      return function(c)\n        return { message = c.rule .. \"/\" .. c.language .. \"/\" .. c.path .. \"/\" .. c.match .. \"/\" .. c.start .. \"-\" .. c.finish }\n      end\n",
        ));
        let v = linter.lint_in(
            declint_core::DocInfo { path: "p.sh", language: "sh" },
            "go TODO go",
        );
        assert_eq!(v[0].message, "probe/sh/p.sh/TODO/3-7");
    }

    #[test]
    fn lua_error_surfaces_without_crashing() {
        let linter = linter_with(&config_yaml(
            "    callback: |\n      return function(c) error(\"boom \" .. c.rule) end\n",
        ));
        let v = linter.lint("TODO");
        assert_eq!(v[0].severity, Severity::Error);
        let message = &v[0].message;
        assert!(message.contains("callback error"), "{message}");
        assert!(message.contains("boom probe"), "{message}");
    }

    #[test]
    fn runaway_loop_hits_the_budget() {
        let linter = linter_with(&config_yaml(
            "    callback: |\n      return function(c) while true do end end\n",
        ));
        let v = linter.lint("TODO");
        assert_eq!(v[0].severity, Severity::Error);
        assert!(
            v[0].message.contains("instruction budget"),
            "{}",
            v[0].message
        );
    }

    #[test]
    fn bad_return_shapes_are_reported() {
        let linter = linter_with(&config_yaml(
            "    callback: |\n      return function(c) return 42 end\n",
        ));
        let v = linter.lint("TODO");
        assert!(v[0].message.contains("returned"), "{}", v[0].message);

        let linter = linter_with(&config_yaml(
            "    callback: |\n      return function(c) return { severity = \"hint\" } end\n",
        ));
        let v = linter.lint("TODO");
        assert!(v[0].message.contains("without a `message`"), "{}", v[0].message);
    }

    #[test]
    fn syntax_error_is_a_config_error() {
        let dir = std::env::temp_dir().join(format!("declint-lua-{}-syn", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(".declint.yaml"),
            "version: 1\nrules:\n  - id: probe\n    pattern: 'x'\n    callback: 'returnnil junk'\n",
        )
        .unwrap();
        let set = ConfigSet::discover(&dir).unwrap();
        let mut callbacks = Callbacks::new();
        let e = attach(&set, &mut callbacks).unwrap_err();
        assert!(e.to_string().contains("rule 'probe'"), "{e}");
        assert!(e.to_string().contains("invalid callback"), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn file_callback_resolves_relative_to_config() {
        let dir = std::env::temp_dir().join(format!("declint-lua-{}-file", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("checks")).unwrap();
        std::fs::write(
            dir.join(".declint.yaml"),
            "version: 1\nrules:\n  - id: probe\n    pattern: 'TODO'\n    callback: checks/cb.lua\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("checks").join("cb.lua"),
            "return function(c) return { message = \"from file: \" .. c.match } end\n",
        )
        .unwrap();
        let set = ConfigSet::discover(&dir).unwrap();
        let mut callbacks = Callbacks::new();
        attach(&set, &mut callbacks).unwrap();
        let linter = Linter::new(set.configs()[0].config.clone(), &callbacks).unwrap();
        let v = linter.lint("TODO");
        assert_eq!(v[0].message, "from file: TODO");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn plain_configs_need_no_lua() {
        let config = Config::from_str(
            "version: 1\nrules:\n  - id: r\n    pattern: x\n    message: m\n",
        )
        .unwrap();
        let dir = std::env::temp_dir().join(format!("declint-lua-{}-plain", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".declint.yaml"), "version: 1\nrules:\n  - id: r\n    pattern: x\n    message: m\n").unwrap();
        let set = ConfigSet::discover(&dir).unwrap();
        let mut callbacks = Callbacks::new();
        attach(&set, &mut callbacks).unwrap();
        assert!(Linter::new(config, &callbacks).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
