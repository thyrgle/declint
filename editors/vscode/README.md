# declint for VS Code

YAML-configured linting in VS Code: diagnostics as you type, quickfixes,
powered by your `.declint.yaml` rules and the `declint` language server.

## Setup

1. Install the binary once:

   ```sh
   cargo install declint
   ```

2. Add a config site to your project (`.declint.yaml`, `.declint/`, or
   `declint.yaml`) — [README](../../README.md) has the schema; presets
   come from `declint init --lang <lang>` and
   [`gh:`-installable rulesets](../../README.md#sharing-rulesets).
3. Install this extension.

The linter attaches to the languages in the `declint.filetypes`
setting — match them to the `languages:` keys of your configs, or
configs without a `languages` key apply to everything.

## Settings

| Setting | Default | Meaning |
|---|---|---|
| `declint.path` | `declint` | Path to the executable |
| `declint.args` | `["serve"]` | Arguments for the language server |
| `declint.filetypes` | broad list | Languages the client attaches to |

## Troubleshooting

* **"failed to start"** — the binary is missing or not on PATH: check
  `declint --version` in a terminal, or point `declint.path` at it.
* **No diagnostics?** Run `declint check .` in the project — if the
  CLI sees violations but the editor doesn't, your config's
  `languages` keys don't include the file's language id.
* `declint serve` discovers configs exactly like the CLI: config site
  search walks up from the file's directory.
