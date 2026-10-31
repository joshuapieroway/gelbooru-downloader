"""``gelbooru search`` — find posts and render or export them."""

from __future__ import annotations

from pathlib import Path
from typing import Annotated

import typer

from ..app import get_context, resolve_limit
from ..console import format_post_table, print_search_summary, warn
from ..utils import human_bytes
from ._common import (
    build_filter_from_options,
    export_posts,
    plan_queries,
    print_collect_preview,
    run_search,
    size_option,
)


def search(
    ctx: typer.Context,
    tags: Annotated[
        str,
        typer.Argument(
            metavar="TAGS",
            help="Gelbooru tag expression, e.g. 'blue_hair solo'. Prefix a tag with '-' or '!' to exclude it.",
        ),
    ],
    limit: Annotated[
        int,
        typer.Option(
            "--limit", "-n", min=0, help="Maximum number of matching posts (0 = no limit)."
        ),
    ] = 50,
    page: Annotated[
        int, typer.Option("--page", "-p", min=1, help="First page to read from (1 = newest first).")
    ] = 1,
    page_size: Annotated[
        int | None,
        typer.Option(
            "--page-size",
            min=1,
            max=100,
            help="Posts requested per API page (Gelbooru caps this at 100).",
        ),
    ] = None,
    rating: Annotated[
        list[str] | None,
        typer.Option(
            "--rating",
            "-r",
            help=(
                "Only these ratings, ORed when comma separated (repeatable). "
                "Values: safe, general, questionable, sensitive, explicit, unrated."
            ),
        ),
    ] = None,
    exclude: Annotated[
        list[str] | None,
        typer.Option("--exclude", "-x", help="Exclude these tags (comma separated, repeatable)."),
    ] = None,
    extension: Annotated[
        list[str] | None,
        typer.Option("--ext", "-e", help="Only these file extensions, e.g. jpg, png (repeatable)."),
    ] = None,
    min_width: Annotated[
        int | None, typer.Option("--min-width", min=0, help="Minimum width in pixels.")
    ] = None,
    max_width: Annotated[
        int | None, typer.Option("--max-width", min=0, help="Maximum width in pixels.")
    ] = None,
    min_height: Annotated[
        int | None, typer.Option("--min-height", min=0, help="Minimum height in pixels.")
    ] = None,
    max_height: Annotated[
        int | None, typer.Option("--max-height", min=0, help="Maximum height in pixels.")
    ] = None,
    min_size: Annotated[
        str | None,
        typer.Option(
            "--min-size",
            callback=lambda value: size_option(value, what="--min-size"),
            help="Minimum file size, e.g. 500KB.",
        ),
    ] = None,
    max_size: Annotated[
        str | None,
        typer.Option(
            "--max-size",
            callback=lambda value: size_option(value, what="--max-size"),
            help="Maximum file size, e.g. 10MB.",
        ),
    ] = None,
    min_score: Annotated[
        int | None, typer.Option("--min-score", min=0, help="Minimum post score.")
    ] = None,
    order: Annotated[
        str | None,
        typer.Option(
            "--order", "-o", help="Gelbooru order metatag, e.g. score, width, height, filesize, id."
        ),
    ] = None,
    shuffle: Annotated[
        bool, typer.Option("--shuffle", help="Randomise the order of the results.")
    ] = False,
    output_format: Annotated[
        str,
        typer.Option("--format", "-f", help="Output format: table (default), json or csv."),
    ] = "table",
    output: Annotated[
        Path | None,
        typer.Option("--output", "-O", help="Write results to this file instead of stdout."),
    ] = None,
    max_pages: Annotated[
        int,
        typer.Option("--max-pages", min=1, help="Safety cap on how many API pages are read."),
    ] = 50,
    dry_run: Annotated[
        bool,
        typer.Option(
            "--dry-run", help="Show which files a download would create, without fetching them."
        ),
    ] = False,
) -> None:
    """Search Gelbooru for posts matching the given tags."""

    app = get_context(ctx)
    app.banner("search")
    queries = plan_queries(tags, ratings=rating, exclude=exclude)
    effective_limit = resolve_limit(limit or None, app.config)
    post_filter = build_filter_from_options(
        ratings=rating,
        exclude=exclude,
        extensions=extension,
        min_width=min_width,
        max_width=max_width,
        min_height=min_height,
        max_height=max_height,
        min_size=size_option(min_size, what="--min-size"),
        max_size=size_option(max_size, what="--max-size"),
        min_score=min_score,
    )

    with app.console.status(
        f"[brand]searching Gelbooru for [bold]{' | '.join(queries)}[/]…", spinner="dots"
    ):
        outcome = run_search(
            app,
            tags=tags,
            limit=effective_limit,
            ratings=rating,
            exclude=exclude,
            order=order,
            page=page,
            page_size=page_size or app.config.general.page_size,
            post_filter=post_filter,
            shuffle=shuffle,
            max_pages=max_pages,
        )

    if not outcome.posts:
        warn(
            app.console,
            f"No posts matched '{' | '.join(queries)}' (fetched {outcome.fetched} results)",
        )
        if outcome.rejected:
            reasons = ", ".join(sorted({reason for _post, reason in outcome.rejected})[:3])
            app.console.print(f"  [muted]most were filtered out: {reasons}[/]")
        raise typer.Exit(0)

    if dry_run:
        app.console.print(f"[ok]{len(outcome.posts)}[/] post(s) match; nothing was downloaded.")
        print_collect_preview(app, outcome)
        return

    normalized_format = output_format.strip().lower()
    if normalized_format in ("json", "csv") or output is not None:
        if normalized_format not in ("json", "csv"):
            normalized_format = "json"
        destination = export_posts(
            outcome.posts, output or Path(f"gelbooru-search.{normalized_format}"), normalized_format
        )
        from ..console import ok

        ok(app.console, f"Wrote {len(outcome.posts)} post(s) to {destination}")
        return

    if normalized_format != "table":
        raise typer.BadParameter(
            f"unknown --format {output_format!r} (use table, json or csv)", param_hint="--format"
        )

    app.console.print(
        format_post_table(
            outcome.posts,
            max_rows=app.config.display.table_max_rows,
            tag_count=app.config.display.tag_preview_count,
            width=app.console.width,
        )
    )
    print_search_summary(
        app.console,
        total_fetched=outcome.fetched,
        shown=min(len(outcome.posts), app.config.display.table_max_rows),
        matched=outcome.matched,
        filtered_out=outcome.filtered_out,
        total_available=outcome.total_available,
        elapsed=outcome.elapsed,
    )
    known = [post.file_size for post in outcome.posts if post.file_size]
    if known:
        app.console.print(
            f"[muted]total size:[/] {human_bytes(sum(known))} [muted]across {len(known)} post(s) · "
            f'[muted]download with:[/] gelbooru download "{tags}" --limit {len(outcome.posts)}'
        )
