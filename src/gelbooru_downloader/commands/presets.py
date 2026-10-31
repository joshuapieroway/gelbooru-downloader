"""``gelbooru presets`` — manage saved tag searches."""

from __future__ import annotations

from pathlib import Path
from typing import Annotated

import typer
from rich.table import Table

from ..app import get_context
from ..console import ok, warn
from ..utils import format_timestamp
from ._common import DownloadRequest, execute_download


def list_presets(ctx: typer.Context) -> None:
    """List every saved preset."""

    app = get_context(ctx)
    presets = list(app.presets)
    if not presets:
        app.console.print("[muted]No presets saved yet.[/] Add one with:")
        app.console.print('  [info]gelbooru presets add anime "blue_hair solo"[/]')
        return

    table = Table(header_style="table_header", border_style="grey30", expand=False)
    table.add_column("Name", style="id", no_wrap=True)
    table.add_column("Tags", overflow="fold", ratio=1)
    table.add_column("Folder", no_wrap=True)
    table.add_column("Limit", justify="right", no_wrap=True)
    table.add_column("Updated", no_wrap=True)
    for preset in presets:
        table.add_row(
            preset.name,
            preset.tags,
            preset.folder or "—",
            str(preset.limit) if preset.limit else "—",
            format_timestamp(preset.updated_at),
        )
    app.console.print(table)
    app.console.print(f"[muted]{len(presets)} preset(s) in {app.paths.presets_file}[/]")


def add_preset(
    ctx: typer.Context,
    name: Annotated[
        str, typer.Argument(help="Preset name (letters, digits, spaces, '-', '_', '.').")
    ],
    tags: Annotated[str, typer.Argument(help="Tag expression to remember.")],
    folder: Annotated[
        str | None,
        typer.Option("--folder", "-C", help="Default sub-folder for downloads from this preset."),
    ] = None,
    limit: Annotated[
        int | None,
        typer.Option("--limit", "-n", min=1, help="Default number of posts to download."),
    ] = None,
    notes: Annotated[
        str, typer.Option("--notes", help="Free-form note stored with the preset.")
    ] = "",
    force: Annotated[
        bool, typer.Option("--force", "-f", help="Overwrite an existing preset.")
    ] = False,
) -> None:
    """Save a tag search under a name."""

    app = get_context(ctx)
    app.paths.ensure()
    preset = app.presets.add(name, tags, folder=folder, notes=notes, limit=limit, overwrite=force)
    ok(app.console, f"saved preset '{preset.name}' → {preset.tags}")
    app.console.print(f"[muted]stored in {app.paths.presets_file}[/]")


def remove_preset(
    ctx: typer.Context,
    name: Annotated[str, typer.Argument(help="Preset to delete.")],
) -> None:
    """Delete a preset."""

    app = get_context(ctx)
    if app.presets.remove(name):
        ok(app.console, f"removed preset '{name}'")
        return
    warn(app.console, f"no preset named '{name}'")
    raise typer.Exit(1)


def show_preset(
    ctx: typer.Context,
    name: Annotated[str, typer.Argument(help="Preset to show.")],
) -> None:
    """Show a single preset in detail."""

    app = get_context(ctx)
    preset = app.presets.get_or_raise(name)
    app.console.print(f"[bold]{preset.name}[/]")
    app.console.print(f"  [muted]tags:[/]    {preset.tags}")
    app.console.print(f"  [muted]folder:[/]  {preset.folder or '—'}")
    app.console.print(f"  [muted]limit:[/]   {preset.limit or '—'}")
    app.console.print(f"  [muted]created:[/] {format_timestamp(preset.created_at)}")
    app.console.print(f"  [muted]updated:[/] {format_timestamp(preset.updated_at)}")
    if preset.notes:
        app.console.print(f"  [muted]notes:[/]   {preset.notes}")
    app.console.print(
        f"\n[info]download it with:[/] gelbooru presets download {preset.name} --limit {preset.limit or 50}"
    )


