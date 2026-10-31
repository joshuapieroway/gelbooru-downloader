"""``gelbooru batch`` and ``gelbooru watch`` — many searches, or repeating one."""

from __future__ import annotations

import time
from pathlib import Path
from typing import Annotated, Any

import typer

from ..app import get_context, resolve_limit, run_async
from ..console import ok, print_summary, warn
from ..models import DownloadSummary
from ..utils import human_duration
from ._common import (
    DownloadRequest,
    download_options,
    download_posts,
    search_posts,
)


def read_tag_file(path: Path) -> list[str]:
    """Read a tag list file: one search per line, ``#`` starts a comment."""

    path = Path(path).expanduser()
    if not path.is_file():
        msg = f"tag file not found: {path}"
        raise typer.BadParameter(msg, param_hint="TAGS_FILE")
    searches: list[str] = []
    with path.open(encoding="utf-8") as handle:
        for line in handle:
            cleaned = line.split("#", 1)[0].strip()
            if cleaned:
                searches.append(cleaned)
    if not searches:
        msg = f"{path} contains no searches (one tag expression per line, '#' comments are ignored)"
        raise typer.BadParameter(msg, param_hint="TAGS_FILE")
    return searches


def batch(
    ctx: typer.Context,
    tags_file: Annotated[
        Path,
        typer.Argument(metavar="TAGS_FILE", help="Text file with one tag expression per line."),
    ],
    output: Annotated[
        Path | None, typer.Option("--output", "-o", help="Directory to write files into.")
    ] = None,
    limit: Annotated[
        int, typer.Option("--limit", "-n", min=0, help="Maximum posts per line (0 = no limit).")
    ] = 25,
    concurrency: Annotated[
        int | None,
        typer.Option("--concurrency", "-c", min=1, max=64, help="Simultaneous downloads."),
    ] = None,
    folder_by_line: Annotated[
        bool,
        typer.Option(
            "--folder-by-line/--no-folder-by-line",
            help="Give every line its own sub-folder named after its first tag (default: on).",
        ),
    ] = True,
    overwrite: Annotated[
        bool, typer.Option("--overwrite", help="Re-download existing files.")
    ] = False,
    dry_run: Annotated[
        bool, typer.Option("--dry-run", help="List files without downloading.")
    ] = False,
    no_duplicates: Annotated[
        bool, typer.Option("--no-duplicates", help="Detect duplicates by SHA-256.")
    ] = False,
    archive: Annotated[
        bool | None, typer.Option("--archive/--no-archive", help="Write a .json sidecar per file.")
    ] = None,
    page: Annotated[int, typer.Option("--page", "-p", min=1, help="First page to read from.")] = 1,
    max_pages: Annotated[
        int, typer.Option("--max-pages", min=1, help="Safety cap on API pages read.")
    ] = 50,
    continue_on_error: Annotated[
        bool,
        typer.Option("--continue-on-error/--stop-on-error", help="Keep going when a search fails."),
    ] = True,
    no_progress: Annotated[
        bool, typer.Option("--no-progress", help="Hide the progress bar.")
    ] = False,
) -> None:
    """Run many searches from a file, downloading the results of each."""

    app = get_context(ctx)
    searches = read_tag_file(tags_file)
    if no_progress:
        app.config.display.show_progress = False

    app.banner(f"batch: {len(searches)} search(es) from {tags_file}")
    totals = DownloadSummary()
    failed_searches: list[tuple[str, str]] = []

    for index, search in enumerate(searches, 1):
        app.console.print(f"\n[brand_dim]({index}/{len(searches)})[/] [bold]{search}[/]")
        request = DownloadRequest(
            limit=limit,
            page=page,
            output=output,
            concurrency=concurrency,
            overwrite=overwrite,
            dry_run=dry_run,
            duplicate_check=no_duplicates,
            archive=archive,
            collection=_line_folder(search) if folder_by_line else None,
            max_pages=max_pages,
        )
        try:
            summary = run_async(_download_line(app, search, request))
        except Exception as exc:
            if not continue_on_error:
                raise
            warn(app.console, f"search failed: {exc}")
            failed_searches.append((search, str(exc)))
            continue
        totals = totals.extend(summary)

    print_summary(
        app.console,
        totals,
        output_dir=download_options(app, _base_request(output)).output_dir,
        title="Batch summary",
    )
    if failed_searches:
        app.console.print(f"[warn]{len(failed_searches)} search(es) failed:[/]")
        for search, message in failed_searches[:5]:
            app.console.print(f"  [muted]{search}[/] → {message}")
        raise typer.Exit(1)


