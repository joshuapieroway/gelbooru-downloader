# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.1.1] — 2026-10-30

### Fixed

- Interactive `browse` previews no longer drift away from their post lines when
  the persistent banner is enabled. Kitty images are fitted into fixed
  rectangles below the banner instead of scrolling the content area.
- Old pages and kitty images are cleared before each browse redraw and when
  browse exits, so repeated `n`/`p` navigation does not accumulate pages or
  leave stale image fragments behind the banner.

### Changed

- Default user-agent strings now report `GelbooruCLI/1.1.1`.

## [1.1.0] — 2026-10-31

### Added

- Interactive mode with tag autocomplete, history, presets, and paged `browse`.
- Persistent banner pinned to the top of interactive mode via a DECSTBM scroll
  region.

### Changed

- The non-interactive banner is now borderless; only interactive mode keeps the
  bordered panel.
- Version bumped to 1.1.0 for the interactive-mode release.

[Unreleased]: https://github.com/joshuapieroway/gelbooru-downloader/compare/v1.1.1...HEAD
[1.1.1]: https://github.com/joshuapieroway/gelbooru-downloader/compare/v1.1.0...v1.1.1
[1.1.0]: https://github.com/joshuapieroway/gelbooru-downloader/releases/tag/v1.1.0
