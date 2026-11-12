# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.6.2] - 2026-11-12

Compare: [`v0.6.1...v0.6.2`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/v0.6.1...v0.6.2)

Patch release: controversial tags now appear in TUI autocomplete.

### Fixed

- TUI tag autocomplete now merges the website's `autocomplete2` results with
  a DAPI prefix search, so tags filtered out of autocomplete (e.g. "loli",
  "gore") still surface, sorted by post count.

## [0.6.1] - 2026-11-11

Compare: [`v0.6.0...v0.6.1`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/v0.6.0...v0.6.1)

Patch release: autocomplete now uses Gelbooru's `autocomplete2` endpoint so
suggestions are sorted by post count (most first), matching the website.

### Fixed

- TUI tag autocomplete switched from the tag DAPI (id order, capped at 200)
  to the `autocomplete2` endpoint, which returns the top matches sorted by
  post count. Typing "rem" now surfaces "remilia scarlet", "rem (re:zero)",
  etc. instead of missing high-count tags.
- Restored the `tags_by_names` client method (used for artist/character/
  series categorization in the viewer) that was accidentally removed.

## [0.6.0] - 2026-11-11

Compare: [`v0.5.1...v0.6.0`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/v0.5.1...v0.6.0)

Major TUI and CLI feature release: search autocomplete, persisted search
history, export command, mp4 handling, context-aware hints, and a
centralized color palette.

### Added

- TUI search autocomplete: as you type, tag suggestions are fetched from
  Gelbooru and ranked by post count (most first). `↑`/`↓` (or `Ctrl+↑`/
  `Ctrl+↓`, `PageUp`/`PageDown`) navigate, `Tab` accepts, `Enter` searches.
- Search history persists between sessions in
  `search_history.txt` (capped at 100 entries); `↑`/`↓` recall previous
  queries when no suggestions are visible.
- `gelbooru export <tags> <json|csv> <file>` command to save search results.
- `--include` / `--exclude` tag filters for `search`, `download`, and
  `export`.
- Video (mp4/webm/...) previews in the TUI viewer (first frame via kitty).
- CLI help now includes usage examples (`after_help`).

### Changed

- TUI bottom hints are context-aware: search-specific binds only show on
  the Search tab.
- Search history is recorded per executed search (consecutive duplicates
  collapsed).
- Centralized TUI color palette constants for consistent theming.
- Highlighted text (tabs, suggestions, selected row) now uses readable
  contrast (reversed/bold or white-on-selection).

### Fixed

- `Ctrl+Backspace` deletes the previous word instead of inserting "H".
- mp4 posts are filtered out of TUI search results (still downloadable
  via CLI).
- Autocomplete fetches are invalidated on history recall / accept / cancel
  so stale suggestions cannot repopulate the panel.
- Queue status cell is color-coded (completed / downloading / errors).

## [0.5.1] - 2026-11-08

Compare: [`v0.5.0...v0.5.1`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/v0.5.0...v0.5.1)

Revises the 0.5.0 tarball (rebuilt against the tagged `CHANGELOG.md`, adds
SHA-256 checksums, and standardises the tarball name across releases) and
fixes filename-prefix propagation for `batch`, `watch`, and `preset run`.
The `{character}` fallback is hardened against malformed tags.

### Fixed

- `batch`, `watch`, and `preset run` now propagate `--sort` / `--order` to
  the download engine, so `{sort_prefix}` is emitted there just like
  `download` instead of only ordering the API response.
- `--order` no longer defaults to `"desc"` at the CLI layer. Without
  `--sort`, no `order` parameter is sent to the API; when `--sort` is
  given, descending order still applies by default in sorting and
  `{sort_prefix}` generation.
- `{character}` fallback no longer stops at the first tag containing `_(`
  when that tag has no closing `)`. Malformed tags are skipped and the
  fallback uses a byte-accurate `find(')')` instead of a char-count index,
  so multi-byte tags cannot mis-slice or panic. Tags with an empty name
  (`_(arknights)`) or empty suffix (`tangtang_()`) are also skipped.
- Rebuilt the `v0.5.0` tarball against the tagged sources: the shipped
  `CHANGELOG.md` inside the archive now matches `v0.5.0`, the asset name
  keeps the `gelbooru-downloader-<version>-<target>.tar.gz` form, and a
  SHA-256 asset accompanies it. `v0.4.0` remains without a tarball; see
  below.

## [0.5.0] - 2026-11-08

Compare: [`v0.4.2...v0.5.0`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/v0.4.2...v0.5.0)

Brings sort/order validation and parity across download, batch, watch, and
presets. The download command now properly wires sort/order to the filename
template. Enhanced `{character}` folder template now extracts character names
from tags like `tangtang_(arknights)` when `character:` namespace is not used.

### Added

- `download --sort`, `download --order` now properly apply to filename templates.
  Use `--sort score --order desc` to download higher-scored posts first.
- `batch --sort`, `batch --order`, `batch --rating` apply the same ordering to
  every tag in the file; `watch --sort`, `watch --order`, `watch --rating`,
  `watch --limit` control each poll.
- `preset create` stores `--sort`, `--order`, and `--rating`; `preset run`
  honours them and allows per-run overrides via `preset run --sort` / `--order`.
