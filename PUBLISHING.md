# Publishing checklist

The declint workspace publishes four crates to crates.io in dependency
order. This document is the runbook.

## One-time setup

- [ ] Create a crates.io account and `cargo login`.
- [ ] Verify `repository` in every `crates/*/Cargo.toml` points at
      `https://github.com/thyrgle/declint` (and that the repository is
      public, or crates.io will just show a dead link).
- [ ] Optional: `rustup install stable` is enough; no nightly needed.

## Per release

0. **Prerequisite:** the `increparse` workspace (the sibling directory)
   must already be published — `declint-lsp` depends on `increparse`
   and `increparse-lsp` from crates.io. If versions changed there,
   update the requirements in `crates/declint-lsp/Cargo.toml` first.
   See `../increparse/PUBLISHING.md`.
1. Bump versions everywhere (`crates/*/Cargo.toml` — the four crates
   share a version) and update intra-workspace version requirements.
2. `cargo test --workspace && cargo clippy --workspace --all-targets`
   must be clean.
3. Dry-run each crate and eyeball the payload:

   ```sh
   cargo package -p declint-core --list
   cargo package -p declint-lua --list
   cargo package -p declint-lsp --list
   cargo package -p declint --list
   ```

4. Publish in dependency order (each `--allow-dirty` only if you know
   what you are doing):

   ```sh
   cargo publish -p declint-core
   cargo publish -p declint-lua
   cargo publish -p declint-lsp
   cargo publish -p declint
   ```

5. Tag and push (the README's `uses: thyrgle/declint@v1.0.0` example
   resolves against this tag; a moving `v0` major tag is a nice extra):

   ```sh
   git tag v1.0.0 && git push origin v1.0.0
   git tag -f v0 && git push origin v0 --force
   ```

6. Update the composite `action.yml` default `declint-version` input to
   the new release.
7. Publish the VS Code extension:

   ```sh
   cd editors/vscode
   npx --yes @vscode/vsce package --allow-missing-repository
   npx --yes @vscode/vsce publish            # needs an Azure DevOps PAT
   npx --yes ovsx publish --pat <token>      # Open VSX (VSCodium etc.)
   ```

   Both tokens are free: a PAT from an Azure DevOps organization
   (`dev.azure.com` -> User settings -> Personal access tokens, scope
   Marketplace -> Manage) and an Open VSX token from
   `open-vsx.org` -> Settings -> Access Tokens. The marketplace
   display name is registered with the first publish ("declint" is
   taken there — the current listing name works around that; only the
   display name changes, never the `uses:`/extension id).

## Notes

- The GitHub Marketplace listing is **optional** — `uses:
  thyrgle/declint@<tag>` works from any repo with `action.yml` at its
  root, no listing needed. If you ever want the storefront, the
  marketplace display name must be unique across the marketplace
  ("declint" is taken there; "declint-ci" was the earlier idea) — that
  only changes the listing name, never the `uses:` reference.

- `declint-lua` vendors Lua C sources via mlua's `vendored` feature, so
  publishing needs no system Lua, but *building* it requires a C
  compiler (CI runners have one).
- New crates.io accounts must verify their email before `cargo publish`
  works.
- If a publish fails mid-way, re-running `cargo publish` for the failed
  crate is safe; crates.io rejects exact duplicates.
