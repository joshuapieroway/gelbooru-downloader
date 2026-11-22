# Gelbooru Downloader

High-performance asynchronous booru client and image downloader built entirely in rust, with Gelbooru downloads and Gelbooru/Rule34 search in the TUI.

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
gelbooru export "landscape sky" csv results.csv
gelbooru repl
gelbooru tui
```

## TUI

Run `gelbooru tui`. Tabs: Search, Queue, History, Stats, Settings (keys `1`-`5`
or `Tab`/`←`/`→`).

- `/` opens the search input. Type a query; tag suggestions appear ranked by
  post count. `Tab` accepts a suggestion, `↑`/`↓` navigate suggestions or
  recall previous searches, `Enter` searches, `Esc` cancels.
- Search treats whitespace as AND and supports `|` (OR) and `-` (NOT) plus
  bracketed grouping: `(aisha_greyrat | norn_greyrat) mushoku_tensei`.
- `s` switches the search tab between Gelbooru and Rule34. The current board
  is shown in the search box and results title.
- `n`/`p` page, `+`/`-` change page size, `v`/Enter open the viewer.
- Bottom hints are context-aware per tab.

### Rule34 search

Rule34 browsing lives in the search tab. Add your Rule34 API credentials in
the Settings tab (or `[rule34]` in `config.toml`), then press `s` to switch.
Rule34 requires an API key and user ID from
<https://rule34.xxx/index.php?page=account&s=options>; without them the
search tab shows an authentication hint instead of results. The user ID is
numeric and the API key is a long hex string; if they are pasted into the
wrong fields the client swaps them automatically.

Queries, autocomplete, paging, the viewer, quick downloads, and the
Character/Series columns all work on Rule34. Rule34 does not expose a result
count, so paging shows `Page N+` while another page may exist.

In the viewer:

| Key | Action |
| --- | --- |
| `←` / `→` | previous / next post |
| `↑` / `↓` (or mouse wheel) | scroll a tall image half a screen; otherwise move through tags |
| `PgUp` / `PgDn` | scroll a tall image a full screen; otherwise move through tags faster |
| `Home` / `End` | jump to the top / bottom of a tall image, or move through tags |
| `j` / `k` | select the previous / next tag |
| `Enter` | open the selected tag as a new search |
| `c` / `t` | copy post ID / tags |
| `o` | open the post on its image board |
| `d` | queue a download to the quick download directory |
| `s` | toggle the details/tags sidebar (enlarges the image) |
| `Esc` / `q` | close the viewer |

Very tall images, such as comic strips and webtoons, are shown at the full
width of the preview frame instead of being shrunk to fit its height. This
starts at three screenfuls; ordinary portrait photos and phone screenshots are
still fitted inside the frame. `↑`/`↓` and the mouse wheel move half a screen,
`PgUp`/`PgDn` move a full screen, and `Home`/`End` jump to the top or bottom.
Slices are prepared once in the background with ImageMagick (`magick` or
`convert`) and handed to kitty directly, so scrolling through a large strip
never re-decodes the source or re-uploads it through the terminal. Without
ImageMagick, tall images fall back to the normal fit-inside-the-frame preview.

Quick downloads go to a dedicated subfolder (configurable via
`downloads.quick_download_directory`, default
`<downloads.directory>/quick-downloads/`), are recorded in history like any
other download, and stay in the persistent queue until they finish.

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

Environment variables: `GELBOORU_BASE_URL`, `GELBOORU_USER_ID`, `GELBOORU_API_KEY`, `GELBOORU_DOWNLOAD_DIR`, `GELBOORU_QUICK_DOWNLOAD_DIR`, `GELBOORU_CONCURRENCY`, `RULE34_BASE_URL`, `RULE34_USER_ID`, `RULE34_API_KEY`.

### Notable config options

```toml
[downloads]
directory = "~/Pictures/Gelbooru"
quick_download_directory = ""   # empty = <directory>/quick-downloads
character_include_series = false
folder_template = "{character}/{rating}"
filename_template = "{score}_{id}.{ext}"

[search]
default_rating = "all"
page_size = 50
default_site = "gelbooru"       # or "rule34"
columns = ["rating", "score", "character", "series", "tags"]

[rule34]
base_url = "https://api.rule34.xxx"
user_id = ""
api_key = ""
```

`search.columns` accepts: `id`, `rating`, `score`, `size`, `character`,
`series`, `tags`, `date` (in any order). Settings are also editable live in
the TUI Settings tab, including the Rule34 credentials.
