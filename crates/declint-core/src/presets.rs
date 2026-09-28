//! The embedded preset library: curated config fragments shipped with
//! declint, imported as `import: preset:<name>`.
//!
//! The presets are ordinary YAML config files living in this crate's
//! `presets/` directory (so they ship inside the published package) and
//! embedded at compile time — binaries are self-contained, and CI needs
//! nothing beyond the executable.

/// One embedded preset.
pub struct Preset {
    /// The name used in `import: preset:<name>`.
    pub name: &'static str,
    /// One-line summary shown by `declint presets`.
    pub description: &'static str,
    /// The full YAML content.
    pub content: &'static str,
}

/// Every embedded preset, in display order.
pub static PRESETS: &[Preset] = &[
    Preset {
        name: "python",
        description: "Python: PEP 8 warm-ups, == None, mutable defaults, \
                      docstrings, top-level function/class scopes",
        content: include_str!("../presets/python.yaml"),
    },
    Preset {
        name: "ini",
        description: "INI: tabs, trailing whitespace, empty values, \
                      duplicate keys, a [server] section example",
        content: include_str!("../presets/ini.yaml"),
    },
    Preset {
        name: "markdown",
        description: "Markdown: tabs, trailing whitespace, bare URLs",
        content: include_str!("../presets/markdown.yaml"),
    },
    Preset {
        name: "sh",
        description: "Shell: rm with variables, sudo, [ x == y ], \
                      missing shebang, strict-mode suggestion",
        content: include_str!("../presets/sh.yaml"),
    },
    Preset {
        name: "dockerfile",
        description: "Dockerfile: unpinned/latest FROM, sudo in RUN, \
                      ADD vs COPY, apt cleanup, missing HEALTHCHECK",
        content: include_str!("../presets/dockerfile.yaml"),
    },
    Preset {
        name: "toml",
        description: "TOML: tabs, trailing whitespace, empty values, \
                      spacing, duplicate keys per section",
        content: include_str!("../presets/toml.yaml"),
    },
    Preset {
        name: "json",
        description: "JSON: trailing commas, single-quoted strings, \
                      unquoted keys, comments",
        content: include_str!("../presets/json.yaml"),
    },
    Preset {
        name: "javascript",
        description: "JavaScript: == vs ===, var declarations, \
                      leftover console.log",
        content: include_str!("../presets/javascript.yaml"),
    },
];

/// Looks up an embedded preset by name.
pub fn lookup(name: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|preset| preset.name == name)
}

/// The names of every embedded preset, comma-separated — for error
/// messages ("unknown preset `x` (available: python, ini, markdown)").
pub fn names() -> String {
    PRESETS
        .iter()
        .map(|preset| preset.name)
        .collect::<Vec<_>>()
        .join(", ")
}
