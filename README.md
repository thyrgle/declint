# ezlint

A YAML-configured regex linter and language server, built on
[increparse](../increparse). Describe lint rules in a small YAML file and get
both an **LSP server** (`ezlint serve`, diagnostics in your editor as you
type) and a **CLI** (`ezlint check`, for CI) — no Rust required.

```yaml
version: 1
rules:
  - id: no-tabs
    pattern: '\t+'
    message: "Use spaces, found '{match}'"
    severity: warning
```

## Quick start

```sh
cargo install --path crates/ezlint

# In CI: lint files, print violations, exit 1 if any were found.
ezlint check src/**

# In your editor: run a language server over stdio.
ezlint serve           # reads ./ezlint.yaml
```

### Editor setup

Any LSP client that lets you launch a custom stdio server works.

**Neovim** (with nvim-lspconfig):

```lua
require('lspconfig.configs').ezlint = {
  default_config = {
    cmd = { 'ezlint', 'serve' },
    filetypes = { 'text', 'markdown', 'ini', 'conf' },  -- whatever you lint
    root_dir = vim.fn.getcwd,
  },
}
require('lspconfig').ezlint.setup({})
```

**VS Code**: point a generic LSP client extension (e.g. any
"custom language server" extension) at `ezlint serve` with the filetypes you
want.

## Config reference

The config file is `ezlint.yaml` by default (both subcommands take an
explicit path too).

```yaml
version: 1                # required; this ezlint understands version 1
rules:
  - id: rule-name         # required; unique; becomes the diagnostic's code
    pattern: '\t+'        # required; Rust `regex` crate syntax (no lookaround)
    message: "..."        # required; see templates below
    severity: warning     # optional: error | warning | info | hint (default warning)
```

Everything is validated at load time — regex syntax, message placeholders,
duplicate ids, unknown keys — and a bad config is rejected with the rule id
and file line of the problem, e.g.:

```text
ezlint: rules.yaml:5: rule 0 ('no-tabs'): invalid pattern: repetition operator missing expression
```

### Message templates

| Placeholder | Meaning |
|-------------|---------|
| `{match}`   | the text the pattern matched |
| `{name}`    | the text of named capture group `name` — `(?<word>\w+)` |
| `{{` / `}}` | literal braces |

Unknown placeholder names are a config error, caught at load time.

### Regex flavor

Patterns use the [`regex`](https://docs.rs/regex) crate: linear-time, no
catastrophic backtracking, named groups via `(?<name>...)`. Lookaround and
backreferences are not supported in v1.

## Workspace layout

| Crate | Role |
|-------|------|
| [`crates/ezlint-core`](crates/ezlint-core) | Config loading/validation, message templates, the regex lint engine. No LSP dependencies. |
| [`crates/ezlint-lsp`](crates/ezlint-lsp)   | The `serve()` language server: violations → LSP diagnostics via `increparse-lsp`. |
| [`crates/ezlint`](crates/ezlint)           | The CLI binary (`serve` / `check`). |

`ezlint-lsp::language(config)` also embeds into your own server if you
already have an `increparse-lsp`-based one.

## Design notes

Linting here is a flat regex scan per document revision, so the parse tree
stays trivial (one region, immediately done) — what ezlint reuses from
increparse/`increparse-lsp` is the plumbing: incremental change
translation, byte↔UTF-8/16/32 position conversion, document bookkeeping,
and diagnostics publishing. Violations are merged into the same
`publishDiagnostics` stream as parse diagnostics via the
`SimpleLanguage::extra_diagnostics` hook.

Diagnostics carry `source: "ezlint"` and `code: <rule-id>`, so clients can
filter, style, and (later) suppress per rule.

## Roadmap

- v2: rule callbacks (Rust snippets with a compile cache, and Lua), `fix:`
  templates → LSP CodeActions, inline `# ezlint:disable=<id>` comments,
  per-rule file globs, config discovery up the directory tree.

## Status

v0.1.0 — regex rules only; the schema is versioned to keep future configs
compatible.
