# AGENTS.md

Context for AI agents working on **gelbooru-downloader**. Keep this file short,
factual and up to date — update it when the codebase or tooling changes.

## Project

- Python 3.12+ package (src layout) built with **hatchling**.
- Console script `gelbooru` (entry point `gelbooru_downloader.cli:main`); also
  runnable as `python -m gelbooru_downloader`.
- Dependencies: `httpx`, `typer`, `rich`, `pydantic`. Dev: `pytest`,
  `pytest-asyncio`, `ruff`, `build`.
- 429 tests pass; `ruff check src tests` is clean.

## Working in this repo

```bash
# set up (venv already exists at .venv on this machine)
.venv/bin/python -m pip install -e ".[dev]"

# tests
.venv/bin/python -m pytest            # full suite
.venv/bin/python -m pytest tests/test_api.py -q

# lint / format
.venv/bin/ruff check src tests
.venv/bin/ruff format src tests

# run the CLI
.venv/bin/gelbooru --help
.venv/bin/gelbooru search "blue_hair solo" --limit 5
```

Notes for this machine: there is no global `pip`/`node`; always use the venv's
interpreter. `pytest` is configured with `asyncio_mode = "auto"` and treats
`DeprecationWarning` in `gelbooru_downloader.*` as an error.

## Architecture

| Module | Responsibility |
|---|---|
| `cli.py` | Typer app, global options, logging, `command_guard` error handling |
| `app.py` | `AppContext` (config, paths, console, history, presets, client/downloader factories) |
| `config.py` | TOML config, env overrides (`GELBOORU_*`), platform paths, validation |
| `api.py` | Async Gelbooru dapi client (JSON + XML), retries, backoff, pagination |
| `models.py` | `Post`, `PostPage`, `DownloadResult`, `DownloadSummary`, history models |
| `filters.py` | Client-side filtering, sorting, folder-name derivation |
| `downloader.py` | Concurrent, resumable, atomic downloads; progress/reporting |
| `database.py` | SQLite download history (WAL, per-thread connections) |
| `presets.py` | Named tag presets (JSON, atomic writes) |
| `console.py` | Rich banner, tables, progress bars, reports |
| `exceptions.py` | Typed error hierarchy rooted at `GelbooruError` |
| `utils.py` | Filenames, sizes, retries, atomic writes, tag expressions |
| `commands/*` | One module per command group; `_common.py` holds shared search/download plumbing |

Public API re-exports: `GelbooruClient`, `Downloader`, `Config` (see `__init__.py`).

## Key invariants

- **Safety:** `target_path` resolves every destination and refuses paths outside
  the output directory; filenames are sanitised (`sanitize_filename`).
- **Atomic writes:** files stream to `*.part` and are renamed only after
  validation; interrupted runs never leave a truncated image that looks done.
- **Content validation:** downloads reject HTML/XML/JSON bodies even when served
  with an `image/*` content type.
- **Path traversal:** `filename_template` may not contain separators or `..`.
- **Secrets:** API keys are redacted in logs and `config show`; never print them.
- **Per-file failure isolation:** one bad post never aborts a batch; errors are
  recorded in the summary and history.

## Configuration precedence

`defaults < config file < environment variables (GELBOORU_*) < CLI options`.
See `config.ENV_MAP` for the full env-var → dotted-key table and
`AppPaths.discover` for platform directories (XDG on Linux).

## Testing

- Tests live in `tests/`; `conftest.py` provides fixtures and payload builders,
  `mock_server.py` is an offline fake dapi + CDN.
- Async tests rely on `asyncio_mode = "auto"`.
- Do not weaken assertions or skip tests to make a change pass; fix the code or
  the test's underlying assumption.

## Conventions

- Keep command modules thin: build a `DownloadRequest`, call `_common` helpers,
  report via `console`.
- Pure helpers belong in `utils.py` / `filters.py` so they are unit-testable
  without network or disk side effects.
- Validate at boundaries (config via pydantic, CLI via typer callbacks); raise
  typed `GelbooruError` subclasses for domain failures so `command_guard` can
  render them.
- Run `ruff` before finishing; the project enables a broad rule set (see
  `pyproject.toml`).

## Known limitations

See README "Known limitations": API paging stops near pid 20000; client-side
filtering costs bandwidth; `file_size` absent from the JSON API; auth mandatory;
no HTML scraping; rate limits are the server's decision.