def download_preset(
    ctx: typer.Context,
    name: Annotated[str, typer.Argument(help="Preset to download from.")],
    limit: Annotated[
        int | None,
        typer.Option("--limit", "-n", min=0, help="Override the preset's limit (0 = no limit)."),
    ] = None,
    output: Annotated[
        Path | None, typer.Option("--output", "-o", help="Directory to write files into.")
    ] = None,
    concurrency: Annotated[
        int | None,
        typer.Option("--concurrency", "-c", min=1, max=64, help="Simultaneous downloads."),
    ] = None,
    page: Annotated[int, typer.Option("--page", "-p", min=1, help="First page to read from.")] = 1,
    overwrite: Annotated[
        bool, typer.Option("--overwrite", help="Re-download existing files.")
    ] = False,
    dry_run: Annotated[
        bool, typer.Option("--dry-run", help="List files without downloading.")
    ] = False,
    shuffle: Annotated[bool, typer.Option("--shuffle", help="Pick posts at random.")] = False,
    no_duplicates: Annotated[
        bool, typer.Option("--no-duplicates", help="Detect duplicates by SHA-256.")
    ] = False,
    archive: Annotated[
        bool | None, typer.Option("--archive/--no-archive", help="Write a .json sidecar per file.")
    ] = None,
    folder_by_tags: Annotated[
        bool,
        typer.Option("--folder-by-tags", help="Group files into folders named after their tags."),
    ] = False,
    rating: Annotated[
        list[str] | None,
        typer.Option(
            "--rating",
            "-r",
            help="Only these ratings, ORed when comma separated (repeatable).",
        ),
    ] = None,
    exclude: Annotated[
        list[str] | None, typer.Option("--exclude", "-x", help="Exclude these tags.")
    ] = None,
    max_pages: Annotated[
        int, typer.Option("--max-pages", min=1, help="Safety cap on API pages read.")
    ] = 50,
    no_progress: Annotated[
        bool, typer.Option("--no-progress", help="Hide the progress bar.")
    ] = False,
) -> None:
    """Download everything matching a saved preset."""

    app = get_context(ctx)
    preset = app.presets.get_or_raise(name)
    if no_progress:
        app.config.display.show_progress = False

    request = DownloadRequest(
        limit=limit if limit is not None else preset.limit,
        page=page,
        output=output or (Path(preset.folder) if preset.folder else None),
        concurrency=concurrency,
        overwrite=overwrite,
        dry_run=dry_run,
        shuffle=shuffle,
        duplicate_check=no_duplicates,
        archive=archive,
        collection=None,
        folder_by_tags=folder_by_tags,
        ratings=tuple(rating or ()),
        exclude=tuple(exclude or ()),
        max_pages=max_pages,
        order="score" if not shuffle else None,
    )
    app.banner(f"preset '{preset.name}' → {preset.tags}")

    if dry_run:
        from ..app import resolve_limit
        from ._common import download_options, plan_queries, print_collect_preview, run_search

        # Auto-detect sort order in tags and use index-based filenames
        if request.filename_template is None:
            request.filename_template = "{index}_{id}"
        effective = resolve_limit(request.limit, app.config)
        outcome = run_search(
            app,
            tags=preset.tags,
            limit=effective,
            ratings=request.ratings,
            exclude=request.exclude,
            order=request.order,
            page=page,
            page_size=app.config.general.page_size,
            post_filter=request.post_filter(),
            shuffle=shuffle,
            max_pages=max_pages,
        )
        if not outcome.posts:
            queries = plan_queries(preset.tags, ratings=request.ratings, exclude=request.exclude)
            warn(app.console, f"No posts matched preset '{preset.name}' ({' | '.join(queries)})")
            return
        options = download_options(app, request)
        ok(app.console, f"{len(outcome.posts)} file(s) would be downloaded — nothing was fetched")
        print_collect_preview(app, outcome, options=options)
        app.console.print(f"[muted]output directory:[/] {options.output_dir}")
        return

    summary = execute_download(app, preset.tags, request, title=f"Preset '{preset.name}'")
    if summary.failed:
        raise typer.Exit(1)
