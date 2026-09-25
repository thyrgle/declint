//! Rule-file loading: schema validation with rule ids and file lines in
//! every error.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;

use serde_yaml::Value;

use crate::callback::CallbackRef;
use crate::presets;
use crate::template::Template;
use crate::Severity;

/// How a rule finds its matches: a compiled regex (the default) or a
/// registered parser function (see [`Rule::parser`]).
#[derive(Debug, Clone)]
pub(crate) enum Matcher {
    /// `pattern:` — compiled at config-load time.
    Regex(regex::Regex),
    /// `parser:` — resolved against the registry at linter build time.
    Parser,
}

/// One resolved `import:` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ImportSource {
    /// `preset:<name>` — an embedded library config.
    Preset(String),
    /// A path ending in `.yaml`/`.yml`, relative to the importing file.
    File(String),
}

impl ImportSource {
    /// The entry as written in the config, for error messages.
    fn as_written(&self) -> String {
        match self {
            Self::Preset(name) => format!("preset:{name}"),
            Self::File(path) => path.clone(),
        }
    }
}

/// Classifies an `import:` entry.
pub(crate) fn classify_import(entry: &str) -> Result<ImportSource, String> {
    if let Some(name) = entry.strip_prefix("preset:") {
        if name.is_empty() {
            return Err("preset imports need a name (`preset:python`)".into());
        }
        Ok(ImportSource::Preset(name.to_string()))
    } else if entry.ends_with(".yaml") || entry.ends_with(".yml") {
        Ok(ImportSource::File(entry.to_string()))
    } else {
        Err(
            "import entries must be `preset:<name>` or a path ending in `.yaml`/`.yml`"
                .into(),
        )
    }
}

/// The config schema version this declint understands.
pub const SUPPORTED_VERSION: u64 = 1;

/// Everything that can go wrong while loading a config file.
///
/// Carries a 1-based file line whenever the problem can be pinned to one
/// (`declint.yaml:5: rule 0 ('no-tabs'): invalid pattern ...`). Rule-level
/// errors point at the rule's `- ` entry line; YAML syntax errors point at
/// the exact position reported by the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    message: String,
    line: Option<usize>,
    column: Option<usize>,
    path: Option<String>,
}

impl ConfigError {
    /// Creates an error from a message alone (no file position).
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            line: None,
            column: None,
            path: None,
        }
    }

    fn at_line(message: impl Into<String>, line: Option<usize>) -> Self {
        Self {
            message: message.into(),
            line,
            column: None,
            path: None,
        }
    }

    fn with_syntax_location(mut self, line: usize, column: usize) -> Self {
        self.line = Some(line);
        self.column = Some(column);
        self
    }

    /// Attaches a config file path, for errors from
    /// [`Config::load`](Config::load).
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Annotates an error with the import entry it occurred behind.
    fn at_import(mut self, entry: &str, parent_origin: &str) -> Self {
        self.message = format!("import '{entry}': {}", self.message);
        if self.path.is_none() {
            self.path = Some(parent_origin.to_string());
        }
        self
    }

    /// The problem's 1-based line in the config file, when known.
    pub fn line(&self) -> Option<usize> {
        self.line
    }

    /// The problem's 1-based column in the config file, when known.
    pub fn column(&self) -> Option<usize> {
        self.column
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(path) = &self.path {
            write!(f, "{path}")?;
        }
        if let Some(line) = self.line {
            write!(f, ":{line}")?;
            if let Some(column) = self.column {
                write!(f, ":{column}")?;
            }
        }
        if self.path.is_some() || self.line.is_some() {
            write!(f, ": ")?;
        }
        f.write_str(&self.message)
    }
}

impl std::error::Error for ConfigError {}

