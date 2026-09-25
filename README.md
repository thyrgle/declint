# declint

A YAML-configured regex linter and language server, built on
[increparse](../increparse). Describe lint rules in a small YAML file and get
both an **LSP server** (`declint serve`, diagnostics in your editor as you
type) and a **CLI** (`declint check`, for CI) — no Rust required.

```yaml
version: 1
rules:
  - id: no-tabs
    pattern: '\t+'
    message: "Use spaces, found '{match}'"
    severity: warning
```

**New to declint?** [`doc/tutorial.md`](doc/tutorial.md) builds an INI
linter step by step — first rules, scoped sections, Lua callbacks, and
the editor payoff — in about 30 minutes. For a second course,
[`doc/python-tutorial.md`](doc/python-tutorial.md) does the same for a
Python ruleset: warm-up rules, the no-lookaround tricks, function
scopes, and the mutable-default-arguments callback.

## Quick start

```sh
cargo install --path crates/declint

# In CI: lint files, print violations, exit 1 if any were found.
declint check src/**

# In your editor: run a language server over stdio.
declint serve           # reads ./declint.yaml
```

### Neovim setup

Any LSP client that launches a custom stdio server works. **Neovim**
(with nvim-lspconfig):

```lua
require('lspconfig.configs').declint = {
  default_config = {
    cmd = { 'declint', 'serve' },                -- discovers .declint.yaml / .declint/ itself
    filetypes = { 'markdown', 'sh' },           -- match your configs' languages
    root_dir = vim.fn.getcwd,
  },
}
require('lspconfig').declint.setup({})
```

The `filetypes` list should match the `languages:` keys in your configs —
Neovim sends the filetype as the LSP `languageId`, and declint only
publishes diagnostics from configs whose `languages` match it. A file of
any other filetype gets a clean (empty) diagnostic publish.

### VS Code

Point a generic LSP client extension at `declint serve` with the filetypes
you want.

## Config discovery

With no explicit path, both `declint serve` and `declint check` look for a
**config site** starting at the current directory and walking up through
parents. At each directory the search order is:

1. `.declint.yaml` — a single hidden config file
2. `.declint/` — a hidden directory; **every `*.yaml` inside is a config**
   (a natural layout for per-language rule files: `markdown.yaml`,
   `sh.yaml`, ...)
3. `declint.yaml` — the legacy, non-hidden name, still accepted

The first hit wins. Ids must be unique *within* one config file, but
different files in a `.declint/` directory may reuse them — only one
language's configs apply to any given document, so codes stay
unambiguous.

## Config reference

The config file is `declint.yaml` by default (both subcommands take an
explicit path too).

```yaml
version: 1                # required; this declint understands version 1
languages: [markdown, sh] # optional; editor language ids this config applies
                          # to (exact match; missing = all languages)
rules:                    # file-global rules; optional if `scopes` is present
  - id: rule-name         # required; unique; becomes the diagnostic's code
    pattern: '\t+'        # required; Rust `regex` crate syntax (no lookaround)
    message: "..."        # required unless `callback` is present; see templates
    severity: warning     # optional: error | warning | info | hint (default warning)
    callback: |           # optional; Lua snippet or `checks/foo.lua` path,
      return function(c)  # see "Callbacks" below
        return nil
      end
scopes:                   # optional; see "Scoped rules" below
  - id: scope-name        # unique; shares a namespace with rule ids
    start: '...'          # required; starts a region (multi-line `^`/`$`)
    end: '...'            # optional; ends a region (default: end of file)
    rules: [...]          # rules that run only inside this scope's regions
```

Everything is validated at load time — regex syntax, message placeholders,
duplicate ids, unknown keys — and a bad config is rejected with the rule id
and file line of the problem, e.g.:

```text
declint: rules.yaml:5: rule 0 ('no-tabs'): invalid pattern: repetition operator missing expression
```

### Message templates

| Placeholder | Meaning |
|-------------|---------|
| `{match}`   | the text the pattern matched |
| `{name}`    | the text of named capture group `name` — `(?<word>\w+)` |
| `{{` / `}}` | literal braces |

Unknown placeholder names are a config error, caught at load time.

### Callbacks

For decisions a message template can't express, a rule can run a **Lua
callback** instead of (or alongside) a `message`. The `callback:` value is
either an inline block scalar or a path to a `.lua` file (relative to the
config); the snippet must `return function(ctx) ... end`:

