# Gelbooru Downloader (Rust)

High-performance asynchronous Gelbooru API client and image downloader.

## Build

```bash
cargo build --release
```

Binary: `target/release/gelbooru`

## Usage

```bash
gelbooru config --init
gelbooru search "landscape sky" --rating safe
gelbooru inspect 123456
gelbooru download 123456
gelbooru download "landscape sky" --limit 100
gelbooru batch tags.txt
gelbooru history
gelbooru history --search landscape
gelbooru preset create landscapes
gelbooru preset list
gelbooru preset run landscapes
gelbooru watch "landscape sky" --interval 300
gelbooru repl
gelbooru tui
```

## TUI

Run `gelbooru tui`, press `/` to search, and `v` (or Enter) on a result to
open the full-screen viewer. In the viewer:

| Key | Action |
| --- | --- |
| `←` / `→` | previous / next post |
| `j` / `k` | scroll tags |
| `c` / `t` | copy post ID / tags |
| `o` | open the post on Gelbooru |
| `d` | queue a download to `<downloads.directory>/quick-downloads/` |
| `Esc` | close the viewer (queued downloads continue in the background) |

Quick downloads go to a dedicated subfolder (default
`~/Pictures/Gelbooru/quick-downloads/`) and are recorded in history like any
other download. Pressing `d` on several posts queues them concurrently, up to
`downloads.concurrency` (default 16); progress stays visible in the footer.

## Configuration

TOML config with env/CLI overrides. See `config.example.toml`. XDG directories used by default.

Environment variables: `GELBOORU_BASE_URL`, `GELBOORU_USER_ID`, `GELBOORU_API_KEY`, `GELBOORU_DOWNLOAD_DIR`, `GELBOORU_CONCURRENCY`.

## Tests

```bash
cargo test
```