/// A validated configuration: the schema version, global rules, and
/// scopes.
#[derive(Debug, Clone)]
pub struct Config {
    /// The config's declared schema version (always
    /// [`SUPPORTED_VERSION`]).
    pub version: u64,
    /// The editor language ids this config applies to (exact match
    /// against the client's `languageId`; in Neovim, the filetype).
    /// Empty = every language.
    pub languages: Vec<String>,
    /// The raw `import:` entries, as written.
    pub imports: Vec<String>,
    /// The classified `import:` entries (drained during resolution).
    import_entries: Vec<ImportSource>,
    /// The validated global rules, in file order (imported rules
    /// first).
    pub rules: Vec<Rule>,
    /// The validated scopes, in file order (imported scopes first).
    pub scopes: Vec<Scope>,
}

impl Config {
    /// Whether this config applies to documents with language id `id`.
    /// Configs without a `languages` key apply to everything.
    pub fn matches_language(&self, id: &str) -> bool {
        self.languages.is_empty() || self.languages.iter().any(|l| l == id)
    }
}

/// One validated lint rule.
#[derive(Debug, Clone)]
pub struct Rule {
    /// The rule's unique id — what shows up as the diagnostic's code, and
    /// what suppressions (a future feature) will name. Unique across all
    /// global rules and every scope's rules.
    pub id: String,
    /// The pattern, as written in the config. Empty when the rule uses a
    /// `parser` instead.
    pub pattern: String,
    /// How serious a hit is (default: [`Severity::Warning`]).
    pub severity: Severity,
    /// The message template, parsed and capture-validated. Optional when
    /// the rule has a `callback` (then it doubles as the message for
    /// callbacks that return "violate with the default message").
    pub message: Option<Template>,
    /// The rule's callback reference, if any.
    pub callback: Option<CallbackRef>,
    /// The rule's parser reference, if any — mutually exclusive with
    /// [`Rule::pattern`].
    pub parser: Option<CallbackRef>,
    /// How the rule finds matches — construction only succeeds when the
    /// regex (if any) is valid.
    pub(crate) matcher: Matcher,
}

/// One validated scope: a segmenter (`start`/`end`) plus the rules that
/// run only inside its regions.
///
/// `start` and `end` are compiled with multi-line mode forced on, so `^`
/// and `$` anchor to lines — the natural way to write region boundaries.
#[derive(Debug, Clone)]
pub struct Scope {
    /// The scope's unique id. Shares a namespace with rule ids.
    pub id: String,
    /// Where a region begins, as written.
    pub start: String,
    /// Where a region ends, as written (`None` = run to end of file).
    pub end: Option<String>,
    /// The rules that run inside this scope's regions.
    pub rules: Vec<Rule>,
    /// The compiled `start` pattern (multi-line forced).
    pub(crate) start_re: regex::Regex,
    /// The compiled `end` pattern (multi-line forced).
    pub(crate) end_re: Option<regex::Regex>,
}

