# Troubleshooting

## Common Issues

### "No API credentials configured"
Gelbooru's dapi requires an API key. Set environment variables:

```bash
export GELBOORU_USER_ID=1234
export GELBOORU_API_KEY=your_key_here
```

Or add them to your config file:
```toml
[network]
user_id = "1234"
api_key = "your_key_here"
```

### "Too deep! Pull it back some."
The dapi only serves roughly the newest 20,000 posts. Try:
- A smaller `--page`
- Narrower tags

### Images Not Rendering in Browse
- Browse uses coloured ASCII art which works in any terminal
- If images appear as escape sequences, your terminal may not support
  24-bit colour — try a different terminal

### Slow Tag Autocomplete
- First autocomplete for a prefix fetches all matching tags from Gelbooru
  (may take a few seconds for common prefixes like "b" or "bl")
- Results are cached for 5 minutes
- Subsequent autocompletions for the same or longer prefixes are instant

### Download Failures
- Check `gelbooru config history --status failed` for details
- Common causes: rate limiting, network timeouts, invalid content type
- Use `--retries` to increase retry attempts
- Use `--timeout` to increase per-request timeout

### Config File Issues

```bash
# Show where config lives
gelbooru config path

# Reset to defaults
gelbooru config init --force

# Show effective config
gelbooru config show
```

### Database Issues

```bash
# Check history
gelbooru config history --stats

# Clear history (doesn't delete files)
gelbooru config clear --yes
```

### Permission Issues

Config and preset files are created with mode `0600`. If you see permission
errors, check file ownership:

```bash
ls -la ~/.config/gelbooru-downloader/
```

## Known Limitations

- API paging stops near pid 20000
- Client-side filtering costs bandwidth
- `file_size` is absent from the JSON API
- Auth is mandatory
- No HTML scraping
- Rate limits are the server's decision
- `created_at` may contain non-English month abbreviations
