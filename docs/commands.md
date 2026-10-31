# Commands Reference

All commands are accessed via `gelbooru <command>`. Run `gelbooru <command> --help`
for command-specific options.

## Global Options

| Option | Description |
|---|---|
| `--config, -c <path>` | Use this config file |
| `--portable` | Store config/history beside the executable |
| `--color / --no-color` | Force coloured output on or off |
| `--quiet, -q` | Only show warnings and errors |
| `--verbose, -v` | Increase logging (-v warnings, -vv info, -vvv debug) |
| `--log-file <path>` | Also write debug logs to this file |

## search TAGS

Search Gelbooru and display results as a table, or export to JSON/CSV.

```bash
gelbooru search "blue_hair solo" --limit 20
gelbooru search "landscape" --rating safe --ext jpg,png
gelbooru search "cat_ears" --min-width 1000 --max-size 5MB --order score
gelbooru search "1girl" --format json --output results.json
```

### Key Options

| Option | Description |
|---|---|
| `--limit, -n` | Maximum posts (0 = no limit, default 50) |
| `--page, -p` | First page (1 = newest first) |
| `--page-size` | Posts per API page (max 100) |
| `--rating, -r` | Filter ratings (safe, general, questionable, sensitive, explicit, unrated) |
| `--exclude, -x` | Exclude tags (comma separated) |
| `--ext, -e` | Filter by file extension |
| `--min-width`, `--max-width` | Width filter in pixels |
| `--min-height`, `--max-height` | Height filter in pixels |
| `--min-size`, `--max-size` | File size filter (e.g., 500KB, 10MB) |
| `--min-score` | Minimum post score |
| `--order, -O` | Sort order (score, width, height, filesize, id) |
| `--shuffle` | Randomise result order |
| `--format, -f` | Output format: table (default), json, csv |
| `--output, -O` | Write to file instead of stdout |
| `--max-pages` | Safety cap on API pages (default 50) |
| `--dry-run` | Show matches without downloading |

## download TAGS

Search and download matching posts in one step.

```bash
gelbooru download "landscape" --limit 50 --output ./downloads/landscape
gelbooru download "blue_hair solo" --rating safe --no-duplicates --archive
gelbooru download "cat_ears" --concurrency 10 --overwrite --dry-run
```

### Additional Options (beyond search)

| Option | Description |
|---|---|
| `--output, -o` | Output directory |
| `--concurrency, -c` | Simultaneous downloads (1-64) |
| `--retries` | Retries per file (0-10) |
| `--timeout` | Per-request timeout in seconds |
| `--overwrite` | Re-download existing files |
| `--skip-existing/--no-skip-existing` | Skip files already on disk |
| `--dry-run` | List files without downloading |
| `--no-duplicates` | Detect duplicates by SHA-256 |
| `--archive/--no-archive` | Write JSON metadata sidecar |
| `--collection, -C` | Store in sub-folder |
| `--folder-by-tags` | Group into folders by leading tags |
| `--filename, -F` | Filename template ({id}, {md5}, {rating}, {index}) |
| `--no-progress` | Hide progress bar |

## browse TAGS

Paged thumbnail view with titles and navigation. Images are rendered as
coloured ASCII art inline.

```bash
gelbooru browse "blue_hair solo" -n 12
gelbooru browse "landscape" --rating safe --exclude furry
```

### Options

| Option | Description |
|---|---|
| `--limit, -n` | Maximum posts (0 = no limit) |
| `--rating, -r` | Filter by rating |
| `--exclude, -x` | Exclude tags |

Navigation: `n` (next page), `p` (previous page), `q` (quit).

## export TAGS FORMAT FILE

Export search results to JSON or CSV without downloading.

```bash
gelbooru export "blue_hair solo" json results.json
gelbooru export "landscape" csv results.csv --limit 100
```

## post

Inspect and download individual posts by ID.

```bash
gelbooru post info 1234567
gelbooru post info 1234567 --json
gelbooru post download 1234567 7654321 --output ./downloads
```

## presets

Save and reuse tag searches.

```bash
gelbooru presets add anime "blue_hair solo" --folder anime --limit 50
gelbooru presets list
gelbooru presets show anime
gelbooru presets download anime --limit 25
gelbooru presets remove anime
```

## batch TAGS_FILE

Run many searches from a file (one tag expression per line).

```bash
gelbooru batch searches.txt --output ./downloads --limit 25
```

### Options

| Option | Description |
|---|---|
| `--output, -o` | Base output directory |
| `--limit, -n` | Max posts per search (default 25) |
| `--concurrency, -c` | Simultaneous downloads |
| `--folder-by-line/--no-folder-by-line` | Group by search line (default on) |
| `--overwrite` | Re-download existing files |
| `--dry-run` | Preview without downloading |
| `--no-duplicates` | Detect duplicates by SHA-256 |
| `--archive/--no-archive` | Write metadata sidecars |
| `--continue-on-error/--stop-on-error` | Continue on failure (default continue) |
| `--no-progress` | Hide progress bar |

## watch TAGS

Repeat a search on an interval and fetch new posts.

```bash
gelbooru watch "blue_hair solo" --interval 300 --limit 100 --output ./downloads
gelbooru watch "landscape" --once --runs 5
```

### Options

| Option | Description |
|---|---|
| `--interval, -i` | Seconds between runs (min 5, default 300) |
| `--limit, -n` | Max posts per run (default 100) |
| `--output, -o` | Output directory |
| `--once` | Run once and exit |
| `--runs` | Number of runs (0 = forever) |
| `--overwrite` | Re-download existing files |
| `--no-duplicates` | Detect duplicates by SHA-256 |
| `--archive/--no-archive` | Write metadata sidecars |
| `--no-progress` | Hide progress bar |

## interactive

REPL mode with tab completion and Gelbooru tag autocomplete. The banner stays
pinned at the top of the terminal, and `browse` pages render kitty image
previews in fixed cells below it (with an ASCII fallback).

```bash
gelbooru interactive
gelbooru interactive --output ./downloads --limit 25
```

Commands within the REPL:
- `<tags>` — Browse posts (default action)
- `search <tags> [-n 25]` — Search and show table
- `download <tags> [-n 25] [-o DIR]` — Download
- `browse <tags> [-n 25]` — Paged thumbnail view
- `info <post_id>` — Show post details
- `dl <post_id...>` — Download specific posts
- `export <tags> <json|csv> <file>` — Export results
- `presets` — List saved presets
- `history` — Recent downloads
- `help`, `quit` — Help or exit

## config

Manage configuration and history.

```bash
gelbooru config show
gelbooru config set general.output_dir ~/Downloads
gelbooru config unset general.output_dir
gelbooru config init --force
gelbooru config path
gelbooru config history --limit 20 --status failed
gelbooru config history --stats
gelbooru config clear --yes --prune
```
