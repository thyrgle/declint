# Architecture

declint is a YAML-configured linter and language server, built on
[increparse](../increparse). This document maps the workspace: what
each crate does, how a lint run flows through them, and the invariants
that hold everywhere. For usage, start at the [README](README.md); for
the rules themselves, the [tutorials](doc/tutorial.md).

## The crates

```text
                ┌─────────────────────────────┐
                │ declint (bin)               │  CLI: check / serve /
                │  clap, walking, formatters  │  presets / init
                └───────┬─────────────┬───────┘
                        │             │
        ┌───────────────▼──┐   ┌──────▼────────┐
        │ declint-lsp      │   │ declint-lua   │
        │ serve(), LSP     │   │ Lua callbacks │
        │ plumbing         │   │ and parsers   │
        └───────┬──────────┘   └──────┬────────┘
                │       ┌─────────────┘
                ▼       ▼
        ┌──────────────────────────────┐
        │ declint-core                 │  engine: config discovery and
        │ config, imports, presets,    │  merging, linting, violations
        │ Linter, matchers, registry   │
        └──────────────────────────────┘
                        │
                        ▼
                ┌───────────────┐
                │ increparse    │  (sibling repo): the parse engine
                └───────────────┘  beneath increparse-lsp
```

| Crate | Role | Depends on |
|-------|------|------------|
| `declint-core` | Config discovery/merging, presets registry, the lint engine, violation model. No LSP, no Lua. | `regex`, `serde_yaml` |
| `declint-lua` | Compiles and runs Lua `callback:`/`parser:` snippets (mlua, sandboxed, instruction budgets). | `declint-core`, `mlua` |
| `declint-lsp` | Turns linters into a language server: `serve()`, document plumbing, diagnostics. | `declint-core`, `increparse-lsp` |
| `declint` | The binary: `check` (CLI), `serve` (LSP), `presets`, `init`. | all of the above |

## A lint run, end to end

```text
config site discovery          .declint.yaml → .declint/ → declint.yaml,
        │                      walking up from the working directory
        ▼
ConfigSet                      each file parsed and validated: schema,
        │                      regexes compiled, imports resolved
        │                      depth-first (preset: or relative paths,
        │                      cycle detection), fragments merged,
        │                      one id namespace across the merge
        ▼
Linter::new(config, &Callbacks)   every rule's callback/parser reference
        │                         resolved against the registry (Lua's
        │                         attach() fills it; Rust hosts register
        │                         by name) — unregistered = config error
        ▼
per file:                      language filter (config `languages` vs the
        │                      file's language id), read, DocInfo{path,
        │                      language}
        ▼
lint_all_in                    scope segmentation (regex start/end cut
        │                      regions) → for each rule: find phase
        │                      (regex captures or parser call) → decide
        │                      phase (callback decision or template
        │                      render) → Violation{rule_id, severity,
        │                      span, message}
        ▼
sorted Violations              by (start, end, rule_id) — deterministic
        │
        ▼
consumers                      CLI: text or GitHub workflow commands;
                               LSP: publishDiagnostics via increparse-lsp
```

## Invariants

These hold everywhere and are what make the pieces safely composable:

* **Callbacks and parsers are pure.** Same input, same output; no state
  between matches. Consequence: duplicate counting needs a *parser*
  rule (whole text at once), and absence rules emit violations for
  non-matches.
* **Sandboxed by budget.** Every Lua call runs under an instruction
  budget (1M for callbacks, 10M for parsers). A runaway rule becomes an
  `error`-severity diagnostic; it never hangs the editor or CI.
* **Config errors are fatal and precise.** A config is either fully
  usable or rejected, with the rule id and file line of the problem —
  regexes are compiled, placeholders checked, ids kept unique at load
  time.
* **One id namespace per config file.** Imported fragments merge into
  the importer's namespace; collisions across imports are errors. Only
  one language's configs apply to any document, so per-language files
  may reuse ids.
* **Language policy lives at the top.** Imported presets and fragments
  never declare `languages`; the importing config does.
* **Output is deterministic.** Violations are sorted by position, then
  span end, then rule id — regardless of rule or config order.
* **Zero-width matches are noise** and always skipped.

## Extension points

* **Rust embedders** implement `MatchCallback` / `MatchParser` and
  register them by name — YAML references `callback: name` /
  `parser: name`. No Lua required.
* **Presets are data.** The library in `crates/declint-core/presets/`
  is ordinary YAML, embedded at compile time and importable as
  `preset:<name>`; teams share their own files the same way.
* **`SimpleLanguage::extra_diagnostics`** (from increparse-lsp) is the
  seam where lint violations join parse diagnostics — embed declint in
  a larger language server with one hook.

## Cross-repo notes

`increparse` (sibling directory) is the multi-pass parsing engine
beneath `increparse-lsp`, which provides declint's document plumbing,
position encodings, and diagnostics publishing. Publishing order and
the version coupling between the workspaces are documented in
[`PUBLISHING.md`](PUBLISHING.md) and
[`../increparse/PUBLISHING.md`](../increparse/PUBLISHING.md).
