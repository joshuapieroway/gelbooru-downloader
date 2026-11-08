# TODO — gelbooru-downloader

Project version: 0.5.0 (2026-11-08)
Source of truth for planned work. Update as items ship; move done items to the bottom.

## High Priority — sort/filter parity

- [x] Add `--sort` and `--order` to `batch`, `preset run`, and `watch` for parity with `download`/`search` (currently `sort: None` in `src/batch/mod.rs`, `src/presets/mod.rs`, `src/watch/mod.rs`)
- [x] Add `--rating` and `--limit` passthrough to `batch` and `watch` (currently hardcoded to `None` / default)
- [x] Validate `--sort` / `--order` values and fail fast with a helpful message instead of silently falling back to `id` in `src/api/search.rs`
- [ ] Add `--sort` / `--order` to `repl download` for full parity (currently `sort: None` in `src/repl/mod.rs`)
- [ ] Add `gelbooru download --dry-run` to list resolved posts without transferring files
- [ ] Clean up stale persistent-queue rows: surface `queue list` age and prune rows older than N days

## Medium Priority — reliability & tests

- [ ] Replace `unwrap()` / `expect()` in production paths (149 occurrences) with `AppError` propagation
- [ ] Add coverage for `batch`, `watch`, `preset`, and `repl` integration paths (currently only API + downloader are integration-tested)
- [ ] Harden filename/folder templates: reject empty expanded names and surface a warning when every placeholder renders empty
- [ ] Add config schema validation with friendly errors for bad TOML values (concurrency, page_size, templates)
- [ ] Add bandwidth limiting (`downloads.bandwidth_limit_mbps`) and per-file pause/cancel in the TUI Queue tab
- [ ] Verify interrupted-download resume hashes correctly when `.part` is truncated or corrupted (add a test)

## Low Priority / Future

- [ ] Document sort values and template placeholders in `--help` long-help (`score`, `id`, `width`, `height`, `created`, `rating`)
- [ ] Add TUI sort control (cycle `score`/`id`/`date`) and limit control alongside the existing 25/50/100 page cycle
- [ ] Export history with `--format json` in addition to CSV; include `sort`/`order` used for the search
- [ ] Reduce TUI `viewer.rs` size (2451 lines) by extracting preview cache and tag-panel rendering
- [ ] Add a `gelbooru doctor` / `gelbooru status` command that reports config, DB size, queue depth, and disk usage

## Done

- [x] Async Gelbooru API client (search, pagination, post lookup)
- [x] Concurrent downloader with resume, atomic `.part` writes, blake3 hashing, validation, duplicate detection
- [x] SQLite history, presets, watch state, and persistent queue
- [x] Watch mode, batch downloads, REPL, client-side filters
- [x] Full-screen ratatui TUI with searchable list and image viewer
- [x] TOML config with env/CLI overrides and XDG dirs
- [x] Filename/folder templates (`{id}`, `{score}`, `{score_raw}`, `{md5}`, `{ext}`, etc.)
- [x] Byte-based progress bars, preview prefetching, per-job queue tracking
- [x] Restore `download --sort` / `--order` (v0.4.2) — removed in v0.4.0, now available again for tag downloads
- [x] Sort/order parity + validation for `batch`, `watch`, `preset create/run` (v0.5.0) — `validate_sort_order` fails fast with allowed values
