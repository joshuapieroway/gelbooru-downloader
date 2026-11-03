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

## Notes

- Network available for cargo; offline cache present.
- Tests use wiremock.
