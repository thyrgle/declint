# Your first linter in 30 minutes

This guide builds a **linter** for INI files — no Rust, no plugin code, no
build step. Just a YAML file that grows with you: two regex rules, then
named captures, then per-section rules, then Lua callbacks, and finally a
language server running in your editor. (Building a *parser* instead?
The sibling project [`increparse`](../../increparse) has a quickstart for
that — and it uses the same sample file.)

The target is the classic INI format:

```ini
[owner]
name = Ada
born = 1815

[database]
enabled = true
```

Everything here is runnable — every command below was executed while
writing this guide. The finished setup lives in
[`examples/ini/`](../examples/ini) if you want to compare your work.

## 1. Setup and first rules

Install the CLI:

```console
$ cargo install --path crates/declint
```

Make a scratch project and give it a slightly messy INI file — note the
stray tab at the end of line 2 and the trailing spaces on line 3
(invisible in print, very visible to a linter):

```console
$ mkdir my-linter && cd my-linter
$ printf '[owner]\nname = Ada\t\nborn = 1815   \n\n[database]\nenabled = true\n' > app.ini
```

declint finds its config by walking up from the current directory:
`.declint.yaml`, then `.declint/` (a directory of configs), then
`declint.yaml`. Create the hidden file with two rules:

```yaml
# .declint.yaml
version: 1
rules:
  - id: no-tabs
    pattern: '\t+'
    message: "Tab character — use spaces"
    severity: warning
  - id: trailing-whitespace
    pattern: '(?m)[ \t]+$'
    message: "Trailing whitespace"
    severity: hint
```

Two things to notice before running:

* **Patterns are single-quoted YAML strings.** In single quotes YAML
  leaves backslashes alone, so `'\t+'` reaches the regex engine as
  `\t+`. Double quotes would make YAML try to interpret `\t` itself.
* The `(?m)` prefix turns on multi-line mode, so `$` matches at the end
  of *every line*, not just the end of the file. Rule patterns start
  from scratch — only scope boundaries (part 3) get multi-line mode for
  free.

Now lint:

```console
$ declint check app.ini
app.ini:2:11: warning[no-tabs]: Tab character — use spaces
app.ini:2:11: hint[trailing-whitespace]: Trailing whitespace
app.ini:3:12: hint[trailing-whitespace]: Trailing whitespace
declint: 3 violation(s)
$ echo $?
1
```

Each line is `file:line:col: severity[id]: message` — the id in brackets
is the rule's `id`. Exit code 1 means "violations found" (CI-friendly:
0 clean, 1 violations, 2 broken setup). Fix the file — delete the tab
and the trailing spaces — and run again:

```console
$ printf '[owner]\nname = Ada\nborn = 1815\n\n[database]\nenabled = true\n' > app.ini
$ declint check app.ini; echo $?
0
```

## 2. Captures and templates

Time passes; a teammate pushes two crimes against INI: an empty value
and a capital-B boolean:

```ini
[owner]
name = Ada
born = 1815
nickname =

[database]
enabled = True
```

Add two rules that use **named capture groups** and interpolate them
into messages with `{name}` placeholders:

```yaml
  - id: empty-value
    pattern: '(?m)^\s*(?<key>[\w-]+)[ \t]*=[ \t]*$'
    message: "'{key}' has no value"
    severity: warning
  - id: boolean-case
    pattern: '(?m)^\s*[\w-]+[ \t]*=[ \t]*(?<value>True|False|TRUE|FALSE)\s*$'
    message: "booleans are lowercase, found '{value}'"
    severity: info
```

```console
$ declint check app.ini
app.ini:4:1: warning[empty-value]: 'nickname' has no value
app.ini:7:1: info[boolean-case]: booleans are lowercase, found 'True'
declint: 2 violation(s)
```

`{match}` is also available (the whole matched text), and `{{` / `}}`
escape literal braces. Placeholder names are checked when the config
loads — as are the patterns themselves:

```console
$ declint check --config bad.yaml app.ini
declint: bad.yaml:3: rule 0 ('oops'): invalid pattern: regex parse error:
    (
    ^
error: unclosed group
```

