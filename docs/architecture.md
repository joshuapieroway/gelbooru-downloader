# Architecture

## Module Overview

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

## Key Invariants

### Safety
- `target_path` resolves every destination and refuses paths outside the output directory
- Filenames are sanitised (`sanitize_filename`)
- `filename_template` may not contain separators or `..`

### Atomic Writes
- Files stream to `*.part` and are renamed only after validation
- Interrupted runs never leave a truncated image that looks done

### Content Validation
- Downloads reject HTML/XML/JSON bodies even when served with `image/*` content type

### Secrets
- API keys are redacted in logs and `config show`
- Never printed in full

### Per-File Failure Isolation
- One bad post never aborts a batch
- Errors are recorded in the summary and history

## Configuration Precedence

```
defaults < config file < environment variables (GELBOORU_*) < CLI options
```

See `config.ENV_MAP` for the full env-var → dotted-key table and
`AppPaths.discover` for platform directories (XDG on Linux).

## Testing

- Tests live in `tests/` on the `development` branch
- `conftest.py` provides fixtures and payload builders
- `mock_server.py` is an offline fake dapi + CDN
- Async tests rely on `asyncio_mode = "auto"`
- `DeprecationWarning` in `gelbooru_downloader.*` is treated as an error

## Conventions

- Keep command modules thin: build a `DownloadRequest`, call `_common` helpers, report via `console`
- Pure helpers belong in `utils.py` / `filters.py` for unit testability
- Validate at boundaries (config via pydantic, CLI via typer callbacks)
- Raise typed `GelbooruError` subclasses for domain failures
- Run `ruff` before finishing

## Public API

Re-exports: `GelbooruClient`, `Downloader`, `Config` (see `__init__.py`).
