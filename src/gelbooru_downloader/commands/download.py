"""``gelbooru download`` — search and download in one step."""

from __future__ import annotations

from pathlib import Path
from typing import Annotated, Any

import typer

from ..app import get_context
from ..console import ok
from ._common import (
    DownloadRequest,
    download_options,
    execute_download,
    plan_queries,
    print_collect_preview,
    run_search,
    size_option,
)


def download(
    ctx: typer.Context,
    tags: Annotated[
        str,
        typer.Argument(
            metavar="TAGS",
            help="Gelbooru tag expression, e.g. 'landscape'. Use '-tag' or '!tag' to exclude.",
        ),
    ],
    limit: Annotated[
        int,
        typer.Option(
            "--limit", "-n", min=0, help="Maximum number of posts to download (0 = no limit)."
        ),
    ] = 50,
    page: Annotated[int, typer.Option("--page", "-p", min=1, help="First page to read from.")] = 1,
    page_size: Annotated[
        int | None,
        typer.Option(
            "--page-size",
            min=1,
            max=100,
            help="Posts requested per API page (Gelbooru caps this at 100).",
        ),
    ] = None,
    output: Annotated[
        Path | None,
        typer.Option("--output", "-o", help="Directory to write files into."),
    ] = None,
    concurrency: Annotated[
        int | None,
        typer.Option("--concurrency", "-c", min=1, max=64, help="Simultaneous downloads."),
    ] = None,
    retries: Annotated[
        int | None, typer.Option("--retries", min=0, max=10, help="Retries per file.")
    ] = None,
    timeout: Annotated[
        float | None, typer.Option("--timeout", min=1.0, help="Per-request timeout in seconds.")
    ] = None,
    overwrite: Annotated[
        bool,
        typer.Option("--overwrite", help="Re-download files that already exist."),
    ] = False,
    skip_existing: Annotated[
        bool | None,
        typer.Option(
            "--skip-existing/--no-skip-existing", help="Skip files that are already on disk."
        ),
    ] = None,
    dry_run: Annotated[
        bool,
        typer.Option("--dry-run", help="List the files that would be downloaded and stop."),
    ] = False,
    shuffle: Annotated[
        bool,
        typer.Option("--shuffle", help="Pick posts at random from the search results."),
    ] = False,
    no_duplicates: Annotated[
        bool,
        typer.Option("--no-duplicates", help="Detect duplicate files by SHA-256 and skip them."),
    ] = False,
    archive: Annotated[
        bool | None,
        typer.Option(
            "--archive/--no-archive", help="Write a .json metadata sidecar next to every file."
        ),
    ] = None,
    collection: Annotated[
        str | None,
        typer.Option("--collection", "-C", help="Store files in a sub-folder with this name."),
    ] = None,
    folder_by_tags: Annotated[
        bool,
        typer.Option(
            "--folder-by-tags", help="Group files into folders named after their leading tags."
        ),
    ] = False,
    filename_template: Annotated[
        str | None,
        typer.Option(
            "--filename", "-F", help="Filename template: {id}, {md5}, {rating}, {index}, {score}."
        ),
    ] = None,
    rating: Annotated[
        list[str] | None,
        typer.Option(
            "--rating",
            "-r",
            help=(
                "Only these ratings, ORed when comma separated: "
                "--rating questionable,explicit (repeatable). Values: safe, general, "
                "questionable, sensitive, explicit, unrated."
            ),
        ),
    ] = None,
    exclude: Annotated[
        list[str] | None,
        typer.Option("--exclude", "-x", help="Exclude these tags (comma separated, repeatable)."),
    ] = None,
    extension: Annotated[
        list[str] | None,
        typer.Option("--ext", "-e", help="Only these file extensions (repeatable)."),
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
            help="Skip files smaller than this, e.g. 500KB.",
        ),
    ] = None,
    max_size: Annotated[
        str | None,
        typer.Option(
            "--max-size",
            callback=lambda value: size_option(value, what="--max-size"),
            help="Skip files larger than this, e.g. 10MB.",
        ),
    ] = None,
    min_score: Annotated[
        int | None, typer.Option("--min-score", min=0, help="Minimum post score.")
    ] = None,
    order: Annotated[
        str | None,
        typer.Option(
            "--order", "-O", help="Gelbooru order metatag: score, width, height, filesize, id."
        ),
    ] = None,
    max_pages: Annotated[
        int, typer.Option("--max-pages", min=1, help="Safety cap on API pages read.")
    ] = 50,
    no_progress: Annotated[
        bool, typer.Option("--no-progress", help="Hide the progress bar.")
    ] = False,
) -> None:
    """Search Gelbooru and download every matching post."""

    app = get_context(ctx)
    request = _request_from_options(
        limit=limit,
        page=page,
        page_size=page_size,
        output=output,
        concurrency=concurrency,
        retries=retries,
        timeout=timeout,
        overwrite=overwrite,
        skip_existing=skip_existing,
        dry_run=dry_run,
        shuffle=shuffle,
        duplicate_check=no_duplicates,
        archive=archive,
        collection=collection,
        folder_by_tags=folder_by_tags,
        filename_template=filename_template,
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
        order=order,
        max_pages=max_pages,
    )
    if no_progress:
        app.config.display.show_progress = False

    # Auto-detect sort order in tags and use index-based filenames
    if request.filename_template is None and ("sort:" in tags.lower() or "order:" in tags.lower()):
        request.filename_template = "{index}_{id}"

    app.banner(f'downloading "{tags}"')
    if dry_run:
        _dry_run(app, tags, request)
        return

    summary = execute_download(app, tags, request, title=f'Downloading "{tags[:40]}"')
    if summary.completed or summary.skipped:
        from ._common import download_options

        ok(app.console, f"files are in {download_options(app, request).output_dir}")
    if summary.failed:
        raise typer.Exit(1)


def _dry_run(app: Any, tags: str, request: DownloadRequest) -> None:
    """``--dry-run``: search, show the plan, download nothing."""

    from ..app import resolve_limit

    limit = resolve_limit(request.limit, app.config)
    outcome = run_search(
        app,
        tags=tags,
        limit=limit,
        ratings=request.ratings,
        exclude=request.exclude,
        order=request.order,
        page=request.page,
        page_size=request.page_size or app.config.general.page_size,
        post_filter=request.post_filter(),
        shuffle=request.shuffle,
        max_pages=request.max_pages,
    )
    if not outcome.posts:
        from ..console import warn

        queries = plan_queries(
            tags, ratings=request.ratings, exclude=request.exclude, order=request.order
        )
        warn(app.console, f"No posts matched '{' | '.join(queries)}'")
        return
    options = download_options(app, request)
    ok(app.console, f"{len(outcome.posts)} file(s) would be downloaded — nothing was fetched")
    print_collect_preview(app, outcome, options=options)
    app.console.print(f"[muted]output directory:[/] {options.output_dir}")


def _request_from_options(**options: Any) -> DownloadRequest:
    """Build a :class:`DownloadRequest` from CLI options.

    Unknown keys raise instead of being dropped: silently ignoring a misspelled
    option would apply a filter the user asked for but never got.
    """

    known = set(DownloadRequest.__slots__)
    unknown = sorted(set(options) - known - {"None"})
    if unknown:
        msg = f"internal error: unknown download option(s) {', '.join(unknown)}"
        raise ValueError(msg)
    filtered = {key: value for key, value in options.items() if key in known}
    return DownloadRequest(**filtered)
