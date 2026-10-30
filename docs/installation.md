# Installation

## Requirements

- Python 3.12 or newer
- A Gelbooru API key (free account at https://gelbooru.com)

## Quick Install

```bash
git clone https://github.com/joshuapieroway/gelbooru-downloader.git
cd gelbooru-downloader
python -m venv .venv
.venv/bin/pip install -e .
```

The `gelbooru` console script is now available at `.venv/bin/gelbooru`.

You can also run it as a module:

```bash
.venv/bin/python -m gelbooru_downloader
```

## Credentials

Gelbooru's dapi requires an API key. Set these environment variables:

```bash
export GELBOORU_USER_ID=1234
export GELBOORU_API_KEY=your_key_here
```

Create a key at https://gelbooru.com/account. Credentials can also be stored in
the config file (see [Configuration](configuration.md)).

## Verify Installation

```bash
gelbooru version
```

This should print the version, Python version, and configuration paths.

## Development Setup

For development (includes tests and linting tools):

```bash
.venv/bin/pip install -e ".[dev]"
```

Tests live on the `development` branch. On `main`:

```bash
.venv/bin/ruff check src      # lint
.venv/bin/ruff format src     # format
```
