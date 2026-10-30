# Downloading

## Basic Download

```bash
gelbooru download "landscape" --limit 50 --output ./downloads/landscape
```

## Resumable & Atomic Downloads

- Files are streamed to `*.part` and renamed only after validation
- Interrupted runs never leave truncated files
- Content validation rejects HTML/XML/JSON bodies even with `image/*` content type
- Per-file failure isolation: one bad post doesn't abort the batch

## Concurrent Downloads

```bash
gelbooru download "1girl" --concurrency 10 --limit 100
```

## Filename Templates

```bash
gelbooru download "1girl" --filename "{id}_{rating}" --limit 10
```

Available placeholders: `{id}`, `{md5}`, `{rating}`, `{index}`.

Path traversal is prevented: templates may not contain separators or `..`.

## Folder Organization

```bash
# Store in a named sub-folder
gelbooru download "landscape" --collection landscapes

# Group by leading tags
gelbooru download "1girl blue_hair" --folder-by-tags
```

## Duplicate Detection

```bash
# Detect duplicates by SHA-256
gelbooru download "1girl" --no-duplicates
```

## Metadata Sidecars

```bash
# Write .json metadata next to each file
gelbooru download "1girl" --archive
```

## Dry Run

```bash
# Preview without downloading
gelbooru download "1girl" --dry-run --limit 20
```

## Batch Processing

Create a file with one tag expression per line:

```
# searches.txt
blue_hair solo
landscape sky
cat_ears -rating:explicit
```

Then run:

```bash
gelbooru batch searches.txt --output ./downloads --limit 25
```

Each line gets its own sub-folder by default (`--folder-by-line`).

## Watch Mode

```bash
# Check every 5 minutes for new posts
gelbooru watch "blue_hair solo" --interval 300 --limit 100 --output ./downloads

# Run once
gelbooru watch "landscape" --once

# Run 5 times then stop
gelbooru watch "landscape" --runs 5
```

## Downloading by Post ID

```bash
gelbooru post download 1234567 7654321 --output ./downloads
```

## Safety Features

- `target_path` resolves every destination and refuses paths outside the output directory
- Filenames are sanitised (`sanitize_filename`)
- API keys are redacted in logs and `config show`
- Per-file failure isolation: errors are recorded in the summary and history
