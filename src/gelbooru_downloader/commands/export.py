"""``gelbooru export`` — write search results to JSON or CSV."""

from __future__ import annotations

from pathlib import Path
from typing import Annotated

import typer

from ..app import get_context
from ..console import ok, warn
from ._common import run_search


def export(
    ctx: typer.Context,
    tags: Annotated[
        str,
        typer.Argument(metavar="TAGS", help="Gelbooru tag expression, e.g. 'blue_hair solo'."),
    ],
    fmt: Annotated[
        str,
        typer.Argument(metavar="FORMAT", help="Output format: json or csv."),
    ],
    destination: Annotated[
        Path,
        typer.Argument(metavar="FILE", help="Destination file path."),
    ],
    limit: Annotated[
        int, typer.Option("--limit", "-n", min=0, help="Maximum number of posts (0 = no limit).")
    ] = 50,
) -> None:
    """Export search results to JSON or CSV without downloading anything."""

    app = get_context(ctx)
    app.banner("export")
    requested = limit or None
    outcome = run_search(
        app,
        tags=tags,
        limit=requested,
        page_size=app.config.general.page_size,
    )
    if not outcome.posts:
        warn(app.console, "nothing to export")
        return
    from ._common import export_posts

    try:
        path = export_posts(outcome.posts, destination, fmt)
    except ValueError as exc:
        warn(app.console, str(exc))
        return
    ok(app.console, f"exported {len(outcome.posts)} post(s) to {path}")
