"""Gelbooru Downloader — a terminal client for the Gelbooru image board API.

Public API::

    from gelbooru_downloader import GelbooruClient, Downloader, Config

Everything else lives in focused submodules:

``config``      configuration, environment overrides and platform paths
``api``         async Gelbooru dapi client (JSON + XML)
``models``      posts, pages and download results
``filters``     client-side filtering of API results
``downloader``  concurrent, resumable, atomic downloads
``database``    SQLite download history
``presets``     named tag presets
``console``     Rich rendering (banner, tables, progress, reports)
``cli``         the Typer command line interface
"""

from __future__ import annotations

from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as _version

__all__ = ["Config", "Downloader", "GelbooruClient", "__version__"]

try:  # pragma: no cover - depends on install method
    __version__ = _version("gelbooru-downloader")
except PackageNotFoundError:  # pragma: no cover - running from a source checkout
    __version__ = "1.1.0"

# Public API re-exports (lazy-ish: imported at module load).
from .api import GelbooruClient
from .config import Config
from .downloader import Downloader

__title__ = "gelbooru-downloader"
__author__ = "Gelbooru Downloader Contributors"
__license__ = "MIT"