A config is either fully usable or rejected, with the rule id and the
config-file line of the problem. The same strictness catches unknown
keys — try `patern:` and read the correction.

## 3. Sections as scopes

Regexes see the whole file. To make rules apply *only inside a region*,
declare a **scope**: it cuts the file into pieces (pass 1), and its
`rules` run inside each piece (the subpasses). Give the sample a
`[server]` section:

```ini
[owner]
name = Ada
born = 1815

[database]
enabled = true

[server]
port = not-a-number
```

A naive first try — a rule that flags *every* `port` line inside
`[server]`:

```yaml
scopes:
  - id: server
    start: '^\[server\]$'
    end: '^\['
    rules:
      - id: port-numeric
        pattern: '(?m)^\s*port[ \t]*=[ \t]*(?<port>\S*)'
        message: "port must be a number, found '{port}'"
        severity: error
```

* `start` is where a region begins, `end` where it stops — here, at the
  next section header. Omit `end` and regions run to end of file.
* Scope boundaries are compiled with multi-line mode **forced on**, so
  `^` and `$` anchor to lines without writing `(?m)`.
* A scope's regions are sequential; a new region starts at each `start`
  match.

```console
$ declint check app.ini
app.ini:4:1: warning[empty-value]: 'nickname' has no value
app.ini:10:1: error[port-numeric]: port must be a number, found 'not-a-number'
declint: 2 violation(s)
```

To see the scoping actually work, put `port = oops` under `[owner]` and
re-run: nothing fires for it. Only the `[server]` region is searched by
the `server` scope's rules.

But notice what the rule *can't* do: it fired on `port = not-a-number`
and it would fire just as happily on `port = 8000` — the message
template can show the value, but nothing checked it. Message templates
are strings, not programs. For actual validation you need a callback.

## 4. Lua callbacks

Replace `port-numeric` with a rule whose `callback` is a Lua function.
The snippet must `return function(ctx) ... end`; the function receives
one table per match and decides the outcome:

```yaml
scopes:
  - id: server
    start: '^\[server\]$'
    end: '^\['
    rules:
      - id: port-range
        pattern: '(?m)^\s*port[ \t]*=[ \t]*(?<port>\S+)'
        severity: error
        callback: |
          return function(c)
            local n = tonumber(c.captures.port)
            if n == nil then
              return { message = "port must be a number, found '" .. c.captures.port .. "'" }
            end
            if n < 1 or n > 65535 then
              return { message = "port must be 1-65535, found " .. n }
            end
            return nil
          end
```

The contract:

| Return | Meaning |
|--------|---------|
| `nil` / `false` | **allowed** — no diagnostic for this match |
| `true` | violate, using the rule's `message` template (that's why `message` is now optional on callback rules) |
| `{ message = "...", severity = "..." }` | violate with overrides; `severity` falls back to the rule's |
| thrown Lua error | surfaces as an `error`-severity diagnostic naming the rule — the run continues |

The context table has `match` (the matched text), `captures` (named
groups by name, unnamed as `captures["1"]`, ...), `start`/`finish`
(byte offsets), `line`/`col` (1-based), and `path`, `language`, `rule`.

Push the sample around to see both branches:

```console
$ declint check app.ini          # port = 99999 in [server]
app.ini:10:1: error[port-range]: port must be 1-65535, found 99999
declint: 1 violation(s)
$ sed -i 's/port = 99999/port = 8000/' app.ini
$ declint check app.ini; echo $?
0
```

Callbacks can also **allow** matches, which is how you write allow-list
rules. A global rule that wants kebab-case keys but grandfathers one
legacy name:

```yaml
rules:
  - id: kebab-case-keys
    pattern: '(?m)^\s*(?<key>[\w-]+)[ \t]*='
    severity: warning
    callback: |
      return function(c)
        local key = c.captures.key
        if key == key:lower() then return nil end
        if key == "mySQL" then return nil end
        return { message = "key '" .. key .. "' should be kebab-case" }
      end
```

