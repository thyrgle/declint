# declint

A YAML-configured regex linter and language server, built on
[increparse](../increparse). Describe lint rules in a small YAML file and get
both an **LSP server** (`declint serve`, diagnostics in your editor as you
type) and a **CLI** (`declint check`, for CI) — no Rust required. The
internal layout is summarized in [`ARCHITECTURE.md`](ARCHITECTURE.md).

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

# Fastest start: scaffold a config from a preset (python, ini, markdown).
declint init --lang ini
declint presets              # list the embedded preset library

# In CI: lint everything under the current directory, annotate the PR,
# fail the job only on error-severity violations.
declint check . --format github --fail-on error

# Shell completions (bash; zsh/fish/powershell also supported).
source <(declint completions bash)

# In your editor: run a language server over stdio.
declint serve           # discovers .declint.yaml / .declint/ itself
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
import:                   # optional; merge other configs' rules+scopes in
  - preset:python         #   embedded presets (see "Imports & presets")
  - ./team-scopes.yaml    #   or any relative YAML file
languages: [markdown, sh] # optional; editor language ids this config applies
                          # to (exact match; missing = all languages)
rules:                    # file-global rules; optional if `scopes` is present
  - id: rule-name         # required; unique; becomes the diagnostic's code
    pattern: '\t+'        # required unless `parser` is present; Rust `regex`
                          # syntax (no lookaround)
    parser: |             # instead of `pattern`: a Lua function that sees
      return function(text, offset)  # the whole file — see "Parser rules"
        return nil
      end
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

### Parser rules

Regex rules match one pattern at a time and can't remember previous
matches. When a rule needs the whole file in hand — counting duplicates,
flagging absent constructs, hand-rolled matching — replace `pattern`
with a `parser`: a Lua function called **once per scan unit** that
returns every match. (The two keys are mutually exclusive; one is
required.)

```yaml
rules:
  - id: duplicate-keys
    parser: |
      return function(text, offset)
        -- text: the whole file (or the scope region, for scoped rules)
        -- offset: text's absolute byte position in the file
        local seen, matches = {}, {}
        local pos = 1
        while pos <= #text do
          local nl = text:find("\n", pos, true) or (#text + 1)
          local line = text:sub(pos, nl - 1)
          local key = line:match("^%s*([%w-]+)%s*=")
          if key and seen[key] then
            matches[#matches + 1] = {
              start = offset + pos - 1, finish = offset + pos - 1 + #line,
              captures = { key = key, count = tostring(seen[key] + 1) },
            }
          end
          if key then seen[key] = (seen[key] or 0) + 1 end
          pos = nl + 1
        end
        return matches
      end
    message: "'{key}' defined {count} times"
    severity: error
```

The contract:

* Return `nil` (no matches) or a list of
  `{ start = , finish = , captures = { name = "..." } }` tables, with
  positions **relative to `text`**; they are converted to absolute
  positions for you.
* Each returned match then flows through the standard pipeline — its
  `captures` interpolate into the message template and are visible to
  the rule's `callback` (if any) as `c.captures`.
* Parsers run under a larger instruction budget than callbacks (10
  million instructions per call). Prefer `string.find`/`string.gmatch`
  — they execute at C speed; per-character Lua loops do not.
* Like callbacks, parsers must be pure: same text in, same matches out.
* Rust embedders: implement `declint_core::MatchParser` and register it
  with `Callbacks::register_parser("name", ...)`, then reference
  `parser: name` in YAML.

Parser rules close the gaps the tutorials' "ceiling" sections describe:
duplicates, absence rules ("every function must have a docstring"), and
custom matching that no regex dialect could express.

### Testing your rules

Rules are programs — pin their behavior with embedded fixtures, run by
`declint test`:

```yaml
rules:
  - id: port-range
    pattern: '(?m)^\s*port[ \t]*=[ \t]*(?<port>\S+)'
    severity: error
    tests:
      - name: out-of-range port
        text: "[server]\nport = 99999\n"
        violations: 1
        messages: ["port must be 1-65535, found 99999"]
      - name: valid port passes
        text: "[server]\nport = 8000\n"
        violations: 0
```

Each test runs only that rule over `text` (scopes included — scoped
rules test their scoped behavior), then checks the violation count and
that each expected `messages` entry appears. `declint test` runs every
embedded test in the discovered configs — imported presets included —
and exits 1 on any failure. `declint explain <file>` shows the other
half of the picture: which configs applied, which rules resolved, and
what matched in a given file.

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

## Imports & presets

Configs can pull in other configs — and declint ships a small library of
curated presets, embedded in the binary:

```yaml
version: 1
languages: [python]        # language policy belongs to THIS config
import:
  - preset:python          # embedded preset (declint presets — list them)
  - ./team-scopes.yaml     # any relative YAML config
rules:
  # your own rules, merged after the imports
```

* Imported `rules:`/`scopes:` concatenate before the importer's own;
  all ids share one namespace, and duplicates across imports are a
  config error naming both sources.
* Imported configs must not declare `languages` — only the top-level
  config does.
* Relative paths resolve against the importing file; imports may nest,
  with cycle detection.
* `declint presets` lists the library; `declint presets python` prints
  the YAML; `declint init --lang python` scaffolds a config that
  imports it.

