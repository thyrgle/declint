//! Rule-file loading: schema validation with rule ids and file lines in
//! every error.

use std::collections::HashSet;
use std::fmt;
use std::path::Path;

use serde_yaml::Value;

use crate::template::Template;
use crate::Severity;

/// The config schema version this ezlint understands.
pub const SUPPORTED_VERSION: u64 = 1;

/// Everything that can go wrong while loading a config file.
///
/// Carries a 1-based file line whenever the problem can be pinned to one
/// (`ezlint.yaml:5: rule 0 ('no-tabs'): invalid pattern ...`). Rule-level
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
    fn new(message: impl Into<String>) -> Self {
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
    /// The validated global rules, in file order.
    pub rules: Vec<Rule>,
    /// The validated scopes, in file order.
    pub scopes: Vec<Scope>,
}

/// One validated lint rule.
#[derive(Debug, Clone)]
pub struct Rule {
    /// The rule's unique id — what shows up as the diagnostic's code, and
    /// what suppressions (a future feature) will name. Unique across all
    /// global rules and every scope's rules.
    pub id: String,
    /// The pattern, as written in the config.
    pub pattern: String,
    /// How serious a hit is (default: [`Severity::Warning`]).
    pub severity: Severity,
    /// The message template, parsed and capture-validated.
    pub message: Template,
    /// The compiled pattern — construction only succeeds when this is
    /// valid.
    pub(crate) regex: regex::Regex,
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
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(yaml: &str) -> Result<Self, ConfigError> {
        let value: Value = serde_yaml::from_str(yaml).map_err(|e| {
            let err = ConfigError::new(e.to_string());
            match e.location() {
                Some(loc) => err.with_syntax_location(loc.line(), loc.column()),
                None => err,
            }
        })?;
        Self::from_value(value, yaml)
    }

    /// Loads and validates the config file at `path`.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let shown = path.display().to_string();
        let yaml = std::fs::read_to_string(path).map_err(|e| {
            ConfigError::new(format!("cannot read config file: {e}")).with_path(&shown)
        })?;
        Self::from_str(&yaml).map_err(|e| e.with_path(shown))
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
                    "unsupported config version {version} (this ezlint understands version \
                     {SUPPORTED_VERSION})"
                ),
                None,
            ));
        }

        let rules_value = map.get(Value::from("rules"));
        let scopes_value = map.get(Value::from("scopes"));
        if rules_value.is_none() && scopes_value.is_none() {
            return Err(ConfigError::new("config must define `rules` or `scopes`"));
        }

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

        Ok(Self {
            version,
            rules,
            scopes,
        })
    }
}

/// Compiles a scope boundary pattern with multi-line mode forced on, so
/// `^`/`$` anchor to lines.
fn compile_boundary(pattern: &str) -> Result<regex::Regex, regex::Error> {
    regex::RegexBuilder::new(pattern).multi_line(true).build()
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
            if !matches!(key, "id" | "pattern" | "message" | "severity") {
                return Err(at_rule(format!(
                    "unknown key `{key}` (expected one of `id`, `pattern`, `message`, \
                     `severity`)"
                )));
            }
        }
    }

    let missing = |key: &str| at_rule(format!("missing `{key}` key"));

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

    let pattern_value = map
        .get(Value::from("pattern"))
        .ok_or_else(|| missing("pattern"))?;
    let pattern = pattern_value
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| at_rule("`pattern` must be a non-empty string".into()))?
        .to_string();

    let regex = regex::Regex::new(&pattern)
        .map_err(|e| at_rule(format!("invalid pattern: {e}")))?;

    let message_value = map
        .get(Value::from("message"))
        .ok_or_else(|| missing("message"))?;
    let message_src = message_value
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| at_rule("`message` must be a non-empty string".into()))?;
    let template = Template::parse(message_src)
        .map_err(|e| at_rule(format!("invalid message template: {e}")))?;
    template
        .validate(&regex)
        .map_err(|e| at_rule(format!("invalid message template: {e}")))?;

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
        message: template,
        regex,
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