- `validate_sort_order` check: invalid `--sort` / `--order` values exit with
  a message listing allowed values (`score`, `id`, `width`, `height`, `created`,
  `date`, `created_at`, `rating` and `asc`/`desc`) instead of silently
  sorting by `id`.
- `{character}` folder template now extracts character names from non-namespaced
  tags matching the pattern `name_(series)`, e.g., `tangtang_(arknights)` → `tangtang (arknights)`.

### Changed

- `download` command now wires `sort` and `order` parameters to the download
  engine for proper filename prefix generation.
- `{character}` placeholder prefers `character:` namespace tags over pattern-style
  tags, and falls back to extracting from `name_(series)` patterns.
- Legacy presets without `sort`/`order` deserialize as `None` and behave as
  before.

## [0.4.2] - 2026-11-08

Compare: [`v0.4.1...v0.4.2`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/v0.4.1...v0.4.2)

Restores `--sort` and `--order` flags to the `download` command, allowing users to
control API-side ordering when downloading multiple posts by tags.

### Added

- `download --sort` and `download --order` flags restore control over post ordering.
  Use `--sort score --order desc` to download higher-rated posts first, or other
  values like `id`, `width`, `height`, `created`, `rating` (default sort is `score`
  descending).

## [0.4.1] - 2026-11-07

Compare: [`v0.4.0...v0.4.1`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/v0.4.0...v0.4.1)

Kept download progress clean and prevented duplicate sample downloads when a
filename template changes.

### Changed

- Routine retry messages, including hotlink 404s that fall back to the sample
  URL, are logged at debug level instead of warning. Use `--verbose` to see
  them.

### Fixed

- A post already recorded in history is skipped even when the filename template
  points at a new name, instead of downloading it again and possibly saving a
  duplicate sample. `--force` still overrides this check.

## [0.4.0] - 2026-11-06

Compare: [`v0.3.1...v0.4.0`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/v0.3.1...v0.4.0)

Filename templates now sort by descending score without any extra flags.

### Added

- `{score_raw}` renders the literal post score.

### Changed

- `{score}` renders a fixed-width inverted score, so ascending filename order
  lists the highest score first. `{score_desc}` remains as an alias.
- Filename templates such as `{score}_{id}` now put the highest-scored posts at
  the top without `--sort` or `--order`.

### Removed

- `download --sort` and `download --order`; use the filename template to
  control ordering. `search --sort` and `search --order` are unchanged.

## [0.3.1] - 2026-11-06

Compare: [`v0.3.0...v0.3.1`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/v0.3.0...v0.3.1)

Adds a filename placeholder that sorts posts by descending score.

### Added

- `{score_desc}` renders a fixed-width inverted score, so ascending filename
  order lists the highest score first. Combine it with `{score}` when the real
  score should stay visible, for example `{score_desc}_{score}_{id}.{ext}`.

## [0.3.0] - 2026-11-06

Compare: [`v0.2.7...v0.3.0`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/v0.2.7...v0.3.0)

Downloads can now be organized with filename and folder templates, and
unfinished work is kept in a persistent queue that resumes after a restart.

### Added

- `[downloads]` supports `filename_template` and `folder_template`; placeholders
  include `{id}`, `{md5}`, `{ext}`, `{rating}`, `{artist}`, `{copyright}`,
  `{character}`, `{title}`, `{source}`, `{year}`, `{tag_count}`, and
  `{sort_prefix}`.
- `download` accepts `--filename-template` and `--folder-template`; presets can
  store both templates and `preset create` accepts `--tags`,
  `--filename-template`, and `--folder-template`.
- A SQLite-backed download queue stores pending posts, their destination, and
  templates before transfer. Failed rows keep their error message.
- `gelbooru queue list`, `queue resume`, `queue retry`, `queue clear`, and
  `queue remove` inspect and manage the persistent queue.
- The TUI resumes queued and interrupted downloads when it starts.

### Changed

- The default `{sort_prefix}{id}_{md5}.{ext}` template keeps existing filenames,
  including the empty-hash `_file` fallback.
- Folder template segments are sanitized and `.`/`..` segments are skipped, so
  a template cannot escape the configured download directory.

### Fixed

- Downloads stopped by a closed TUI or an interrupted CLI run no longer lose
  their remaining work.

## [0.2.7] - 2026-11-06

Compare: [`v0.2.6...v0.2.7`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/v0.2.6...v0.2.7)

Quick downloads now report completion instead of leaving the queued message in
the viewer footer.

### Changed

- When a quick download finishes, the TUI viewer shows
  `Download complete: <file> (<size>) in <dir>`; a skipped duplicate shows
  `Download complete (already present): <path>`.

## [0.2.6] - 2026-11-06

Compare: [`v0.2.5...v0.2.6`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/v0.2.5...v0.2.6)

Adds a visible cursor to the viewer's tag panel.

### Added

- The TUI viewer highlights the selected tag in bold with a contrasting
  background; `j`/`k`, `PageUp`/`PageDown`, `Home`/`End`, and the mouse wheel
  move the selection and keep it scrolled into view.

### Changed

- The viewer footer describes `j`/`k` as tag selection instead of tag scrolling.

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