```yaml
rules:
  - id: todo-ticket
    pattern: 'TODO:[ \t]*(?<ticket>\S*)'
    severity: warning
    callback: |
      return function(ctx)
        if ctx.captures.ticket == "" then
          return { severity = "warning", message = "TODO without a ticket" }
        end
        return nil          -- cites a ticket -> allowed
      end
```

The callback receives one table — `match`, `captures` (named and
`"1"`-numbered), `start`/`finish` (byte offsets), `line`/`col` (1-based),
`path`, `language`, `rule` — and decides:

| Return | Meaning |
|--------|---------|
| `nil` / `false` | **allowed** — no diagnostic for this match |
| `true` | violate, using the rule's own `message` template |
| `{ message = "...", severity = "..." }` | violate with overrides (`severity` optional) |
| thrown Lua error | an `error`-severity diagnostic names the rule; the run continues |

Every call runs under an instruction budget, so a runaway loop fails as a
diagnostic instead of hanging the editor. Snippets are compiled when the
config loads — a syntax error is a config error with the rule id.

Rust embedders can skip Lua entirely: implement
`declint_core::MatchCallback` and register it by name
(`Callbacks::register("my_check", ...)`), then reference it in YAML as
`callback: my_check`.

### Scoped rules

A `scopes` entry segments the file and runs its rules **only inside those
regions** — lint's version of a pass that expands, then subpasses that run
per region:

```yaml
version: 1
scopes:
  - id: shell-fence
    start: '^```sh$'   # where a region begins (required)
    end: '^```$'       # where it ends (optional; default: end of file)
    rules:
      - id: no-sudo
        pattern: '\bsudo\b'
        message: "Don't use sudo in scripts"
        severity: error
```

* `start`/`end` are compiled with **multi-line mode forced**, so `^`/`$`
  anchor to lines.
* Regions are sequential within a scope; different scopes may overlap
  freely (each family is independent).
* Rule and scope ids share one namespace and must be unique across the
  whole config.
* In the editor, an edit inside one region re-lints only that region —
  every other region keeps its identity in the parse tree.

See [`examples/scoped-rules.yaml`](examples/scoped-rules.yaml) for a full
config.

### CLI language handling

`declint check` decides each file's language in this order:

1. `--language <id>` — always wins (e.g. `--language markdown`)
2. The file's extension, via a built-in table of common Neovim filetype
   names (`md` → `markdown`, `py` → `python`, `sh` → `sh`, ...)
3. Unknown → only configs **without** a `languages` key apply

### Regex flavor

Patterns use the [`regex`](https://docs.rs/regex) crate: linear-time, no
catastrophic backtracking, named groups via `(?<name>...)`. Lookaround and
backreferences are not supported in v1.

## Workspace layout

| Crate | Role |
|-------|------|
| [`crates/declint-core`](crates/declint-core) | Config loading/validation, message templates, the regex lint engine. No LSP dependencies. |
| [`crates/declint-lsp`](crates/declint-lsp)   | The `serve()` language server: violations → LSP diagnostics via `increparse-lsp`. |
| [`crates/declint`](crates/declint)           | The CLI binary (`serve` / `check`). |

`declint-lsp::language(config)` also embeds into your own server if you
already have an `increparse-lsp`-based one.

## Design notes

Linting here is a flat regex scan per document revision, so the parse tree
stays trivial (one region, immediately done) — what declint reuses from
increparse/`increparse-lsp` is the plumbing: incremental change
translation, byte↔UTF-8/16/32 position conversion, document bookkeeping,
and diagnostics publishing. Violations are merged into the same
`publishDiagnostics` stream as parse diagnostics via the
`SimpleLanguage::extra_diagnostics` hook.

Diagnostics carry `source: "declint"` and `code: <rule-id>`, so clients can
filter, style, and (later) suppress per rule.

## Roadmap

- v2: nested scopes (a scope inside a scope), callback `range` overrides,
  per-rule instruction budgets, `fix:` templates → LSP CodeActions,
  inline `# declint:disable=<id>` comments, per-rule file globs.

## Status

0.4.0 — Lua match callbacks (inline and file), hidden configs with
directory discovery, per-language configs, scoped rules; the schema is
versioned to keep future configs compatible.
