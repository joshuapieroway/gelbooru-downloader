# Gelbooru Downloader

A fast, async terminal client for the [Gelbooru](https://gelbooru.com) image board.
Search by tags, inspect posts, and download original files concurrently with
resumable transfers, a SQLite download history, presets, and a rich terminal UI.

## Install

Requires Python 3.12+.

```bash
git clone https://github.com/joshuapieroway/gelbooru-downloader.git
cd gelbooru-downloader
python -m venv .venv
.venv/bin/pip install -e ".[dev]"
```

The `gelbooru` console script is now on `.venv/bin/gelbooru`
(`python -m gelbooru_downloader` also works).

## Credentials

Gelbooru's dapi requires an API key:

```bash
export GELBOORU_USER_ID=1234
export GELBOORU_API_KEY=your_key_here
```

Create a key at https://gelbooru.com/account. Credentials can also live in the
config file (see `gelbooru config`).

## Quick start

```bash
gelbooru search "blue_hair solo" --limit 20
gelbooru download "landscape" --limit 50 --output ~/Downloads/Gelbooru-downloader/landscape
gelbooru browse "blue_hair solo" -n 12 --images
gelbooru export "blue_hair solo" json results.json
gelbooru post info 1234567
gelbooru presets add anime "blue_hair solo"
gelbooru presets download anime
```

## Commands

| Command | Purpose |
|---|---|
| `search TAGS` | Search and render/export results |
| `download TAGS` | Search and download matching posts |
| `browse TAGS` | Paged thumbnail view with titles and navigation (colored ASCII previews) |
| `export TAGS FORMAT FILE` | Export search results to JSON or CSV |
| `post info ID` / `post download ID...` | Inspect or fetch individual posts |
| `presets ...` | Save and reuse tag searches |
| `batch TAGS_FILE` | Run many searches from a file |
| `watch TAGS` | Re-run a search on an interval |
| `config ...` | Show/set config, history, paths |
| `interactive` | Optional REPL with Gelbooru tag autocomplete and hints |

Run `gelbooru <command> --help` for options.

## Configuration

Precedence: defaults < config file < `GELBOORU_*` env vars < CLI options.
State lives in `~/.config/gelbooru-downloader/` and `~/.local/share/gelbooru-downloader/`
(XDG). Use `--portable` to keep state beside the executable. See
`gelbooru config path` and `gelbooru config show`.

## Development

Tests and dev tooling live on the `development` branch. On `main`:

```bash
.venv/bin/ruff check src      # lint
.venv/bin/ruff format src     # format
```

## License

MIT — see `LICENSE`.
