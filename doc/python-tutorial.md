# Linting Python with declint

This is **level 2** — if you haven't built the INI linter yet, start with
[`doc/tutorial.md`](tutorial.md) for the basics (rules, templates,
scopes, callbacks, the editor loop). This guide assumes those thirty
minutes and gets straight to work on a language you probably lint every
day.

The honest premise up front: declint's rules are regexes with
per-match Lua callbacks. They see **text, not syntax** — no AST, no
name resolution, no memory between matches. That ceiling is real, and
part 8 maps it precisely. Inside that ceiling there is still a
surprisingly useful subset of what flake8 and pycodestyle do, and
building it teaches every feature declint has.

> **The fast path.** If you just want working rules and not the tour,
> `declint init --lang python` scaffolds a config that imports
> `preset:python` — the finished ruleset from this guide, curated. This
> tutorial rebuilds that preset piece by piece so you can customize it;
> the thirty minutes pay for themselves the first time you need a rule
> nobody ships.

The sample is a small `app.py` — deliberately valid Python (run
`python3 -c "compile(open('app.py').read(), 'app.py', 'exec')"` to
check), but seeded with one instance of every crime:

```python
import os
from collections import *

def fetch(url):
    print("fetching", url)
    if url == None:
        return []
    return os.path.join(url)

def process(items, cache = []):
	try:
		seen = set();
		return [i for i in items if i in cache]
	except:
		return cache

print("script starts")
```

Crimes on display: a wildcard import, a `print` debugging leftover,
`== None`, a mutable default argument, a tab-indented block, a
semicolon, and a bare `except:`. The `print` on the last line is *not*
a crime — scripts are allowed to print; functions are not. Keep that
distinction in mind; part 5 is about it.

Everything below was executed while writing this guide. The finished
setup lives in [`examples/python/`](../examples/python).

## 1. Warm-up: four plain rules

```console
$ mkdir py-linter && cd py-linter
$ cp /path/to/app.py .
```

```yaml
# .declint.yaml
version: 1
languages: [python]
rules:
  - id: no-tabs
    pattern: '(?m)^\t+'
    message: "PEP 8: spaces, not tabs, for indentation"
    severity: warning
  - id: no-semicolons
    pattern: '(?m)^\s*[^#\s].*;\s*$'
    message: "PEP 8: avoid semicolons to separate statements"
    severity: warning
  - id: no-wildcard-imports
    pattern: '(?m)^\s*from\s+[\w.]+\s+import\s+\*'
    message: "wildcard imports make names ambiguous"
    severity: warning
  - id: bare-except
    pattern: '(?m)^\s*except\s*:'
    message: "bare except catches everything — name the exception"
    severity: warning
```

Three notes, all inherited from the INI tutorial: patterns are
single-quoted YAML (backslashes reach the regex engine), `(?m)` makes
`^`/`$` anchor to lines, and `languages: [python]` pins the config to
Python files (`declint check` infers `.py` → `python` from the
extension).

```console
$ declint check app.py
app.py:2:1: warning[no-wildcard-imports]: wildcard imports make names ambiguous
app.py:11:1: warning[no-tabs]: PEP 8: spaces, not tabs, for indentation
app.py:12:1: warning[no-tabs]: PEP 8: spaces, not tabs, for indentation
app.py:12:1: warning[no-semicolons]: PEP 8: avoid semicolons to separate statements
app.py:13:1: warning[no-tabs]: PEP 8: spaces, not tabs, for indentation
app.py:14:1: warning[no-tabs]: PEP 8: spaces, not tabs, for indentation
app.py:14:1: warning[bare-except]: bare except catches everything — name the exception
app.py:15:1: warning[no-tabs]: PEP 8: spaces, not tabs, for indentation
declint: 8 violation(s)
```

(The tab-indented block earns one `no-tabs` per indented line — that is
what a line-anchored rule does. Real linters collapse these; declint
reports each match.)

## 2. Captures: `== None`

```yaml
  - id: compare-to-none
    pattern: '==\s*None\b'
    message: "use 'is None', not '== None'"
    severity: warning
```

```console
$ declint check app.py
app.py:6:12: warning[compare-to-none]: use 'is None', not '== None'
declint: 1 violation(s)
```

