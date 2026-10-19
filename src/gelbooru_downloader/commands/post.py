"""``gelbooru post`` — inspect and download individual posts by id."""

from __future__ import annotations

from pathlib import Path
from typing import Annotated, Any

import typer

from ..app import get_context, run_async
from ..console import ok, print_post_detail, warn
from ._common import DownloadRequest, download_options, download_posts


async def fetch_posts(app: Any, post_ids: list[int]) -> tuple[list[Any], list[int]]:
    """Fetch posts by id; returns ``(found_posts, missing_ids)``."""

    async with app.client() as client:
        posts = await client.get_posts(post_ids)
    found = {post.id for post in posts}
    missing = [post_id for post_id in post_ids if post_id not in found]
    return posts, missing


def post_info(
    ctx: typer.Context,
    post_id: Annotated[
        int,
        typer.Argument(metavar="POST_ID", help="The Gelbooru post id, e.g. 1234567."),
    ],
    json_output: Annotated[
        bool,
        typer.Option("--json", help="Print the post metadata as JSON."),
    ] = False,
) -> None:
    """Show everything the API knows about a single post."""

    import json

    app = get_context(ctx)
    with app.console.status(f"[brand]fetching post {post_id}…", spinner="dots"):
        posts, missing = run_async(fetch_posts(app, [post_id]))

    if missing:
        warn(app.console, f"post {post_id} was not found — it may be deleted or never existed")
        raise typer.Exit(1)

    post = posts[0]
    if json_output:
        app.console.print_json(json.dumps(post.to_dict()))
        return
    print_post_detail(app.console, post)
    if not post.is_available:
        app.console.print(
            "[warn]this post cannot be downloaded:[/] "
            f"{post.unavailable_reason}. Gelbooru did not provide a usable file URL."
        )


def post_download(
    ctx: typer.Context,
    post_ids: Annotated[
        list[int],
        typer.Argument(metavar="[POST_ID]...", help="One or more post ids to download."),
    ],
    output: Annotated[
        Path | None, typer.Option("--output", "-o", help="Directory to write files into.")
    ] = None,
    concurrency: Annotated[
        int | None,
        typer.Option("--concurrency", "-c", min=1, max=64, help="Simultaneous downloads."),
    ] = None,
    overwrite: Annotated[
        bool, typer.Option("--overwrite", help="Re-download files that exist.")
    ] = False,
    dry_run: Annotated[
        bool, typer.Option("--dry-run", help="Show what would be downloaded.")
    ] = False,
    archive: Annotated[
        bool | None, typer.Option("--archive/--no-archive", help="Write a .json sidecar per file.")
    ] = None,
    collection: Annotated[
        str | None, typer.Option("--collection", "-C", help="Store files in a sub-folder.")
    ] = None,
    no_progress: Annotated[
        bool, typer.Option("--no-progress", help="Hide the progress bar.")
    ] = False,
) -> None:
    """Download one or more posts by id, at original quality."""

    app = get_context(ctx)
    if not post_ids:
        raise typer.BadParameter("at least one post id is required", param_hint="POST_ID")

    if no_progress:
        app.config.display.show_progress = False

    with app.console.status(f"[brand]fetching {len(post_ids)} post(s)…", spinner="dots"):
        posts, missing = run_async(fetch_posts(app, list(post_ids)))

    for post_id in missing:
        warn(app.console, f"post {post_id}: not found on Gelbooru")

    if not posts:
        raise typer.Exit(1)

    request = DownloadRequest(
        output=output,
        concurrency=concurrency,
        overwrite=overwrite,
        dry_run=dry_run,
        archive=archive,
        collection=collection,
    )
    app.banner(f"downloading post(s) {', '.join(str(post.id) for post in posts)}")

    if dry_run:
        from ._common import SearchOutcome, print_collect_preview

        ok(app.console, f"{len(posts)} file(s) would be downloaded — nothing was fetched")
        print_collect_preview(
            app,
            SearchOutcome(posts=list(posts), fetched=len(posts)),
            options=download_options(app, request),
        )
        return

    from ..console import print_summary

    summary = run_async(download_posts(app, posts, request, title="Downloading posts"))
    print_summary(app.console, summary, output_dir=download_options(app, request).output_dir)
    if summary.failed:
        raise typer.Exit(1)