Shipped presets: `python` (PEP 8 warm-ups, mutable defaults, top-level
function scopes), `ini` (tabs, trailing whitespace, empty values, a
`[server]` scope example), `markdown` (tabs, trailing whitespace, bare
URLs).

## Sharing rulesets

Rule sets are plain YAML configs in git repos — anyone's repo can be a
ruleset. To use one:

```console
$ declint install gh:thyrgle/declint-rules
vendored .declint/vendor/thyrgle/declint-rules/HEAD/declint.yaml
wired into .declint.yaml
```

* **Project installs** vendor the files under
  `.declint/vendor/<owner>/<repo>/<ref>/` and wire the import into your
  config. The files are ordinary repo content: review them in the PR,
  commit them, and CI never touches the network — same commit, same
  lints, forever. Pin with `@<tag-or-sha>` for extra precision
  (unpinned installs print a warning).
* **Global installs** (`declint install -g gh:owner/repo`) vendor into
  `~/.declint/store/` instead, for personal rules available everywhere.
  Import them from any config with `import: [global:owner/repo]`.
  Global installs float — re-running install updates them — so prefer
  project installs for anything shared or audited.

The fetched config's own relative `import:` entries are fetched too, so
multi-file rule sets work. And the trust note, plainly: **rulesets may
contain Lua callbacks and parsers** — installing one is running code.
Vendor-first exists so you read exactly what runs, exactly once, before
it ever executes.

`declint install` requires network access; linting the vendored copy
does not.

### Inline suppressions

Put `declint:disable` in a line to silence every rule on that line, or
`declint:disable=no-tabs,trailing-whitespace` to silence specific rules —
in whatever comment syntax the language uses:

```ini
port = 99999 ; declint:disable=port-range
password = hunter2 # declint:disable-next-line
```

`declint:disable-next-line` covers the following line instead. The
suppression applies to the line's violations only — nothing global, and
the markers work in any language because they are matched as plain text.

### Fixes

Rules with a `fix:` template are auto-repairable: the template replaces
each match, interpolating captures like messages.

```yaml
rules:
  - id: compare-to-none
    pattern: '==\s*None'
    message: "use 'is None', not '== None'"
    fix: 'is None'
```

* **In CI and the terminal:** `declint check --fix` applies every
  available fix in place (non-overlapping, position-ordered) and fails
  only for the violations that remain.
* **In the editor:** rules with fixes become quickfix code actions —
  the lightbulb offers "declint: apply fix for …" on the offending
  lines.

## Continuous integration

`declint check` is CI-shaped: `file:line:col: severity[id]: message`
output, exit 0 clean / 1 violations / 2 broken setup. Pass
`--format github` and every violation becomes a GitHub Actions workflow
command — inline annotations right on the pull request:

```console
$ declint check --format github .
::error file=app.ini,line=10,col=1,endLine=10::[declint/port-range] port must be 1-65535, found 99999
```

Directory arguments are walked recursively (`.gitignore` is respected,
hidden paths and non-UTF-8 files skipped), so the whole incantation for
a repository is `declint check .` with the config discovery you already
use locally. Two knobs for CI:

* `--fail-on error|warning|info|hint` — all violations are still
  reported (and annotated), but only those at or above the threshold
  fail the job. The default is `hint`: any violation fails.
* Lua callback `print()`s route to stderr — debug freely, the
  annotation stream stays clean.

**As a workflow step** (annotates the PR, fails the job on violations):

```yaml
jobs:
  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: thyrgle/declint@v0        # the composite action in this repo
        with:
          path: .
          declint-version: "0.8.0"
```

**Or by hand**, without the action:

```yaml
jobs:
  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo install declint --locked
      - run: declint check . --format github
```

The config comes from the repository being linted — the same
`.declint.yaml` / `.declint/` your editors use. Lua callbacks and parser
rules run sandboxed with instruction budgets, so a looping rule fails a
diagnostic instead of hanging the job.

### Regex flavor

Patterns use the [`regex`](https://docs.rs/regex) crate: linear-time, no
catastrophic backtracking, named groups via `(?<name>...)`. Lookaround and
backreferences are not available in regex rules — for matching beyond
that, use a [parser rule](#parser-rules).

**CRLF files:** `$` in multi-line mode anchors before `\n` — a `\r`
left by Windows line endings sits between your match and the anchor.
End-of-line patterns should tolerate it (`[ \t]+\r?$`). Scope
boundaries are CRLF-safe automatically: their multi-line mode also
enables CRLF anchors.

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

0.9.0 — declint test (embedded rule fixtures), declint explain (config/rule visibility for a file), --format json. 0.8.0 — declint install (gh: rulesets vendored into the project or the global store, importable as global:<pkg>). 0.7.1 — Lua print() routes to stderr, CRLF-safe scope boundaries, --fail-on severity thresholds, shell completions. 0.7.0 — imports & presets (preset:python/ini/markdown, declint init, declint presets), Lua parser rules (whole-file matchers for duplicates, absence
rules, and custom matching), Lua match callbacks, hidden configs with
directory discovery, per-language configs, scoped rules; the schema is
versioned to keep future configs compatible.
