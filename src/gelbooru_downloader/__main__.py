"""Allow ``python -m gelbooru_downloader`` in addition to the ``gelbooru`` script."""

from __future__ import annotations

import sys

from .cli import main

if __name__ == "__main__":
    sys.exit(main())
