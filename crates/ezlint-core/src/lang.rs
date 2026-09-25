//! Guessing a file's editor language id from its extension.
//!
//! The names are common Neovim filetypes, which is what a Neovim LSP
//! client sends as `languageId` — the same vocabulary as a config's
//! `languages` key. `ezlint check --language` always wins over this
//! table; files whose extension is unknown get no language, and only
//! configs without a `languages` key apply to them.

use std::path::Path;

/// The language id for `path`'s extension, or `None` when unknown.
pub fn language_from_extension(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let language = match ext.as_str() {
        "md" | "markdown" => "markdown",
        "py" | "pyi" | "pyw" => "python",
        "rb" => "ruby",
        "sh" | "bash" | "zsh" => "sh",
        "fish" => "fish",
        "ps1" | "psm1" => "powershell",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" | "tsx" => "typescript",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "cs" => "cs",
        "rs" => "rust",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "dart" => "dart",
        "zig" => "zig",
        "lua" => "lua",
        "vim" => "vim",
        "nix" => "nix",
        "php" => "php",
        "pl" | "pm" => "perl",
        "r" => "r",
        "jl" => "julia",
        "hs" => "haskell",
        "ex" | "exs" => "elixir",
        "erl" | "hrl" => "erlang",
        "clj" | "cljs" | "cljc" | "edn" => "clojure",
        "sql" => "sql",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" => "scss",
        "json" | "jsonc" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "ini" | "cfg" | "conf" | "properties" => "ini",
        "xml" => "xml",
        "txt" | "text" | "log" => "text",
        _ => return None,
    };
    Some(language.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lang(name: &str) -> Option<String> {
        language_from_extension(Path::new(name))
    }

    #[test]
    fn common_extensions() {
        assert_eq!(lang("README.md").as_deref(), Some("markdown"));
        assert_eq!(lang("app.py").as_deref(), Some("python"));
        assert_eq!(lang("run.sh").as_deref(), Some("sh"));
        assert_eq!(lang("main.rs").as_deref(), Some("rust"));
        assert_eq!(lang("conf.ini").as_deref(), Some("ini"));
        assert_eq!(lang("x.toml").as_deref(), Some("toml"));
    }

    #[test]
    fn case_and_paths() {
        assert_eq!(lang("/a/b/NOTES.MD").as_deref(), Some("markdown"));
        assert_eq!(lang("archive.TAR.GZ"), None);
    }

    #[test]
    fn unknown_extensions() {
        assert_eq!(lang("Makefile"), None);
        assert_eq!(lang("data.bin"), None);
        assert_eq!(lang("no_extension"), None);
    }
}
