# TODO — gelbooru-downloader

Project version: 0.6.0 (2026-11-11)
Source of truth for planned work. Update as items ship; move done items to the bottom.

## High Priority — sort/filter parity

- [x] Add `--sort` and `--order` to `batch`, `preset run`, and `watch` for parity with `download`/`search` (currently `sort: None` in `src/batch/mod.rs`, `src/presets/mod.rs`, `src/watch/mod.rs`)
- [x] Add `--rating` and `--limit` passthrough to `batch` and `watch` (currently hardcoded to `None` / default)
- [x] Validate `--sort` / `--order` values and fail fast with a helpful message instead of silently falling back to `id` in `src/api/search.rs`
- [ ] Add `--sort` / `--order` to `repl download` for full parity (currently `sort: None` in `src/repl/mod.rs`)
- [ ] Add `gelbooru download --dry-run` to list resolved posts without transferring files
- [ ] Clean up stale persistent-queue rows: surface `queue list` age and prune rows older than N days

## Medium Priority — reliability & tests

- [x] Replace mutex lock `.unwrap()` with safe recovery from poisoning in `src/database/history.rs` (21 locations) and `src/tui/app.rs` (10 locations)
- [x] Fix HTTP client builder error handling consistency in `src/downloader/engine.rs` - now uses `map_err()` like `api/client.rs`
- [x] Improve JSON deserialization error handling in `src/database/history.rs::queue_row()` with tracing warning
- [x] Fix cursor visibility bug in `src/tui/viewer.rs` - cursor now shows after image display (line 1629: changed `Hide` to `Show`)
- [x] Fix hash-based duplicate detection bug: removing overly-aggressive post-download hash check that deleted newly downloaded files when a different file with the same hash existed in another directory. Now only the pre-download `post_id` and file-path checks apply for deduplication.
- [x] Fix post_id duplicate check to only skip when existing path is in the same directory as the target (prevents skipping downloads to different output directories)
- [x] Add regression test `same_post_downloaded_to_different_directory_is_not_skipped` to verify the fix
- [ ] Add coverage for `batch`, `watch`, `preset`, and `repl` integration paths (currently only API + downloader are integration-tested)
- [ ] Harden filename/folder templates: reject empty expanded names and surface a warning when every placeholder renders empty
- [ ] Add config schema validation with friendly errors for bad TOML values (concurrency, page_size, templates)
- [ ] Add bandwidth limiting (`downloads.bandwidth_limit_mbps`) and per-file pause/cancel in the TUI Queue tab
- [ ] Verify interrupted-download resume hashes correctly when `.part` is truncated or corrupted (add a test)

## Low Priority / Future

- [ ] Document sort values and template placeholders in `--help` long-help (`score`, `id`, `width`, `height`, `created`, `rating`)
- [ ] Add TUI sort control (cycle `score`/`id`/`date`) and limit control alongside the existing 25/50/100 page cycle
- [ ] Export history with `--format=json` in addition to CSV; include `sort`/`order` used for the search
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
- [x] Propagate `engine.sort`/`engine.order` in `batch`, `watch`, `preset run` so `{sort_prefix}` works there (v0.5.1) — removed spurious `order=desc` when no `--sort` given, hardened `{character}` fallback