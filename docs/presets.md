# Presets

Presets are named, saved tag searches stored as JSON next to your config.

## Storage

- File: `~/.config/gelbooru-downloader/presets.json` (mode `0600`, atomic writes)
- Each preset holds: `tags`, optional `folder`, `notes`, `limit`, `created_at`, `updated_at`

## Management

```bash
# List all presets
gelbooru presets list

# Add a preset
gelbooru presets add anime "blue_hair solo" --folder anime --limit 50 --notes "My anime search"

# Show a preset
gelbooru presets show anime

# Remove a preset
gelbooru presets remove anime
```

### Options for `add`

| Option | Description |
|---|---|
| `--folder, -C` | Default sub-folder for downloads |
| `--limit, -n` | Default result limit |
| `--notes` | Description/notes |
| `--force, -f` | Overwrite existing preset |

### Name Rules

Names must match `^[A-Za-z0-9][A-Za-z0-9 _.\-]{0,63}$` (alphanumeric, spaces,
underscores, dots, hyphens).

## Downloading from a Preset

```bash
gelbooru presets download anime --limit 25 --output ./downloads
```

Uses the preset's tags with exclusions preserved. Applies the preset's default
`limit`/`folder` unless overridden on the CLI. Supports all download flags.

## Interactive Mode

Presets can also be accessed in interactive mode:

```
gelbooru interactive
# then: presets
```