```console
$ declint check app.ini          # serverName = db1 in [server]
app.ini:9:1: warning[kebab-case-keys]: key 'serverName' should be kebab-case
declint: 1 violation(s)
$ sed -i 's/serverName = db1/mySQL = db1/' app.ini
$ declint check app.ini; echo $?
0
```

Two safety rails, so a bad callback can't ruin your day:

* **Syntax errors** are caught when the config loads, as config errors
  with the rule id and file.
* **Runaway loops** are cut off by an instruction budget per call
  (currently one million instructions) and reported as a diagnostic;
  a callback that throws reports its Lua error the same way. The lint
  run itself never crashes.

Callbacks are pure — they see one match at a time and can't remember
previous ones. Counting duplicate keys, for example, is beyond a
regex-per-match engine by design; that's a job for a parser (see
[increparse](../../increparse)).

## 5. Point it at a language, run it in your editor

So far `declint check` applies every rule to every file. To pin this
config to INI files, add a `languages` key — its values are editor
language ids, matched exactly against the document's `languageId`
(in Neovim: the filetype):

```yaml
version: 1
languages: [ini]
rules:
  # ... everything from above ...
```

On the command line the language comes from `--language`, or is guessed
from the extension (`.ini` maps to `ini` already), or — when unknown —
only configs *without* a `languages` key apply.

Now the payoff. Wire the language server into Neovim
([nvim-lspconfig](https://github.com/neovim/nvim-lspconfig)):

```lua
require('lspconfig.configs').declint = {
  default_config = {
    cmd = { 'declint', 'serve' },   -- discovers .declint.yaml / .declint/ itself
    filetypes = { 'ini' },          -- match your configs' languages
    root_dir = vim.fn.getcwd,
  },
}
require('lspconfig').declint.setup({})
```

Open `app.ini` and the diagnostics appear as you type — squiggles in
the gutter, messages on hover, `code` set to the rule id so plugin
authors can filter. Edit a violation away and the diagnostic clears on
the next keystroke batch; an edit inside one region only re-lints that
region. (VS Code: any generic LSP client extension pointed at
`declint serve` works — see the README.)

## 6. Growing up: the `.declint/` directory

One file per language scales better than one file for everything. The
hidden *directory* form holds a config per language — delete
`.declint.yaml` and split:

```text
.declint/
├── ini.yaml        # languages: [ini] — everything from part 4
└── markdown.yaml   # languages: [markdown] — no-tabs + a bare-url rule
```

```yaml
# .declint/markdown.yaml
version: 1
languages: [markdown]
rules:
  - id: no-tabs
    pattern: '\t+'
    message: "Tab character — use spaces"
    severity: warning
  - id: bare-url
    pattern: '(?m)https?://\S+$'
    message: "bare URL — wrap it in angle brackets"
    severity: hint
```

Then lint mixed projects in one pass — each file gets only its own
language's configs:

```console
$ declint check app.ini NOTES.md
app.ini:9:1: warning[kebab-case-keys]: key 'serverName' should be kebab-case
NOTES.md:3:30: hint[bare-url]: bare URL — wrap it in angle brackets
declint: 2 violation(s)
```

Note that `no-tabs` exists in *both* files. Rule ids must be unique
within one config file, but different files in `.declint/` may reuse
them — only one language's configs ever apply to a given document, so
`code` stays unambiguous.

## Where to go from here

* **Level 2:** [`python-tutorial.md`](python-tutorial.md) repeats the
  arc on a real language — a mini-flake8 with function scopes, the
  no-lookaround workarounds, and the mutable-default-arguments
  callback.
* The **README** has the full config reference: template escapes, scope
  semantics, discovery order, the callback contract, and CLI flags.
* [`examples/ini/`](../examples/ini) holds the finished setup from this
  guide — `cd` in and run `declint check app.ini NOTES.md`.
* Rules with a taste for structure? That's a parser's job:
  [`increparse`](../../increparse) builds incremental, error-tolerant
  parsers and language servers, and this project's rules would be its
  lint layer.
