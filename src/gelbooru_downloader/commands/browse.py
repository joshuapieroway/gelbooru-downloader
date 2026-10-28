"""``gelbooru browse`` — a paged, thumbnail-first view of search results."""

from __future__ import annotations

from typing import Annotated, Any

import typer
from rich.prompt import Prompt

from ..console import _rating_text, warn
from ._common import run_search
from .interactive import _render_preview, _supports_inline_images


def run_browse(
    app: Any,
    tags: str,
    requested: int,
    want_images: bool,
    *,
    ratings: list[str] | None = None,
    exclude: list[str] | None = None,
) -> None:
    """Fetch ``tags`` and page through the results with thumbnails and titles."""

    with app.console.status(f"[brand]searching {tags}…", spinner="dots"):
        outcome = run_search(
            app,
            tags=tags,
            limit=requested,
            ratings=ratings,
            exclude=exclude,
            page_size=app.config.general.page_size,
        )
    if not outcome.posts:
        warn(app.console, f"nothing matched '{tags}'")
        return

    posts = outcome.posts
    page_size = 3
    page = 0
    total_pages = max(1, -(-len(posts) // page_size))
    while True:
        app.console.print(f"[muted]page {page + 1}/{total_pages} · {len(posts)} post(s)[/]")
        start = page * page_size
        for offset, post in enumerate(posts[start : start + page_size], start):
            app.console.print(
                f"[id]{offset + 1}.[/] [id]{post.id}[/]  {_rating_text(post.rating)}  "
                f"{post.width or '?'}×{post.height or '?'}  {post.summary(4)}"
            )
            if want_images:
                _render_preview(app.console, post.preview_url or post.file_url)
            elif post.preview_url:
                app.console.print(f"   [muted]{post.preview_url}[/]")
        app.console.print("")
        choices = []
        if page < total_pages - 1:
            choices.append("next (n)")
        if page > 0:
            choices.append("prev (p)")
        choices.append("quit (q)")
        prompt = " / ".join(choices)
        try:
            raw = Prompt.ask(f"[brand]browse[/] ({prompt})", default="q", show_default=False)
        except (EOFError, KeyboardInterrupt):
            return
        raw = raw.strip().lower()
        if raw in {"q", "quit", "exit"}:
            return
        if raw in {"n", "next"} and page < total_pages - 1:
            page += 1
        elif raw in {"p", "prev", "previous"} and page > 0:
            page -= 1
        else:
            warn(app.console, f"unknown command {raw!r}")


def browse(
    ctx: typer.Context,
    tags: Annotated[
        str,
        typer.Argument(metavar="TAGS", help="Gelbooru tag expression, e.g. 'blue_hair solo'."),
    ],
    limit: Annotated[
        int, typer.Option("--limit", "-n", min=0, help="Maximum number of posts (0 = no limit).")
    ] = 50,
    images: Annotated[
        bool, typer.Option("--images", help="Render preview images inline (Kitty terminals).")
    ] = False,
    rating: Annotated[
        list[str] | None,
        typer.Option(
            "--rating",
            "-r",
            help="Filter by rating (safe, general, questionable, sensitive, explicit, unrated).",
        ),
    ] = None,
    exclude: Annotated[
        list[str] | None,
        typer.Option("--exclude", "-x", help="Exclude tags (comma separated, e.g. muscular,buff)."),
    ] = None,
) -> None:
    """Browse search results as a paged list with thumbnails and titles."""

    from ..app import get_context

    app = get_context(ctx)
    app.banner("browse")
    want_images = images or _supports_inline_images()
    run_browse(app, tags, (limit or None), want_images, ratings=rating, exclude=exclude)
