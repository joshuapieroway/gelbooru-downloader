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
- [ ] Handle `created_at` strings with locale-specific month names (Gelbooru
      occasionally returns non-English month abbreviations).

## P1 — Quality / maintainability

- [ ] Add static type checking (`mypy` or `pyright`) to CI; annotate the few
      `Any` escape hatches in `commands/interactive.py` and `app.py`.
- [ ] Replace `save_config`'s `__import__('gelbooru_downloader')` hack with a
      normal import of `__version__`.
- [ ] Split `commands/download.py` (very long Typer option list) into a shared
      option dataclass / decorator to reduce duplication with `post.py`,
      `presets.py` and `batch.py`.
- [ ] Add a `--json` output mode for `download`/`batch` summaries for scripting.
- [ ] Document and test the `GELBOORU_HOME` / portable-mode flows on Windows
      and macOS (only exercised on Linux today).
- [ ] Add a fuzz/property test for `sanitize_filename` and `parse_tag_expression`.

## P2 — Features

- [ ] Support downloading by MD5 or filename template `{tags}` / `{score}`.
- [ ] Add an `gelbooru export` command reusing `export_posts` (today only
      available through `search --format`).
- [ ] Optional progress for multi-page searches (currently only downloads show
      progress).
- [ ] Watch mode: persist seen post ids across runs (today it relies on the
      history database + skip-existing).
- [ ] Add shell-completion installation instructions to the README.
- [ ] Publish to PyPI / provide a `pipx` install path (currently venv-only).

## Recently completed

- [x] Move active session to `/opt/gelbooru-downloader` as the main project.
- [x] Fix public API exports (`GelbooruClient`, `Downloader`, `Config`) in
      `__init__.py` to match the documented API.
- [x] Fix unreachable interactive `search` result table.
- [x] Add project-level `AGENTS.md`.