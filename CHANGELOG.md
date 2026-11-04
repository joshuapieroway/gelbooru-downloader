# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.1] - 2026-11-03

Compare: [`v0.2.0...v0.2.1`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/v0.2.0...v0.2.1)

A follow-up fix for the TUI download queue: several posts can be queued at
once, the configured concurrency is enforced across all of them, and progress
stays visible while the viewer is idle.

### Fixed

- The TUI viewer now shares one download engine across queued posts, so
  concurrent downloads respect `downloads.concurrency` instead of each
  starting an independent engine and client.
- Download progress refreshes while the viewer is idle, so multiple active
  downloads remain visible without pressing a key.
- Posts stay marked as queued until a concurrency slot is available; only
  active transfers show as downloading.
- Pressing `d` on a post that is already queued or downloading no longer
  starts a duplicate download.
- Queue errors now always resolve to a terminal `failed` or `cancelled` state.

## [0.2.0] - 2026-11-03

Compare: [`v0.1.2...v0.2.0`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/v0.1.2...v0.2.0)

A performance and responsiveness release. CLI downloads now report real byte
progress, the TUI viewer keeps working while downloads run in the background,
and previews load faster through connection reuse and neighbor prefetching.

### Added

- Byte-based progress bars for `gelbooru download <id>`,
  `gelbooru download <tags>` and `gelbooru preset run`, showing percentage,
  transferred/total bytes, transfer speed and completed-file count. When the
  API reports no sizes the bar falls back to file counts.
- TUI download queue: `d` queues a download and returns control to the viewer
  immediately. The status bar shows progress for up to three active posts and
  summarises any extra entries as `(+N more)`.
- GIF previews in kitty terminals, which were previously blocked with a
  "preview disabled" message.
- Background prefetching of the previous and next previews, bounded to three
  concurrent downloads and six tracked tasks. Navigating to a prefetched post
  displays it from the cache instead of waiting on the network.
- Sample/full-resolution reporting in the viewer, including a hint that `d`
  fetches the full image when a sample is shown.
- Tests for byte-based progress, preview cache validation, unique temporary
  paths, shared client reuse, prefetch reuse and atomic preview writes.

### Changed

- Preview downloads reuse one pooled HTTP client with keep-alive and four idle
  connections per host instead of creating a client per image.
- Preview cache writes use unique `<file>.<pid>.<seq>.part` temporary files
  followed by an atomic rename, so background and on-demand fetches cannot
  collide or expose partially written images.
- Default download concurrency raised from 8 to 16.
- Download HTTP pool tuned to the configured concurrency, with a 30-second idle
  timeout and 10-second TCP keepalive. The request timeout dropped from 300 to
  120 seconds.
- Retry backoff is now a capped 100 ms → 1 s → 10 s sequence instead of
  2 s → 4 s → 8 s, making transient failures recover faster.
- Resuming a partial download hashes the existing `.part` file in 8 KiB chunks
  instead of reading the entire file into memory.
- History records the URL that actually produced the file, including sample
  fallbacks.
- Viewer previews use the full image up to 1667 px and the sample above that,
  keeping large images fast to open.

### Fixed

- The TUI viewer no longer freezes while a quick download runs; navigation,
  tag scrolling and previews stay responsive.
- HTTP 404 and 500/503 download errors now include a plain-language explanation
  and a suggested next step instead of a bare status code.
- Completed, skipped, duplicate and failed downloads all update their queue
  entry, keeping the status bar accurate.
- Cancelled and failed downloads remove their `.part` files.

## [0.1.2] - 2026-11-03

Compare: [`v0.1.1...v0.1.2`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/v0.1.1...v0.1.2)

### Added

- Unlimited backward and forward pagination in the full-screen viewer: moving
  past either end of the current page fetches the adjacent API page, so browsing
  is no longer limited to the search results already loaded.

### Fixed

- Search-list scrolling stays within the final result on a page.

## [0.1.1] - 2026-11-02

Compare: [`v0.1.0...v0.1.1`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/v0.1.0...v0.1.1)

### Fixed

- Clamped search-list scrolling to the last result on the current page.

### Documentation

- Documented the simple one-branch maintenance and release workflow.

## [0.1.0] - 2026-11-02

Initial release.

### Added

- Async Gelbooru API client with search, pagination and post lookup.
- Concurrent downloader with resume support, atomic `.part` writes, file
  validation, blake3 hashing and duplicate detection.
- SQLite-backed download history and reusable presets.
- Watch mode, batch downloads, an interactive REPL and client-side filters.
- Full-screen ratatui TUI with a searchable post list and image viewer.
- TOML configuration with environment and CLI overrides using XDG directories.
