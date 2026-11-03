# AGENTS.md — Gelbooru Downloader

Rust 2024 edition async CLI/TUI downloader. Working dir: /opt/gelbooru-downloader.

## Build / Test

- `cargo build` (dev), `cargo build --release`
- `cargo test` (unit + integration)
- `cargo clippy` (optional)
- Binary name: `gelbooru` (target/debug/gelbooru)

## Architecture

Modules under src/: cli, api, downloader, database, filters, presets, batch, watch, repl, tui, config, utils.

- config: TOML + env overrides, XDG dirs.
- api: reqwest client, models, search pagination.
- filters: client-side FilterRules.
- downloader: DownloadEngine with semaphore concurrency, streaming to .part, atomic rename, blake3 hash, validation.
- database: rusqlite history, presets, watch state.
- tui: ratatui.

## Conventions

- Errors: utils::error::AppError / Result.
- Async DB via spawn_blocking.
- No placeholders; keep tests passing.
- Never commit secrets; API creds via env/CLI/config.

## Repository maintenance

Keep this repository deliberately simple: one branch, one binary, no extra
process.

- Work directly on `main`. Do not create feature, release, or long-lived
  branches unless the user explicitly asks for one.
- Push in coherent batches: after a feature or fix is complete and all checks
  pass. Do not push partial work or every small edit.
- Before pushing:
  - `cargo fmt --check`
  - `cargo test`
  - `cargo build --release`
- Never force-push `main`.
- Keep the tree small. `/target`, databases, downloads, preview caches, and
  local config are ignored; never commit generated artifacts or secrets.
- Bump the version in `Cargo.toml` for releases, then tag `vX.Y.Z` and push the
  tag. Create a GitHub release for meaningful versions only.
- Do not add CI, extra tooling, or new documentation files unless they solve a
  concrete problem; extend the existing files instead.
- Commit messages: short imperative summary, e.g. `Add viewer quick download`.

## Notes

- Network available for cargo; offline cache present.
- Tests use wiremock.