(Shown alone by running just this rule — in the full config below it
joins the others.)

## 3. The no-lookaround trick

Somewhere in the codebase, someone commits this:

```python
def broken(data):
    if data = None:
        pass
```

That single `=` is exactly the bug you want caught *before* running
anything. The obvious regex wants a negative lookaround —
`(?<!=)=(?!)` — but the `regex` crate (chosen for linear-time
matching) has no lookaround at all. The workaround is a character
class that excludes the comparison operators from the neighborhood:

```yaml
  - id: assignment-in-condition
    pattern: '(?m)^\s*(?:if|elif|while)\s+[^#=]*[^=!<>]=[^=]'
    message: "single '=' assigns — did you mean '=='?"
    severity: error
```

Reading it: after `if`/`elif`/`while`, some `=`-free run of text, then
a character that is *not* `= ! < >`, then the lone `=`, then anything
but `=`. The exclusions are what keep `==`, `!=`, `<=`, and `>=`
quiet — verify on the comparison zoo:

```console
$ declint check buggy.py
buggy.py:2:1: error[assignment-in-condition]: single '=' assigns — did you mean '=='?
declint: 1 violation(s)
$ declint check fine.py; echo $?
0
```

Where `fine.py` is full of `==`, `<=`, `>=`, and `!=` — none flagged.
Where the approximation *does* bend: conditions wrapped across lines
inside parentheses (the `^\s*(?:if|...)` anchor only sees the first
line). That is the subset talking, and it is why this rule is
`error`-severity but your real linter still exists.

## 4. Scopes: top-level functions

Now the interesting one: `print()` is fine at the top level of a script
and a crime inside a function. That is a *region* rule, and Python's
indentation makes the regions easy: a top-level function starts at a
line beginning with `def ` and ends at the next line that starts with
any non-whitespace character:

```yaml
scopes:
  - id: function
    start: '^def\s'
    end: '^\S'
    rules:
      - id: print-in-function
        pattern: '\bprint\('
        message: "leftover print() — remove or use logging"
        severity: warning
```

Recall the scope mechanics from the INI tutorial: `start`/`end` are
compiled with multi-line mode **forced on** (unlike rule patterns,
which is why the `print-in-function` rule still carries its own
`(?m)`-free form — it only needs to match *within* a region). Regions
are sequential; a new `def` ends the previous function's region; the
last region runs to end of file.

```console
$ declint check app.py
app.py:5:5: warning[print-in-function]: leftover print() — remove or use logging
app.py:11:1: warning[no-tabs]: PEP 8: spaces, not tabs, for indentation
...
declint: 6 violation(s)
```

The `print` inside `fetch` (line 5) is flagged; the top-level
`print("script starts")` (line 17) is silent — it lives outside every
region. That asymmetry is the whole point: rules-as-config could never
express "here but not there" before scopes.

## 5. A callback: mutable default arguments

The most famous Python gotcha:

```python
def process(items, cache = []):
```

That `[]` is created **once**, at definition time, and shared by every
call. It needs a real decision (not just a message), and while we're
at it, an escape hatch. Note `rest` — the regex deliberately captures
the rest of the line so the callback can look for a suppression
marker:

```yaml
  - id: mutable-default
    pattern: '(?m)^\s*def\s+(?<function>\w+)\s*\([^)]*=[ \t]*(?<default>\[\]|\{\})(?<rest>[^\n]*)'
    severity: error
    callback: |
      return function(c)
        if c.captures.rest:find("declint:allow") then
          return nil
        end
        return { message = "default " .. c.captures["default"] .. " in '" ..
                 c.captures["function"] .. "' is shared across calls" }
      end
```

```console
$ declint check app.py
app.py:9:1: error[mutable-default]: default [] in 'process' is shared across calls
declint: 1 violation(s)
```

And the escape hatch:

```python
def process(items, cache = []):  # declint:allow
```

```console
$ declint check app.py; echo $?
0
```

Two Lua details worth their own sentences. Capture groups become
`c.captures` — named groups by name, but `function` is a Lua
*keyword*, so dot access (`c.captures.function`) is a syntax error;
use `c.captures["function"]`. And callbacks are pure: one match per
call, no memory of the last. Counting duplicate parameter names, for
example, is out of bounds.

