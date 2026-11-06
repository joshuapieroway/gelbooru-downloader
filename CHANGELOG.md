# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.5] - 2026-11-05

Compare: [`v0.2.4...v0.2.5`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/v0.2.4...v0.2.5)

Fixes the viewer footer so a finished quick download no longer keeps showing
its queued message next to the saved result.

### Fixed

- The TUI viewer clears `Queued download for post #N...` and
  `Post #N is already downloading` once that post finishes.

## [0.2.4] - 2026-11-05

Compare: [`v0.2.3...v0.2.4`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/v0.2.3...v0.2.4)

A correctness and polish release: search limits now count matching posts, batch
and watch errors stay visible, and downloads no longer corrupt or lose files in
several edge cases.

### Added

- `download`, `batch`, and `preset run` honour the global `--json` flag and
  print structured results.
- `--quiet` suppresses progress bars and the search result total.
- The REPL gains `download <id|tags>` and `presets`; `config` prints TOML.
- Batch searches run concurrently, report failures per tag, and exit non-zero
  when any tag group fails.
- Watch retries failed downloads on later polls instead of skipping them.
- The TUI quits with `Ctrl+C` and shows the result of a finished quick download
  next to the live queue.

### Changed

- `--limit` counts posts that pass the configured filters, and the API page
  size is capped at 100 so paging offsets stay correct.
- CLI `--limit` and `watch --interval` fall back to the configured defaults;
  the TUI cycles only through 25, 50, and 100 results per page.
- TUI page changes restart an active search on page 1.
- Download, history, and queue sizes use consistent binary units.
- The `safe` default rating is sent as `rating:general`; an explicit
  `rating:` tag or `--rating all` disables the default.

### Fixed

- Overlapping batch tags or repeated post ids no longer race on the same
  `.part` file.
- Falling back from a failed full-size download to a sample no longer resumes
  the sample against the wrong partial bytes.
- A hash duplicate no longer deletes the only remaining copy when the old
  history path is missing.
- Manual redirects are capped, and an unexpected `206` response no longer
  duplicates the start of a file.
- TUI tag truncation no longer panics on multibyte UTF-8, and slow search
  responses cannot overwrite newer results.
- History migration is transactional and recovers a stranded
  `downloads_legacy` table.
- History CSV export includes a header, escapes commas/quotes/newlines, and
  honours `--search`.
- `Post::extension()` reads only the URL path, not the hostname or query.
- `WatchConfig::default()` now uses the same 300-second interval as the config
  file default.

## [0.2.3] - 2026-11-04

Compare: [`v0.2.2...v0.2.3`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/v0.2.2...v0.2.3)

A follow-up to the concurrent queue fix: live jobs stay in the Queue tab, and
each row reports its own speed and ETA.

### Added

- The TUI Queue tab shows per-job transfer speed and ETA, derived from the
  previous poll for that job's record.
- Active downloads refresh their progress record every two seconds, so a long
  transfer is no longer mistaken for an abandoned process and removed from the
  queue after the 10-second stale timeout.
- `ETA` values are formatted as `45s`, `1m 30s` or `2h 2m`.

### Fixed

- Queue tab statistics (`Live`, `Speed`, `Failed`, `Skipped`) update again
  after per-job progress tracking replaced the shared record.
- Finished jobs are dropped from the queue as soon as their progress record
  disappears, without leaving stale speed or ETA values behind.

## [0.2.2] - 2026-11-03

Compare: [`v0.2.1...v0.2.2`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/v0.2.1...v0.2.2)

Concurrent CLI downloads with the same tag but different rating filters or output
directories now appear as separate rows in the TUI Queue tab instead of merging.

### Added

- Per-job progress tracking: each download writes its own JSON record to
  `data_dir()/progress/` instead of sharing a single `download_progress.json`,
  preventing concurrent processes from overwriting each other.
- `ProgressSnapshot::base_label()` now includes the rating filter, so identical
  search queries with different rating sets remain distinct.
- `ProgressSnapshot::output_name()` extracts the final path component for
  disambiguation when labels collide.

### Fixed

- TUI Queue tab now renders each concurrent job as a separate row, identified
  by tag, rating filter, and output directory.

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
