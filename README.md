# Gelbooru Downloader

High-performance asynchronous Gelbooru API client and image downloader built entirely in rust.

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
gelbooru queue list
gelbooru queue resume
gelbooru queue retry
gelbooru queue clear --failed
gelbooru repl
gelbooru tui
```

## TUI

Run `gelbooru tui`, press `/` to search, and `v` (or Enter) on a result to
open the full-screen viewer. In the viewer:

| Key | Action |
| --- | --- |
| `←` / `→` | previous / next post |
| `j` / `k` | select the previous / next tag |
| `c` / `t` | copy post ID / tags |
| `o` | open the post on Gelbooru |
| `d` | queue a download to `<downloads.directory>/quick-downloads/` |
| `Esc` | close the viewer (queued downloads continue in the background) |

Quick downloads go to a dedicated subfolder (default
`~/Pictures/Gelbooru/quick-downloads/`), are recorded in history like any
other download, and stay in the persistent queue until they finish. Pressing
`d` on several posts queues them concurrently, up to
`downloads.concurrency` (default 16); progress stays visible in the footer.

## Filename and folder templates

Downloads can be laid out with templates configured globally under
`[downloads]`, per preset, or per command:

```bash
gelbooru download "landscape sky" \
  --folder-template "{artist}/{rating}" \
  --filename-template "{id}_{md5}.{ext}"
gelbooru preset create landscapes --tags "landscape" \
  --folder-template "{rating}" --filename-template "{artist}_{id}.{ext}"
```

Available placeholders: `{id}`, `{md5}`, `{ext}`, `{rating}`, `{score}`,
`{score_raw}`, `{width}`, `{height}`, `{artist}`, `{copyright}`,
`{character}`, `{meta}`, `{title}`, `{source}`, `{tag_count}`, `{year}`, and
`{sort_prefix}`. Unknown placeholders render as an empty string, and `{{` /
`}}` produce literal braces.

`{character}` prefers `character:` namespace tags (e.g. `character:hero`),
and falls back to tags matching `name_(series)`, e.g. `tangtang_(arknights)`
→ `tangtang (arknights)`. A post with neither renders as an empty segment,
which is skipped in the folder path. Every folder segment is sanitized with
`sanitize_filename`, so a value like `tangtang (arknights)` is stored as
`tangtang__arknights_`.

`{score}` renders a fixed-width inverted score by default, so a normal
alphabetical filename sort lists the highest score first. `{score_raw}` keeps
the literal score. For example, `{score}_{score_raw}_{id}.{ext}` produces
sortable names that still show the real score. `{score_desc}` is retained as an
alias for `{score}`.

`{sort_prefix}` is set from `--sort` / `--order` when the sort key is `score`
or `id`. For other sort keys (`width`, `height`, `created`, `rating`) no
prefix is emitted, but posts are still ordered server- and client-side. The
default `desc` order applies when `--order` is omitted.

A post already recorded in history is skipped even if the filename template
changed, as long as the recorded file still exists. Use `--force` to download
it again under the new template.

Folder segments are separated by `/`; empty, `.`, and `..` segments are
skipped and every segment is sanitized, so a template can never escape the
download directory. The default filename template
(`{sort_prefix}{id}_{md5}.{ext}`) keeps the historical names.

## Persistent download queue

Every queued download is stored in SQLite before it starts. Successful rows are
removed automatically; failures stay behind with their error message. Rows
that were still queued or downloading when a run was interrupted are resumed
automatically when the TUI starts.

```bash
gelbooru queue list            # pending, active, and failed rows
gelbooru queue resume          # continue queued/downloading rows
gelbooru queue retry           # requeue failed rows and resume
gelbooru queue clear --failed  # drop failed rows
gelbooru queue remove <id>     # drop one row
```

## Configuration

TOML config with env/CLI overrides. Run `gelbooru config --init` to create a config file. XDG directories used by default.

Environment variables: `GELBOORU_BASE_URL`, `GELBOORU_USER_ID`, `GELBOORU_API_KEY`, `GELBOORU_DOWNLOAD_DIR`, `GELBOORU_CONCURRENCY`.