## 6. The editor

The config already says `languages: [python]`. The Neovim wiring is
the same two-step as always — match the filetypes to the config:

```lua
require('lspconfig.configs').declint = {
  default_config = {
    cmd = { 'declint', 'serve' },
    filetypes = { 'python' },
    root_dir = vim.fn.getcwd,
  },
}
require('lspconfig').declint.setup({})
```

Open `app.py` and the violations are squiggles; the mutable-default
error carries its callback-computed message; fix a line and it clears
on the next keystroke batch.

One more time with feeling: **declint complements ruff and flake8,
it does not replace them.** Your `.declint.yaml` covers the
house-style extras those tools will never know about — your
suppression marker, your print policy, your naming rules — in a file
that took twenty minutes to write and runs in the same process as your
editor.

## Using (and customizing) the preset

Everything this guide built — plus a little curation — ships as
`preset:python`, embedded in the binary. A config that uses it is four
lines:

```yaml
# .declint.yaml
version: 1
languages: [python]
import:
  - preset:python
```

The preset is a curated superset of the tutorial's ruleset: the
function scope handles `async def`, the warm-up rules are
battle-tested, and the mutable-default callback ships ready-made. Show
it any time with `declint presets python` — it is ordinary YAML, and
`declint init --lang python` writes the four-line config for you.

**Customizing** follows from ids being unique: to change how a preset
rule behaves, copy it out of the preset into your own `rules:` list and
edit the copy — then it can't be imported again, so also drop the
`import:` entry if you've forked every rule you care about:

```yaml
version: 1
languages: [python]
rules:
  # forked from preset:python — our house severity, our message
  - id: mutable-default
    pattern: '(?m)^\s*def\s+(?<function>\w+)\s*\([^)]*=[ \t]*(?<default>\[\]|\{\})(?<rest>[^\n]*)'
    severity: warning          # the preset uses error; we are softer
    callback: |
      return function(c)
        if c.captures.rest:find("declint:allow") then return nil end
        return { message = "'" .. c.captures["function"] .. "' shares its " ..
                 c.captures["default"] .. " across calls" }
      end
```

Two rules of the road:

* Keep the `import:` for everything you *don't* fork — preset upgrades
  flow to every rule you didn't copy.
* If you see `duplicate rule id 'mutable-default' (defined in
  preset:python and …)` in an error, you have the rule in both places:
  remove one.

## 7. The ceiling, precisely

Every limit below is a design fact, not a missing feature — though some
walls moved in 0.5.0. Knowing them tells you when to reach for
something else:

| You want... | Status | Reach for |
|---|---|---|
| Lookaround (`(?<!=)`) | regex rules can't; **parser rules** can hand-roll any matching | parser rules, or `fancy-regex` upstream |
| Duplicate-key / duplicate-def detection | regex rules can't count; **parser rules** see the whole file at once | parser rules (see the README), or a parser for structure |
| "Every function must have a docstring" | sub-rules fire on *matches*; **parser rules** can emit violations for non-matches | parser rules, or a real AST linter (ruff) |
| Brace-matched regions (JS functions) | scope `end:` is a regex, not a paren matcher — a parser rule can *check* brace depth, but scopes themselves stay regex-cut | indentation-based languages, or rigid formatting (`end: '^\}'`) |
| Type-aware rules | regex and Lua see bytes, not types | pyright/mypy, full stop |

The pattern to notice: parser rules push the ceiling back by putting
*you* in charge of matching — but the code is still per-file text
analysis in Lua. Name resolution, import graphs, type inference: those
are parsers, not patterns, and that's the border of this tool.

## Where to go from here

* **Shortcuts:** `declint init --lang python` scaffolds this ruleset
  via `import: [preset:python]` — the preset also carries an
  `async def`-aware function scope.
* The **README** has the full reference — config schema, scope
  semantics, the callback contract, parser rules, discovery order.
* [`examples/python/`](../examples/python) is this guide's finished
  setup: `cd` in and run `declint check app.py`.
* The rules here would sit happily next to the INI ruleset from
  [`doc/tutorial.md`](tutorial.md) in one `.declint/` directory —
  that is what per-language configs are for.
