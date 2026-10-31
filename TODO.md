# TODO

Prioritised work for gelbooru-downloader. Check items off as they land and keep
this file in sync with `AGENTS.md`.

## P0 — Bugs / correctness

- [ ] Add end-to-end coverage for interactive `search`/`download` table output
      (the table call was previously unreachable due to indentation).
- [ ] Decide whether `HistoryDatabase` should store a content hash for every
      download (currently only when `--no-duplicates` is used) so duplicate
      detection can work retroactively.
- [x] Verify resume correctness when the server ignores `Range`: the partial
      `.part` file is discarded, and the recorded history row now reflects the
      bytes actually on disk (downloader finalises `total` from `target.stat()`).
- [x] Handle `created_at` strings with locale-specific month names: added
      `_MONTH_ALIASES` mapping and locale normalization in `_as_timestamp`.

## P1 — Quality / maintainability

- [ ] Add static type checking (`mypy` or `pyright`) to CI; annotate the few
      `Any` escape hatches in `commands/interactive.py` and `app.py`.
- [x] Replace `save_config`'s `__import__('gelbooru_downloader')` hack with
      `importlib.metadata.version` (no circular import needed).
- [ ] Split `commands/download.py` (very long Typer option list) into a shared
      option dataclass / decorator to reduce duplication with `post.py`,
      `presets.py` and `batch.py`.
- [ ] Add a `--json` output mode for `download`/`batch` summaries for scripting.
- [ ] Document and test the `GELBOORU_HOME` / portable-mode flows on Windows
      and macOS (only exercised on Linux today).
- [ ] Add a fuzz/property test for `sanitize_filename` and `parse_tag_expression`.

## P2 — Features

- [x] Support downloading by MD5 or filename template `{tags}` / `{score}`
      (added `{score}` and `{tags}` fields, plus `{index}` zero-padding).
- [x] Add `gelbooru export` command reusing `export_posts` (also available
      through `search --format`).
- [ ] Optional progress for multi-page searches (currently only downloads show
      progress).
- [ ] Watch mode: persist seen post ids across runs (today it relies on the
      history database + skip-existing).
- [x] Add shell-completion installation instructions to the README.
- [ ] Publish to PyPI / provide a `pipx` install path (currently venv-only).

## Recently completed

- [x] Fix interactive `browse` preview drift: kitty images now render in fixed
      rectangles below the pinned banner, and the page area is cleared before
      each redraw and on exit.
- [x] Bump the version to 1.1.1 for the preview-alignment release and add a
      root `CHANGELOG.md`.
- [x] Bump the version to 1.1.0 for the interactive-mode release.
- [x] Pin the interactive banner to the top of the screen and drop the banner
      border outside interactive mode.
- [x] Move active session to `/opt/gelbooru-downloader` as the main project.
- [x] Fix public API exports (`GelbooruClient`, `Downloader`, `Config`) in
      `__init__.py` to match the documented API.
- [x] Fix unreachable interactive `search` result table.
- [x] Add project-level `AGENTS.md`.
- [x] Add interactive `browse` command with paged thumbnails and colored ASCII
      image previews.
- [x] Add prompt_toolkit REPL with Gelbooru tag autocomplete (sorted by count).
- [x] Add `--rating` and `--exclude` support to interactive commands.
- [x] Change default output dir to `~/Downloads/Gelbooru-downloader`.
- [x] Remake banner with blue theme and new GELBOORU logo.
- [x] Default to error-only logging for public builds.
- [x] Add `gelbooru export` command (JSON/CSV).
- [x] Replace `__import__` hack in `config.py` with `importlib.metadata`.
- [x] Handle locale-specific month names in `created_at` parsing.