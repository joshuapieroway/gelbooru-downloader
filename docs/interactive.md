# Interactive Mode

Interactive mode provides a REPL (Read-Eval-Print Loop) for exploring Gelbooru
without typing full commands. Start it with:

```bash
gelbooru interactive
gelbooru interactive --output ./downloads --limit 25
```

## Features

### Tab Completion
- **Tag autocomplete**: As you type a tag prefix (1+ characters), suggestions
  are fetched from the Gelbooru API and sorted by post count (most popular first).
- **Command completion**: Partial command names are completed on Tab.
- **Preset completion**: Saved preset names are also suggested.

### Auto-Suggest
Greyed-out text from your command history appears as you type. Press `→` (right
arrow) to accept the suggestion.

### Default Action
Typing any text that isn't a recognized command is treated as a browse query.
So you can just type `blue_hair solo` and press Enter to browse.

### Sticky Banner
The banner stays pinned at the top of the terminal while you work; command
output scrolls underneath it. Set `show_banner = false` in the `[display]`
section of your config to turn this off.

### Bottom Toolbar
A toolbar at the bottom of the screen shows available commands and keyboard
shortcuts.

## Keyboard Shortcuts

| Key | Action |
|---|---|
| `Tab` | Autocomplete |
| `↑` / `↓` | Navigate history |
| `Ctrl+C` | Cancel current operation |
| `Ctrl+D` | Exit |
| `→` | Accept auto-suggestion |

## REPL Commands

| Command | Description |
|---|---|
| `<tags>` | Browse posts (default) |
| `search <tags> [-n 25]` | Search and show table |
| `download <tags> [-n 25] [-o DIR]` | Download |
| `browse <tags> [-n 25]` | Paged thumbnail view |
| `info <post_id>` | Show post details |
| `dl <post_id...>` | Download specific posts |
| `export <tags> <json\|csv> <file>` | Export results |
| `presets` | List saved presets |
| `history` | Recent downloads |
| `help`, `quit` | Help or exit |

## Image Previews

Browse mode renders images as coloured ASCII art inline. Images are downloaded
to a temporary file, rendered, and immediately deleted — no storage is used.

## Tips

- Type a tag prefix and press Tab to see suggestions with post counts
- Use `--rating` and `--exclude` flags in any command
- Multi-tag searches use AND logic (posts must have all tags)
- When no results are found for a multi-tag search, a hint suggests trying
  individual tags