impl Config {
    /// Loads and validates a config from YAML text.
    ///
    /// `preset:` imports resolve against the embedded library; relative
    /// file imports need a file — use [`Config::load`] for those.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(yaml: &str) -> Result<Self, ConfigError> {
        Self::resolve(yaml, None, "<config>", "<config>", &mut Vec::new())
    }

    /// Loads and validates the config site at `path` (a file), resolving
    /// its `import:` entries recursively.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let shown = path.display().to_string();
        let yaml = std::fs::read_to_string(path).map_err(|e| {
            ConfigError::new(format!("cannot read config file: {e}")).with_path(&shown)
        })?;
        let canonical = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf());
        let mut stack = Vec::new();
        let base = path.parent().map(Path::to_path_buf);
        Self::resolve(
            &yaml,
            base.as_deref(),
            &shown,
            &canonical.display().to_string(),
            &mut stack,
        )
        .map_err(|e| e.with_path(shown))
    }

    /// Parses `yaml` (origin: a display label for error messages) and
    /// resolves its imports depth-first. `stack` holds the markers of
    /// every ancestor — canonical file paths and `preset:<name>` — for
    /// cycle detection.
    fn resolve(
        yaml: &str,
        base: Option<&std::path::Path>,
        origin: &str,
        marker: &str,
        stack: &mut Vec<String>,
    ) -> Result<Self, ConfigError> {
        if stack.iter().any(|m| m == marker) {
            return Err(ConfigError::new(format!(
                "import cycle detected ({origin} imports itself, directly or indirectly)"
            )));
        }
        let value: Value = serde_yaml::from_str(yaml).map_err(|e| {
            let err = ConfigError::new(e.to_string());
            match e.location() {
                Some(loc) => err.with_syntax_location(loc.line(), loc.column()),
                None => err,
            }
        })?;
        let mut config = Self::from_value(value, yaml)?;
        stack.push(marker.to_string());

        let mut rules: Vec<(Rule, String)> = Vec::new();
        let mut scopes: Vec<(Scope, String)> = Vec::new();
        for entry in std::mem::take(&mut config.import_entries) {
            let entry_text = entry.as_written();
            let entry_text = &entry_text;
            match &entry {
                ImportSource::Preset(name) => {
                    let Some(preset) = presets::lookup(name) else {
                        return Err(ConfigError::new(format!(
                            "unknown preset `{name}` (available: {})",
                            presets::names()
                        ))
                        .at_import(entry_text, origin));
                    };
                    let child_origin = format!("preset:{name}");
                    let child =
                        Self::resolve(preset.content, None, &child_origin, &child_origin, stack)
                            .map_err(|e| in_import(entry_text, e))?;
                    Self::absorb(
                        &child,
                        &child_origin,
                        entry_text,
                        origin,
                        &mut rules,
                        &mut scopes,
                    )?;
                }
                ImportSource::File(relative) => {
                    let Some(base) = base else {
                        return Err(ConfigError::new(
                            "relative import requires loading the config from a file \
                             (use Config::load)",
                        )
                        .at_import(entry_text, origin));
                    };
                    let path = base.join(relative);
                    let child_yaml = std::fs::read_to_string(&path).map_err(|e| {
                        ConfigError::new(format!(
                            "cannot read imported config `{relative}`: {e}"
                        ))
                        .at_import(entry_text, origin)
                    })?;
                    let canonical = path.canonicalize().map_err(|e| {
                        ConfigError::new(format!(
                            "cannot resolve imported config `{relative}`: {e}"
                        ))
                        .at_import(entry_text, origin)
                    })?;
                    let child_origin = path.display().to_string();
                    let child = Self::resolve(
                        &child_yaml,
                        Some(path.parent().unwrap_or(Path::new("."))),
                        &child_origin,
                        &canonical.display().to_string(),
                        stack,
                    )
                    .map_err(|e| in_import(entry_text, e))?;
                    Self::absorb(
                        &child,
                        &child_origin,
                        entry_text,
                        origin,
                        &mut rules,
                        &mut scopes,
                    )?;
                }
            }
        }
        for rule in std::mem::take(&mut config.rules) {
            rules.push((rule, origin.to_string()));
        }
        for scope in std::mem::take(&mut config.scopes) {
            scopes.push((scope, origin.to_string()));
        }

        // One id namespace across everything, imported and own.
        let mut seen: HashMap<&str, &str> = HashMap::new();
        for (rule, rule_origin) in &rules {
            if let Some(first) = seen.get(rule.id.as_str()) {
                return Err(ConfigError::new(format!(
                    "duplicate rule id `{}` (defined in {first} and {rule_origin})",
                    rule.id
                )));
            }
            seen.insert(rule.id.as_str(), rule_origin);
        }
        for (scope, scope_origin) in &scopes {
            if let Some(first) = seen.get(scope.id.as_str()) {
                return Err(ConfigError::new(format!(
                    "duplicate scope id `{}` (defined in {first} and {scope_origin})",
                    scope.id
                )));
            }
            seen.insert(scope.id.as_str(), scope_origin);
        }

        // The presence check happens after imports merge: a config that
        // only imports (no own rules/scopes) is legitimate.
        if rules.is_empty() && scopes.is_empty() {
            return Err(ConfigError::new(
                "config must define `rules` or `scopes` (directly or via imports)",
            ));
        }

        stack.pop();
        config.rules = rules.into_iter().map(|(rule, _)| rule).collect();
        config.scopes = scopes.into_iter().map(|(scope, _)| scope).collect();
        Ok(config)
    }

    /// Merges one imported fragment: appended first-in (before the
    /// importer's own items), with its language policy checked.
    fn absorb(
        child: &Config,
        child_origin: &str,
        entry: &str,
        parent_origin: &str,
        rules: &mut Vec<(Rule, String)>,
        scopes: &mut Vec<(Scope, String)>,
    ) -> Result<(), ConfigError> {
        if !child.languages.is_empty() {
            return Err(ConfigError::new(format!(
                "imported config declares `languages` ({}) — language policy belongs to \
                 the importing config",
                child.languages.join(", ")
            ))
            .at_import(entry, parent_origin));
        }
        rules.extend(
            child
                .rules
                .iter()
                .cloned()
                .map(|rule| (rule, child_origin.to_string())),
        );
        scopes.extend(
            child
                .scopes
                .iter()
                .cloned()
                .map(|scope| (scope, child_origin.to_string())),
        );
        Ok(())
    }

    fn from_value(value: Value, yaml: &str) -> Result<Self, ConfigError> {
        let Value::Mapping(map) = value else {
            return Err(ConfigError::new(
                "config must be a YAML mapping with `version` and `rules` and/or `scopes` keys",
            ));
        };

        let version_value = map
            .get(Value::from("version"))
            .ok_or_else(|| ConfigError::new("missing `version` key"))?;
        let version = version_value
            .as_u64()
            .ok_or_else(|| ConfigError::at_line("`version` must be an integer", None))?;
        if version != SUPPORTED_VERSION {
            return Err(ConfigError::at_line(
                format!(
                    "unsupported config version {version} (this declint understands version \
                     {SUPPORTED_VERSION})"
                ),
                None,
            ));
        }

        let rules_value = map.get(Value::from("rules"));
        let scopes_value = map.get(Value::from("scopes"));

        let languages = match map.get(Value::from("languages")) {
            None => Vec::new(),
            Some(value) => {
                let Value::Sequence(entries) = value else {
                    return Err(ConfigError::new(
                        "`languages` must be a list of language ids (e.g. `[markdown, sh]`)",
                    ));
                };
                let mut seen = HashSet::new();
                let mut languages = Vec::new();
                for entry in entries {
                    let Some(language) = entry.as_str().filter(|s| !s.is_empty()) else {
                        return Err(ConfigError::new(
                            "`languages` entries must be non-empty strings",
                        ));
                    };
                    if seen.insert(language.to_string()) {
                        languages.push(language.to_string());
                    }
                }
                languages
            }
        };

        let mut seen_ids = HashSet::new();

        let rules = match rules_value {
            None => Vec::new(),
            Some(v) => {
                let Value::Sequence(entries) = v else {
                    return Err(ConfigError::new("`rules` must be a list of rule mappings"));
                };
                let lines = sequence_item_lines(yaml, "rules");
                entries
                    .iter()
                    .enumerate()
                    .map(|(index, entry)| {
                        parse_rule(entry, index, "", lines.get(index).copied(), &mut seen_ids)
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
        };

        let scopes = match scopes_value {
            None => Vec::new(),
            Some(v) => {
                let Value::Sequence(entries) = v else {
                    return Err(ConfigError::new("`scopes` must be a list of scope mappings"));
                };
                let lines = sequence_item_lines(yaml, "scopes");
                entries
                    .iter()
                    .enumerate()
                    .map(|(index, entry)| {
                        parse_scope(
                            entry,
                            index,
                            lines.get(index).copied(),
                            yaml,
                            &mut seen_ids,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
        };

        let import_entries = match map.get(Value::from("import")) {
            None => Vec::new(),
            Some(value) => {
                let Value::Sequence(entries) = value else {
                    return Err(ConfigError::new(
                        "`import` must be a list of entries (`preset:<name>` or a \
                         `.yaml`/`.yml` path)",
                    ));
                };
                let mut classified = Vec::new();
                for entry in entries {
                    let Some(text) = entry.as_str().filter(|s| !s.is_empty()) else {
                        return Err(ConfigError::new(
                            "`import` entries must be non-empty strings",
                        ));
                    };
                    match classify_import(text) {
                        Ok(source) => classified.push(source),
                        Err(message) => return Err(ConfigError::new(message)),
                    }
                }
                classified
            }
        };

        Ok(Self {
            version,
            languages,
            imports: import_entries
                .iter()
                .map(|source| match source {
                    ImportSource::Preset(name) => format!("preset:{name}"),
                    ImportSource::File(path) => path.clone(),
                })
                .collect(),
            import_entries,
            rules,
            scopes,
        })
    }
}

impl Rule {
    /// The capture-group names of the rule's regex, by group index.
    /// Empty for parser rules (their capture names are free-form).
    pub(crate) fn capture_names(&self) -> Vec<Option<&str>> {
        match &self.matcher {
            Matcher::Regex(regex) => regex.capture_names().collect(),
            Matcher::Parser => Vec::new(),
        }
    }
}

/// Compiles a scope boundary pattern with multi-line and CRLF modes
/// forced on, so `^`/`$` anchor to lines — including `\r\n` line
/// endings, which Windows-edited files use.
fn compile_boundary(pattern: &str) -> Result<regex::Regex, regex::Error> {
    regex::RegexBuilder::new(pattern)
        .multi_line(true)
        .crlf(true)
        .build()
}

fn parse_scope(
    entry: &Value,
    index: usize,
    line: Option<usize>,
    yaml: &str,
    seen_ids: &mut HashSet<String>,
) -> Result<Scope, ConfigError> {
    let at_scope = |message: &str| -> ConfigError {
        let id = entry
            .get(Value::from("id"))
            .and_then(|v| v.as_str())
            .map(|id| format!(" ('{id}')"))
            .unwrap_or_default();
        ConfigError::at_line(format!("scope {index}{id}: {message}"), line)
    };

    let Value::Mapping(map) = entry else {
        return Err(at_scope("must be a mapping with `id`, `start`, and `rules` keys"));
    };

    for key in map.keys() {
        if let Some(key) = key.as_str() {
            if !matches!(key, "id" | "start" | "end" | "rules") {
                return Err(at_scope(&format!(
                    "unknown key `{key}` (expected one of `id`, `start`, `end`, `rules`)"
                )));
            }
        }
    }

    let missing = |key: &str| at_scope(&format!("missing `{key}` key"));

    let id_value = map.get(Value::from("id")).ok_or_else(|| missing("id"))?;
    let id = id_value
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| at_scope("`id` must be a non-empty string"))?
        .to_string();
    if !seen_ids.insert(id.clone()) {
        return Err(at_scope(&format!(
            "duplicate id `{id}` — rule and scope ids share one namespace and must be unique"
        )));
    }

    let start_value = map.get(Value::from("start")).ok_or_else(|| missing("start"))?;
    let start = start_value
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| at_scope("`start` must be a non-empty string"))?
        .to_string();
    let start_re =
        compile_boundary(&start).map_err(|e| at_scope(&format!("invalid start pattern: {e}")))?;

    let end = match map.get(Value::from("end")) {
        None => None,
        Some(end_value) => match end_value.as_str().filter(|s| !s.is_empty()) {
            Some(end) => {
                compile_boundary(end)
                    .map_err(|e| at_scope(&format!("invalid end pattern: {e}")))?;
                Some(end.to_string())
            }
            None => {
                return Err(at_scope("`end` must be a non-empty string"));
            }
        },
    };
    let end_re = end
        .as_ref()
        .map(|pattern| compile_boundary(pattern).expect("validated above"));

    let rules = match map.get(Value::from("rules")) {
        None => Vec::new(),
        Some(rules_value) => {
            let Value::Sequence(entries) = rules_value else {
                return Err(at_scope("`rules` must be a list of rule mappings"));
            };
            let nested_lines = nested_sequence_item_lines(yaml, line, "rules");
            entries
                .iter()
                .enumerate()
                .map(|(rule_index, rule_entry)| {
                    let label = format!("scope '{id}' ");
                    parse_rule(
                        rule_entry,
                        rule_index,
                        &label,
                        nested_lines.get(rule_index).copied(),
                        seen_ids,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?
        }
    };
    if rules.is_empty() {
        return Err(at_scope("scope has no `rules` — a scope without rules does nothing"));
    }

    Ok(Scope {
        id,
        start,
        end,
        rules,
        start_re,
        end_re,
    })
}

fn parse_rule(
    entry: &Value,
    index: usize,
    prefix: &str,
    line: Option<usize>,
    seen_ids: &mut HashSet<String>,
) -> Result<Rule, ConfigError> {
    let at_rule = |message: String| -> ConfigError {
        let id = entry
            .get(Value::from("id"))
            .and_then(|v| v.as_str())
            .map(|id| format!(" ('{id}')"))
            .unwrap_or_default();
        ConfigError::at_line(format!("{prefix}rule {index}{id}: {message}"), line)
    };

    let Value::Mapping(map) = entry else {
        return Err(at_rule(
            "must be a mapping with `id`, `pattern`, and `message` keys".into(),
        ));
    };

    for key in map.keys() {
        if let Some(key) = key.as_str() {
            if !matches!(
                key,
                "id" | "pattern" | "message" | "severity" | "callback" | "parser"
            ) {
                return Err(at_rule(format!(
                    "unknown key `{key}` (expected one of `id`, `pattern`, `parser`, \
                     `message`, `severity`, `callback`)"
                )));
            }
        }
    }

    let missing = |key: &str| at_rule(format!("missing `{key}` key"));

    let pattern_value = map.get(Value::from("pattern"));
    let parser_value = map.get(Value::from("parser"));

    // Exactly one of `pattern` / `parser` — the rule's matcher.
    let (pattern, parser, matcher) = match (pattern_value, parser_value) {
        (None, None) => {
            return Err(at_rule(
                "rule must have a `pattern` or a `parser` to find matches".into(),
            ));
        }
        (Some(_), Some(_)) => {
            return Err(at_rule(
                "`pattern` and `parser` are mutually exclusive — a rule finds matches one way"
                    .into(),
            ));
        }
        (Some(value), None) => {
            let pattern = value
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| at_rule("`pattern` must be a non-empty string".into()))?
                .to_string();
            let regex = regex::Regex::new(&pattern)
                .map_err(|e| at_rule(format!("invalid pattern: {e}")))?;
            (pattern, None, Matcher::Regex(regex))
        }
        (None, Some(value)) => {
            let reference = value
                .as_str()
                .filter(|s| !s.is_empty())
                .map(CallbackRef::parse)
                .ok_or_else(|| at_rule("`parser` must be a non-empty string".into()))?;
            (String::new(), Some(reference), Matcher::Parser)
        }
    };

    let id_value = map.get(Value::from("id")).ok_or_else(|| missing("id"))?;
    let id = id_value
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| at_rule("`id` must be a non-empty string".into()))?
        .to_string();
    if !seen_ids.insert(id.clone()) {
        return Err(at_rule(format!(
            "duplicate id `{id}` — rule and scope ids share one namespace and must be unique"
        )));
    }

    let callback = match map.get(Value::from("callback")) {
        None => None,
        Some(value) => match value.as_str().filter(|s| !s.is_empty()) {
            Some(source) => Some(CallbackRef::parse(source)),
            None => {
                return Err(at_rule("`callback` must be a non-empty string".into()));
            }
        },
    };

    let message = match map.get(Value::from("message")) {
        None => {
            if callback.is_none() {
                return Err(at_rule(
                    "rule must have a `message` (or a `callback` to compute one)".into(),
                ));
            }
            None
        }
        Some(message_value) => {
            let message_src = message_value
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| at_rule("`message` must be a non-empty string".into()))?;
            let template = Template::parse(message_src)
                .map_err(|e| at_rule(format!("invalid message template: {e}")))?;
            // Placeholder names are checked against the regex's capture
            // groups; parser rules provide their own names at runtime.
            if let Matcher::Regex(regex) = &matcher {
                template
                    .validate(regex)
                    .map_err(|e| at_rule(format!("invalid message template: {e}")))?;
            }
            Some(template)
        }
    };

    let severity = match map.get(Value::from("severity")) {
        None => Severity::Warning,
        Some(value) => match value.as_str().and_then(Severity::parse) {
            Some(severity) => severity,
            None => {
                return Err(at_rule(format!(
                    "`severity` must be one of `error`, `warning`, `info`, `hint` (found `{}`)",
                    value.as_str().unwrap_or("<non-string>"),
                )))
            }
        },
    };

    Ok(Rule {
        id,
        pattern,
        severity,
        message,
        callback,
        parser,
        matcher,
    })
}

/// Finds the 1-based line of each `- ` item of the block sequence stored
/// under `key` — the rule/scope entry lines. Flow-style sequences
/// (`rules: [...]`) yield nothing, and errors then simply carry no line.
fn sequence_item_lines(yaml: &str, key: &str) -> Vec<usize> {
    block_seq_item_lines(
        yaml.lines().enumerate().map(|(i, l)| (i + 1, l)),
        key,
    )
}

/// Like [`sequence_item_lines`], but scanning only after `from_line`
/// (1-based, exclusive) — for the `rules` nested inside a scope whose
/// `- ` entry is on that line.
fn nested_sequence_item_lines(yaml: &str, from_line: Option<usize>, key: &str) -> Vec<usize> {
    let skip = from_line.unwrap_or(usize::MAX);
    block_seq_item_lines(
        yaml.lines()
            .enumerate()
            .skip(skip)
            .map(|(i, l)| (i + 1, l)),
        key,
    )
}

fn block_seq_item_lines<'a>(
    lines: impl Iterator<Item = (usize, &'a str)>,
    key: &str,
) -> Vec<usize> {
    let key_prefix = format!("{key}:");
    let mut lines = Vec::from_iter(lines);
    let mut items = Vec::new();
    let mut in_sequence = false;
    let mut key_indent = 0usize;
    for (no, line) in lines.drain(..) {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();
        if !in_sequence {
            if trimmed.starts_with(&key_prefix) && trimmed[key_prefix.len()..].trim().is_empty() {
                in_sequence = true;
                key_indent = indent;
            }
        } else if indent <= key_indent {
            break; // the sequence is over
        } else if trimmed.starts_with("- ") || trimmed == "-" {
            items.push(no);
        }
    }
    items
}

/// Annotates a child config's error with the import entry that pulled it
/// in — the message chains through every import level.
fn in_import(entry: &str, err: ConfigError) -> ConfigError {
    ConfigError {
        message: format!("in import '{entry}': {}", err.message),
        line: err.line,
        column: err.column,
        path: err.path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_block_sequence_items() {
        let yaml = "\
version: 1
rules:
  - id: a
    pattern: x

  # comment
  - id: b
    pattern: y
other: key
";
        assert_eq!(sequence_item_lines(yaml, "rules"), vec![3, 7]);
    }

    #[test]
    fn finds_nested_items_after_a_line() {
        let yaml = "\
version: 1
scopes:
  - id: s
    start: 'x'
    rules:
      - id: r1
        pattern: p
      - id: r2
        pattern: q
";
        // The scope's `- ` entry is on line 3; its nested rule items are
        // lines 6 and 8.
        assert_eq!(nested_sequence_item_lines(yaml, Some(3), "rules"), vec![6, 8]);
    }

    #[test]
    fn flow_style_yields_nothing() {
        assert!(sequence_item_lines("rules: [{id: a}]\n", "rules").is_empty());
    }
}