def _base_request(output: Path | None) -> DownloadRequest:
    """The request used only to resolve the default output directory."""

    return DownloadRequest(output=output)


def _line_folder(search: str) -> str:
    from ..utils import sanitize_path_component

    first = search.split(maxsplit=1)[0].lstrip("-!") if search.strip() else "batch"
    return sanitize_path_component(first, fallback="batch")


async def _download_line(app: Any, search: str, request: DownloadRequest) -> DownloadSummary:
    from ..console import warn

    limit = resolve_limit(request.limit, app.config)
    outcome = await search_posts(
        app,
        tags=search,
        limit=limit,
        ratings=request.ratings,
        exclude=request.exclude,
        order="score",
        page=request.page,
        page_size=app.config.general.page_size,
        post_filter=request.post_filter(),
        shuffle=request.shuffle,
        max_pages=request.max_pages,
    )
    if not outcome.posts:
        warn(app.console, f"no posts matched '{search}'")
        return DownloadSummary()
    # Auto-detect sort order in tags and use index-based filenames
    if request.filename_template is None:
        request.filename_template = "{index}_{id}"
    return await download_posts(app, outcome.posts, request, title=f'Downloading "{search[:32]}"')


def watch(
    ctx: typer.Context,
    tags: Annotated[str, typer.Argument(metavar="TAGS", help="Tag expression to watch.")],
    interval: Annotated[
        int, typer.Option("--interval", "-i", min=5, help="Seconds between runs (minimum 5).")
    ] = 300,
    limit: Annotated[
        int, typer.Option("--limit", "-n", min=1, help="Posts fetched per run.")
    ] = 100,
    output: Annotated[
        Path | None, typer.Option("--output", "-o", help="Directory to write files into.")
    ] = None,
    concurrency: Annotated[
        int | None,
        typer.Option("--concurrency", "-c", min=1, max=64, help="Simultaneous downloads."),
    ] = None,
    once: Annotated[bool, typer.Option("--once", help="Run a single cycle and exit.")] = False,
    runs: Annotated[
        int, typer.Option("--runs", min=0, help="Stop after N cycles (0 = forever).")
    ] = 0,
    overwrite: Annotated[
        bool, typer.Option("--overwrite", help="Re-download existing files.")
    ] = False,
    no_duplicates: Annotated[
        bool, typer.Option("--no-duplicates", help="Detect duplicates by SHA-256.")
    ] = False,
    archive: Annotated[
        bool | None, typer.Option("--archive/--no-archive", help="Write a .json sidecar per file.")
    ] = None,
    no_progress: Annotated[
        bool, typer.Option("--no-progress", help="Hide the progress bar.")
    ] = False,
) -> None:
    """Periodically repeat a search and download newly appearing posts.

    Already downloaded files are skipped, so each cycle only fetches what is new.
    """

    app = get_context(ctx)
    if no_progress:
        app.config.display.show_progress = False

    request = DownloadRequest(
        limit=limit,
        output=output,
        concurrency=concurrency,
        overwrite=overwrite,
        duplicate_check=no_duplicates,
        archive=archive,
    )
    app.banner(f'watching "{tags}" every {interval}s')

    cycle = 0
    grand_total = DownloadSummary()
    started = time.monotonic()

    while True:
        cycle += 1
        app.console.print(
            f"\n[brand]cycle {cycle}[/] [muted]{time.strftime('%H:%M:%S')} · looking for new posts…[/]"
        )
        try:
            summary = run_async(_download_line(app, tags, request))
        except Exception as exc:
            warn(app.console, f"cycle {cycle} failed: {exc}")
            summary = DownloadSummary()

        grand_total = grand_total.extend(summary)
        if summary.completed or summary.skipped:
            print_summary(
                app.console,
                summary,
                output_dir=download_options(app, request).output_dir,
                title=f"Cycle {cycle}",
            )
        else:
            app.console.print(
                f"[muted]nothing new this cycle ({human_duration(time.monotonic() - started)} elapsed)[/]"
            )

        if once or (runs and cycle >= runs):
            break
        try:
            _sleep_until(interval)
        except KeyboardInterrupt:
            app.console.print("\n[warn]stopping — press Ctrl+C again to force quit[/]")
            break

    print_summary(
        app.console,
        grand_total,
        output_dir=download_options(app, request).output_dir,
        title="Watch summary",
    )
    ok(app.console, f"completed {cycle} cycle(s) in {human_duration(time.monotonic() - started)}")


def _sleep_until(seconds: int) -> None:
    """Sleep in one-second steps so Ctrl+C is responsive."""

    for _ in range(max(1, seconds)):
        time.sleep(1)
