//! Rule callbacks: user code that decides whether a match is a
//! violation, and with what message.
//!
//! Core is engine-free: rules carry a [`CallbackRef`] (a name, an inline
//! source, or a file path), hosts register [`MatchCallback`]
//! implementations by name/ref, and [`Linter::build`] wires them up.
//! The `declint-lua` crate compiles Lua callbacks; Rust embedders
//! implement [`MatchCallback`] directly.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::Severity;

/// Everything a callback knows about one match.
#[derive(Debug, Clone)]
pub struct MatchContext {
    /// The linted file's path (`""` when the caller has none).
    pub path: String,
    /// The document's language id (`""` when the caller has none).
    pub language: String,
    /// The id of the rule that matched.
    pub rule_id: String,
    /// Absolute byte offset of the match start.
    pub start: usize,
    /// Absolute byte offset just past the match.
    pub finish: usize,
    /// 1-based line of the match start.
    pub line: usize,
    /// 1-based character column of the match start.
    pub col: usize,
    /// The text the pattern matched.
    pub match_text: String,
    /// Capture groups: named groups by name, numbered groups as `"1"`,
    /// `"2"`, ... (the whole match is `match_text`).
    pub captures: Vec<(String, String)>,
}

impl MatchContext {
    /// A capture's text: try a named group, then a numbered one.
    pub fn capture(&self, name: &str) -> Option<&str> {
        self.captures
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// What a callback decided about one match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// No diagnostic — the match is allowed.
    Allow,
    /// A violation with an explicit message (and optional severity
    /// override; falls back to the rule's `severity`).
    Violate {
        /// Overrides the rule's severity when set.
        severity: Option<Severity>,
        /// The diagnostic message.
        message: String,
    },
    /// A violation with the rule's own `message` template rendered for
    /// this match (what Lua's `return true` means).
    ViolateDefault,
}

/// A callback implementation. Must be pure with respect to the match:
/// same input, same decision.
pub trait MatchCallback: Send + Sync {
    /// Decides one match. An `Err` surfaces as an `error`-severity
    /// diagnostic naming the rule — the lint run itself is never
    /// affected.
    fn evaluate(&self, ctx: &MatchContext) -> Result<Decision, String>;
}

struct NoopCallback;

impl MatchCallback for NoopCallback {
    fn evaluate(&self, _ctx: &MatchContext) -> Result<Decision, String> {
        Ok(Decision::Allow)
    }
}

/// A set of registered callbacks, keyed by name or by
/// [`CallbackRef`] identity (inline source / file path).
#[derive(Default)]
pub struct Callbacks {
    map: HashMap<String, Arc<dyn MatchCallback>>,
}

impl Callbacks {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a callback under a plain name (what a rule's
    /// `callback: name` refers to).
    pub fn register(&mut self, name: impl Into<String>, callback: Arc<dyn MatchCallback>) {
        self.map.insert(format!("name:{}", name.into()), callback);
    }

    /// Registers a convenience callback that allows every match of the
    /// named rule.
    pub fn register_allow(&mut self, name: impl Into<String>) {
        self.register(name, Arc::new(NoopCallback));
    }

    /// Registers the implementation for an inline/file reference — the
    /// loader's entry point (e.g. `declint-lua`).
    pub fn register_ref(&mut self, reference: &CallbackRef, callback: Arc<dyn MatchCallback>) {
        self.map.insert(key_of(reference), callback);
    }

    /// Looks up the implementation for a rule's reference.
    pub fn resolve(&self, reference: &CallbackRef) -> Option<Arc<dyn MatchCallback>> {
        self.map.get(&key_of(reference)).cloned()
    }

    /// Whether any callbacks are registered at all.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl fmt::Debug for Callbacks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Callbacks")
            .field("count", &self.map.len())
            .finish()
    }
}

fn key_of(reference: &CallbackRef) -> String {
    match reference {
        CallbackRef::Name(name) => format!("name:{name}"),
        CallbackRef::File { path } => format!("file:{path}"),
        CallbackRef::Inline { source } => format!("inline:{source}"),
    }
}

/// A rule's reference to its callback, as written in the config.
///
/// Classification of the `callback:` string:
///
/// * ends with `.lua` — a file path, relative to the config file;
/// * contains whitespace or newlines — inline Lua source;
/// * otherwise — the name of a registered callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackRef {
    /// `callback: |` block scalar.
    Inline {
        /// The Lua source.
        source: String,
    },
    /// `callback: checks/foo.lua`.
    File {
        /// The path as written, relative to the config file.
        path: String,
    },
    /// `callback: my_name` — resolved from the registry at build time.
    Name(String),
}

impl CallbackRef {
    /// Classifies a `callback:` string.
    pub fn parse(s: &str) -> Self {
        if s.ends_with(".lua") {
            Self::File { path: s.to_string() }
        } else if s.chars().any(char::is_whitespace) {
            Self::Inline { source: s.to_string() }
        } else {
            Self::Name(s.to_string())
        }
    }

    /// A human-readable description for error messages.
    pub fn describe(&self) -> String {
        match self {
            Self::Inline { .. } => "inline callback".to_string(),
            Self::File { path } => format!("callback file '{path}'"),
            Self::Name(name) => format!("callback '{name}'"),
        }
    }
}
