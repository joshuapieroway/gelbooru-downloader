# Filtering & Search

## Tag Expressions

Tags are space-separated and combined with AND logic by default:

```bash
gelbooru search "blue_hair solo"          # posts with both blue_hair AND solo
gelbooru search "landscape sky"            # posts with both landscape AND sky
```

### Excluding Tags

Prefix a tag with `-` or `!` to exclude it:

```bash
gelbooru search "blue_hair -furry"         # blue_hair but not furry
gelbooru search "landscape !sky"           # landscape but not sky
```

### Rating Filter

```bash
gelbooru search "1girl" --rating safe,general
gelbooru search "landscape" --rating questionable,explicit
```

Available ratings: `safe`, `general`, `questionable`, `sensitive`,
`explicit`, `unrated`.

### Tag Exclusion (CLI Flag)

```bash
gelbooru search "femboy" --exclude muscular,buff,solo_male
```

### Extension Filter

```bash
gelbooru search "landscape" --ext jpg,png
```

### Dimension Filters

```bash
gelbooru search "1girl" --min-width 1000 --max-width 4000
gelbooru search "landscape" --min-height 500 --max-height 2000
```

### Size Filters

```bash
gelbooru search "1girl" --min-size 500KB --max-size 10MB
```

### Score Filter

```bash
gelbooru search "1girl" --min-score 10
```

### Sorting

```bash
gelbooru search "1girl" --order score      # by score
gelbooru search "1girl" --order width      # by width
gelbooru search "1girl" --order height     # by height
gelbooru search "1girl" --order filesize   # by file size
gelbooru search "1girl" --order id         # by ID (default)
gelbooru search "1girl" --shuffle          # random order
```

### Combining Filters

All filters can be combined:

```bash
gelbooru search "1girl blue_hair" \
  --rating safe,general \
  --exclude furry,muscular \
  --ext jpg,png \
  --min-width 800 \
  --min-score 5 \
  --order score \
  --limit 50
```
