//! Loading a whole configuration *site*: a hidden file (`.declint.yaml`)
//! or a hidden directory of per-language configs (`.declint/*.yaml`),
//! found by walking up from a starting directory.

use std::path::{Path, PathBuf};

use crate::config::{Config, ConfigError};
use crate::config::Scope;

/// The hidden config file: `.declint.yaml`.
pub const CONFIG_FILE: &str = ".declint.yaml";

/// The hidden config directory: `.declint/` (every `*.yaml` inside is a
/// config).
pub const CONFIG_DIR: &str = ".declint";

/// The legacy, non-hidden config file, still accepted last.
pub const LEGACY_CONFIG_FILE: &str = "declint.yaml";

/// One config file loaded as part of a [`ConfigSet`].
#[derive(Debug, Clone)]
pub struct NamedConfig {
    /// The file the config came from (used in error messages).
    pub path: PathBuf,
    /// The validated config.
    pub config: Config,
}

/// A set of config files loaded together — the whole `.declint.yaml`, or
/// everything in a `.declint/` directory.
///
/// Ids must be unique *within* one file, but different files may reuse
/// them: only the configs whose `languages` match a document ever apply
/// to it, so diagnostic codes stay unambiguous per document.
#[derive(Debug, Clone, Default)]
pub struct ConfigSet {
    configs: Vec<NamedConfig>,
}

/// One scope from a [`ConfigSet`], with its position in the set: which
/// config it belongs to and its index within that config.
#[derive(Debug, Clone)]
pub struct ScopeEntry {
    /// The scope itself.
    pub scope: Scope,
    /// Index into [`ConfigSet::configs`].
    pub config: usize,
    /// The scope's index within its own config.
    pub local: usize,
}

impl ConfigSet {
    /// A set holding exactly one config (no file path attached).
    pub fn single(config: Config) -> Self {
        Self {
            configs: vec![NamedConfig {
                path: PathBuf::new(),
                config,
            }],
        }
    }

    /// Loads `path` as a config site: a file is one config; a directory
    /// is every `*.yaml`/`*.yml` inside it (in file-name order).
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let shown = path.display().to_string();
        if path.is_dir() {
            let mut files: Vec<PathBuf> = std::fs::read_dir(path)
                .map_err(|e| {
                    ConfigError::new(format!("cannot read config directory: {e}"))
                        .with_path(&shown)
                })?
                .filter_map(|entry| entry.ok().map(|e| e.path()))
                .filter(|p| {
                    matches!(p.extension().and_then(|e| e.to_str()), Some("yaml") | Some("yml"))
                })
                .collect();
            files.sort();
            if files.is_empty() {
                return Err(ConfigError::new(format!(
                    "no `.yaml` or `.yml` config files in `{shown}`"
                )));
            }
            let mut configs = Vec::with_capacity(files.len());
            for file in files {
                let config = Config::load(&file)?;
                configs.push(NamedConfig { path: file, config });
            }
            return Ok(Self { configs });
        }
        let config = Config::load(path)?;
        Ok(Self {
            configs: vec![NamedConfig { path: path.to_path_buf(), config }],
        })
    }

    /// Finds and loads the config site for `start`, walking up through
    /// parent directories. At each directory the search order is
    /// [`CONFIG_FILE`], [`CONFIG_DIR`], [`LEGACY_CONFIG_FILE`]; the first
    /// hit is loaded.
    pub fn discover(start: &Path) -> Result<Self, ConfigError> {
        let start = if start.is_dir() {
            start
        } else {
            start.parent().unwrap_or(Path::new("."))
        };
        for dir in start.ancestors() {
            for candidate in [dir.join(CONFIG_FILE), dir.join(CONFIG_DIR)] {
                if candidate.is_file() || candidate.is_dir() {
                    return Self::load(&candidate);
                }
            }
            let legacy = dir.join(LEGACY_CONFIG_FILE);
            if legacy.is_file() {
                return Self::load(&legacy);
            }
        }
        Err(ConfigError::new(format!(
            "no declint config found (looked for {CONFIG_FILE}, {CONFIG_DIR}/, \
             {LEGACY_CONFIG_FILE} in `{}` and its parents)",
            start.display()
        )))
    }

    /// The loaded configs, in load order.
    pub fn configs(&self) -> &[NamedConfig] {
        &self.configs
    }

    /// Every scope across every config, with its global position: `entry.config`
    /// indexes [`ConfigSet::configs`] and `entry.local` indexes that
    /// config's `scopes`.
    pub fn scope_table(&self) -> Vec<ScopeEntry> {
        let mut out = Vec::new();
        for (config_index, named) in self.configs.iter().enumerate() {
            for (local, scope) in named.config.scopes.iter().enumerate() {
                out.push(ScopeEntry {
                    scope: scope.clone(),
                    config: config_index,
                    local,
                });
            }
        }
        out
    }
}
