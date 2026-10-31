# Configuration

## Config File Location

- **Linux**: `~/.config/gelbooru-downloader/config.toml`
- **macOS**: `~/Library/Application Support/gelbooru-downloader/config.toml`
- **Windows**: `%APPDATA%\gelbooru-downloader\config.toml`

Use `--portable` to store configuration beside the executable.

## Configuration Precedence

```
defaults < config file < environment variables (GELBOORU_*) < CLI options
```

## Environment Variables

| Variable | Config Key | Description |
|---|---|---|
| `GELBOORU_USER_ID` | `network.user_id` | Your Gelbooru user ID |
| `GELBOORU_API_KEY` | `network.api_key` | Your Gelbooru API key |
| `GELBOORU_OUTPUT_DIR` | `general.output_dir` | Default download directory |
| `GELBOORU_CONCURRENCY` | `general.concurrency` | Simultaneous downloads |

See `config.ENV_MAP` in the source for the full table.

## Config File Format

```toml
[general]
output_dir = "~/Downloads/Gelbooru-downloader"
concurrency = 5
page_size = 100
retries = 3
timeout = 30.0
skip_existing = true
archive_metadata = false
filename_template = "{id}"
max_downloads_per_run = 0

[network]
base_url = "https://gelbooru.com"
user_agent = "GelbooruCLI/1.1.1"
api_key = "your_api_key"
user_id = "1234"
auth_mode = "params"
response_format = "json"

[display]
show_banner = true
show_progress = true
table_max_rows = 40
tag_preview_count = 6
```

## Commands

```bash
gelbooru config show              # Show effective configuration
gelbooru config set KEY VALUE     # Set a value
gelbooru config unset KEY         # Reset to default
gelbooru config init --force      # Write a commented config.toml
gelbooru config path              # Show config file path
gelbooru config history           # Show download history
gelbooru config history --stats   # Show history statistics
gelbooru config clear --yes       # Clear download history
```

## History Database

Download history is stored in a SQLite database (WAL mode) at:
- **Linux**: `~/.local/share/gelbooru-downloader/history.db`

History tracks: post ID, status, file size, timestamp, file path, and errors.

## Presets

Presets are stored as JSON in the config directory (`presets.json`) with
restricted permissions (0600). See [Presets](presets.md) for details.
