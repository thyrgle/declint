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

5. Tag and push:

   ```sh
   git tag v0.6.0 && git push origin v0.6.0
   ```

6. Update the composite `action.yml` default `declint-version` input to
   the new release.

## Notes

- `declint-lua` vendors Lua C sources via mlua's `vendored` feature, so
  publishing needs no system Lua, but *building* it requires a C
  compiler (CI runners have one).
- New crates.io accounts must verify their email before `cargo publish`
  works.
- If a publish fails mid-way, re-running `cargo publish` for the failed
  crate is safe; crates.io rejects exact duplicates.
