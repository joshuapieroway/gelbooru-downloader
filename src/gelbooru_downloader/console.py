"""Rich powered terminal presentation.

Everything the user sees goes through this module: the banner, result tables,
progress bars, post detail panels and the final download report. Keeping it in
one place makes the CLI commands easy to read and the look consistent.
"""

from __future__ import annotations

from collections.abc import Sequence
from pathlib import Path
from typing import Any, TextIO

from rich.console import Console, Group, RenderableType
from rich.panel import Panel
from rich.progress import (
    BarColumn,
    MofNCompleteColumn,
    Progress,
    SpinnerColumn,
    TextColumn,
    TimeElapsedColumn,
    TransferSpeedColumn,
)
from rich.table import Table
from rich.text import Text
from rich.theme import Theme

from .downloader import DownloadEvent
from .models import DownloadResult, DownloadStatus, DownloadSummary, Post
from .utils import format_timestamp, human_bytes, human_duration, truncate_tags

__all__ = [
    "GELBOORU_THEME",
    "ConsoleFactory",
    "RichProgressReporter",
    "banner",
    "format_post_table",
    "print_post_detail",
    "print_search_summary",
    "print_summary",
    "render_summary_panel",
]

GELBOORU_THEME = Theme(
    {
        "brand": "bold #4a9eff",
        "brand_dim": "#2d6bb5",
        "table_header": "bold #4a9eff",
        "error_header": "bold red",
        "ok": "bold #4a9eff",
        "warn": "yellow",
        "err": "bold red",
        "info": "#4a9eff",
        "muted": "grey58",
        "id": "bold #7cc4ff",
        "rating_safe": "green",
        "rating_questionable": "yellow",
        "rating_sensitive": "orange1",
        "rating_explicit": "red",
        "rating_unrated": "grey58",
        "rating_general": "#7cc4ff",
        "success": "bold #4a9eff",
        "error": "bold red",
        "accent": "bold #4a9eff",
    }
)

_BANNER = r"""
██████╗ ███████╗██╗     ██████╗  ██████╗  ██████╗ ██████╗ ██╗   ██╗
██╔════╝ ██╔════╝██║     ██╔══██╗██╔═══██╗██╔═══██╗██╔══██╗██║   ██║
██║  ███╗█████╗  ██║     ██████╔╝██║   ██║██║   ██║██████╔╝██║   ██║
██║   ██║██╔══╝  ██║     ██╔══██╗██║   ██║██║   ██║██╔══██╗██║   ██║
╚██████╔╝███████╗███████╗██████╔╝╚██████╔╝╚██████╔╝██║  ██║╚██████╔╝
 ╚═════╝ ╚══════╝╚══════╝╚═════╝  ╚═════╝  ╚═════╝ ╚═╝  ╚═╝ ╚═════╝
"""

_RATING_STYLES = {
    "safe": "rating_safe",
    "questionable": "rating_questionable",
    "sensitive": "rating_sensitive",
    "explicit": "rating_explicit",
    "unrated": "rating_unrated",
    "general": "rating_general",
}


class ConsoleFactory:
    """Builds consoles that respect the user's colour preference."""

    def __init__(
        self,
        *,
        color: bool = True,
        force_terminal: bool | None = None,
        width: int | None = None,
        stderr: bool = False,
        file: TextIO | None = None,
    ) -> None:
        self.color = color
        self.force_terminal = force_terminal
        self.width = width
        self.stderr = stderr
        self.file = file
        self._console: Console | None = None

    @property
    def console(self) -> Console:
        if self._console is None:
            self._console = Console(
                theme=GELBOORU_THEME,
                no_color=not self.color,
                force_terminal=self.force_terminal,
                width=self.width,
                stderr=self.stderr,
                file=self.file,
                soft_wrap=False,
                highlight=False,
            )
        return self._console

    def with_color(self, color: bool) -> ConsoleFactory:
        """A copy with a different colour setting (used by ``--no-color``)."""

        return ConsoleFactory(
            color=color,
            force_terminal=self.force_terminal,
            width=self.width,
            stderr=self.stderr,
            file=self.file,
        )

    def print(self, *args: Any, **kwargs: Any) -> None:
        self.console.print(*args, **kwargs)


# --------------------------------------------------------------------------- #
# Banner and messages
# --------------------------------------------------------------------------- #
def banner(version: str, subtitle: str = "") -> Panel:
    """The startup banner shown before most commands."""

    title = Text(_BANNER.strip("\n"), style="brand", no_wrap=True)
    footer = f"v{version} — {subtitle}" if subtitle else f"v{version}"
    body: RenderableType = Group(title, Text(footer, style="brand_dim"))
    return Panel(
        body,
        border_style="brand_dim",
        padding=(0, 2),
        title="[bold]gelbooru[/]",
        title_align="left",
    )


def ok(console: Console, message: str) -> None:
    console.print(f"[ok]✓[/] {message}")


def info(console: Console, message: str) -> None:
    console.print(f"[info]•[/] {message}")


def warn(console: Console, message: str) -> None:
    console.print(f"[warn]![/] {message}")


def error(console: Console, message: str, *, hint: str | None = None) -> None:
    console.print(f"[err]✗[/] {message}")
    if hint:
        console.print(f"  [muted]hint:[/] {hint}")


# --------------------------------------------------------------------------- #
# Tables
# --------------------------------------------------------------------------- #
def _rating_text(rating: str | None) -> Text:
    value = (rating or "unrated").lower()
    return Text(value, style=_RATING_STYLES.get(value, "muted"))


def format_post_table(
    posts: Sequence[Post],
    *,
    max_rows: int = 40,
    tag_count: int = 6,
    width: int | None = None,
) -> RenderableType:
    """Render search results as a Rich table.

    Columns are dropped as the terminal narrows so the table stays readable on
    small windows instead of squeezing every cell into an ellipsis.
    """

    if not posts:
        return Text("No posts matched the query.", style="muted")

    available = width if width is not None else 80
    # Gelbooru's dapi omits file_size for most posts, so drop the column rather
    # than filling the table with question marks.
    any_sizes = any(post.file_size for post in posts)
    show_size = any_sizes and available >= 100
    show_type = available >= 80
    show_score = available >= 90

    table = Table(
        title=None,
        header_style="table_header",
        border_style="grey30",
        expand=False,
        pad_edge=False,
        row_styles=["", "on grey11"],
    )
    table.add_column("ID", justify="right", style="id", no_wrap=True)
    table.add_column("Rating", no_wrap=True)
    if show_size:
        table.add_column("Size", justify="right", no_wrap=True)
    table.add_column("Dimensions", justify="right", no_wrap=True)
    if show_type:
        table.add_column("Type", no_wrap=True)
    if show_score:
        table.add_column("Score", justify="right", no_wrap=True)
    table.add_column(
        "Tags",
        overflow="ellipsis",
        no_wrap=True,
        max_width=max(20, min(52, available - 32)),
        ratio=1,
    )

    for post in posts[:max_rows]:
        dimensions = f"{post.width}×{post.height}" if post.width and post.height else "?"
        tags = post.summary(tag_count) if post.tags else Text("(untagged)", style="muted")
        row = [str(post.id), _rating_text(post.rating)]
        if show_size:
            row.append(human_bytes(post.file_size) if post.file_size else "?")
        row.append(dimensions)
        if show_type:
            row.append((post.extension or "?").lower())
        if show_score:
            row.append(str(post.score if post.score is not None else "?"))
        row.append(tags)
        table.add_row(*row)

    if len(posts) > max_rows:
        table.caption = (
            f"showing {max_rows} of {len(posts)} posts (raise display.table_max_rows to see more)"
        )
    return table


def format_history_table(entries: Sequence[Any], *, max_rows: int = 40) -> RenderableType:
    """Render download history rows."""

    if not entries:
        return Text("No downloads recorded yet.", style="muted")

    table = Table(
        header_style="table_header",
        border_style="grey30",
        expand=False,
        row_styles=["", "on grey11"],
    )
    table.add_column("Post ID", justify="right", style="id", no_wrap=True)
    table.add_column("Status", no_wrap=True)
    table.add_column("Size", justify="right", no_wrap=True)
    table.add_column("When", no_wrap=True)
    table.add_column("File", overflow="ellipsis", ratio=1)

    for entry in entries[:max_rows]:
        status = str(entry.status)
        style = {
            DownloadStatus.COMPLETED.value: "ok",
            DownloadStatus.SKIPPED.value: "warn",
            DownloadStatus.DUPLICATE.value: "warn",
            DownloadStatus.DRY_RUN.value: "info",
            DownloadStatus.UNAVAILABLE.value: "muted",
        }.get(status, "err")
        name = Path(entry.path).name if entry.path else (entry.error or "")
        table.add_row(
            str(entry.post_id),
            Text(status, style=style),
            human_bytes(entry.file_size),
            format_timestamp(entry.timestamp),
            name,
        )
    if len(entries) > max_rows:
        table.caption = f"showing {max_rows} of {len(entries)} rows"
    return table


def print_post_detail(console: Console, post: Post) -> None:
    """Print a detailed view of a single post."""

    grid = Table.grid(padding=(0, 2))
    grid.add_column(style="bold", justify="right")
    grid.add_column(overflow="fold")

    grid.add_row("Post ID", Text(str(post.id), style="id"))
    grid.add_row("Rating", _rating_text(post.rating))
    grid.add_row("Dimensions", f"{post.width or '?'} × {post.height or '?'}")
    grid.add_row("File size", human_bytes(post.file_size) if post.file_size else "unknown")
    grid.add_row("Type", (post.extension or "?").lower())
    grid.add_row(
        "Score", f"{post.score if post.score is not None else '?'} (up {post.up_score or 0})"
    )
    grid.add_row("Comments", str(post.comment_count if post.comment_count is not None else "?"))
    grid.add_row("Status", post.status or "active")
    grid.add_row("Creator", f"user #{post.creator_id}" if post.creator_id else "unknown")
    if post.md5:
        grid.add_row("MD5", post.md5)
    if post.created_at_dt:
        grid.add_row("Uploaded", post.created_at_dt.astimezone().strftime("%Y-%m-%d %H:%M %Z"))
    grid.add_row("Source", post.source or "unknown")
    grid.add_row("File URL", post.file_url or Text("not provided by the API", style="warn"))
    if post.preview_url:
        grid.add_row("Preview", post.preview_url)
    if not post.is_available:
        grid.add_row("Warning", Text(post.unavailable_reason or "not downloadable", style="warn"))
    grid.add_row("Tags", post.tags or "(none)")

    console.print(
        Panel(
            grid,
            title=f"[bold]post {post.id}[/]",
            title_align="left",
            border_style="brand_dim",
            padding=(1, 2),
        )
    )


def print_search_summary(
    console: Console,
    *,
    total_fetched: int,
    shown: int,
    matched: int,
    filtered_out: int,
    total_available: int | None = None,
    elapsed: float | None = None,
) -> None:
    """One-line recap after a search."""

    parts = [
        f"[id]{total_fetched}[/] fetched",
        f"[ok]{matched}[/] matched",
    ]
    if filtered_out:
        parts.append(f"[warn]{filtered_out}[/] filtered out")
    if shown < matched:
        parts.append(f"[muted]{matched - shown} not displayed[/]")
    if total_available is not None and total_available >= 0:
        parts.append(f"[muted]{total_available} posts match the query[/]")
    if elapsed is not None:
        parts.append(f"[muted]in {human_duration(elapsed)}[/]")
    console.print("  ".join(parts))


# --------------------------------------------------------------------------- #
# Progress
# --------------------------------------------------------------------------- #
class RichProgressReporter:
    """Drives a live Rich progress bar from :class:`DownloadEvent` messages."""

    def __init__(
        self,
        console: Console,
        *,
        enabled: bool = True,
        transient: bool = True,
        title: str = "Downloading",
    ) -> None:
        self.console = console
        self.enabled = enabled
        self.transient = transient
        self.default_title = title
        self._progress: Progress | None = None
        self._task_id: Any = None
        self._total = 0
        self._completed = 0
        self._bytes = 0

    def __enter__(self) -> RichProgressReporter:
        if self.enabled:
            self._progress = Progress(
                SpinnerColumn(style="brand"),
                TextColumn("[bold]{task.description}", style="brand_dim"),
                BarColumn(bar_width=28, complete_style="brand", finished_style="ok"),
                MofNCompleteColumn(),
                TextColumn("•"),
                TextColumn("{task.fields[status]}"),
                TransferSpeedColumn(),
                TimeElapsedColumn(),
                console=self.console,
                transient=self.transient,
                auto_refresh=True,
            )
            self._progress.__enter__()
        return self

    def __exit__(self, *exc_info: object) -> None:
        self.close()

    def begin(self, total: int, *, title: str) -> None:
        self._total = total
        self._completed = 0
        self._bytes = 0
        if self._progress is not None:
            self._task_id = self._progress.add_task(
                title,
                total=total,
                status="[muted]starting[/]",
                completed=0,
            )

    def event(self, event: DownloadEvent) -> None:
        self._completed += 1
        if event.status is DownloadStatus.COMPLETED:
            self._bytes += event.size
            status = f"[ok]downloaded[/] [muted]{human_bytes(event.size)}[/]"
        elif event.status is DownloadStatus.SKIPPED:
            status = f"[warn]skipped[/] [muted]{event.post_id}[/]"
        elif event.status is DownloadStatus.DUPLICATE:
            status = f"[warn]duplicate[/] [muted]{event.post_id}[/]"
        elif event.status is DownloadStatus.DRY_RUN:
            status = f"[info]dry-run[/] [muted]{event.post_id}[/]"
        elif event.status is DownloadStatus.UNAVAILABLE:
            status = f"[muted]unavailable {event.post_id}[/]"
        else:
            status = f"[err]failed[/] [muted]{event.post_id}[/]"
        if self._progress is not None and self._task_id is not None:
            self._progress.update(
                self._task_id,
                completed=self._completed,
                status=status,
                description=f"{self.default_title} · {human_bytes(self._bytes)}",
            )

    def close(self) -> None:
        if self._progress is not None:
            self._progress.__exit__(None, None, None)
            self._progress = None

    def finish(self) -> None:
        # Keep the bar on screen until the caller is done with it; nothing to do.
        return None


def render_summary_panel(
    summary: DownloadSummary, *, output_dir: Path | None = None, title: str = "Summary"
) -> Panel:
    """Build the end-of-batch report panel."""

    body = Table.grid(padding=(0, 2))
    body.add_column(justify="right", style="bold")
    body.add_column()

    def line(label: str, value: str, style: str = "") -> None:
        body.add_row(label, Text(value, style=style) if style else value)

    line("Downloaded", f"[ok]{summary.completed}[/]", style="ok")
    line("Skipped", str(summary.skipped), style="warn" if summary.skipped else "muted")
    if summary.duplicates:
        line("Duplicates", str(summary.duplicates), style="warn")
    line("Failed", str(summary.failed), style="err" if summary.failed else "muted")
    line("Unavailable", str(summary.unavailable), style="muted")
    if summary.dry_run:
        line("Dry run", f"[info]{summary.dry_run}[/] (nothing was written)", style="info")
    line("Total bytes", human_bytes(summary.bytes_downloaded))
    line("Elapsed", human_duration(summary.elapsed))
    if output_dir is not None:
        line("Output", str(output_dir))

    return Panel(
        body,
        title=f"[bold]{title}[/]",
        title_align="left",
        border_style="brand_dim",
        padding=(1, 2),
    )


def print_summary(
    console: Console,
    summary: DownloadSummary,
    *,
    output_dir: Path | None = None,
    max_failures: int = 10,
    title: str = "Summary",
) -> None:
    """Print the final report plus (capped) failure details."""

    console.print(render_summary_panel(summary, output_dir=output_dir, title=title))

    failures = summary.failures
    if not failures:
        return
    table = Table(header_style="error_header", border_style="grey30", expand=False)
    table.add_column("Post", justify="right", style="id", no_wrap=True)
    table.add_column("Status", no_wrap=True)
    table.add_column("Reason", overflow="fold", ratio=1)
    for result in failures[:max_failures]:
        table.add_row(
            str(result.post_id), str(result.status.value), result.error or "unknown error"
        )
    if len(failures) > max_failures:
        table.caption = f"… and {len(failures) - max_failures} more (see `gelbooru config history --status failed`)"
    console.print(table)


def render_results_preview(
    results: Sequence[DownloadResult], *, max_rows: int = 20
) -> RenderableType:
    """Compact table used by ``--dry-run``."""

    table = Table(header_style="table_header", border_style="grey30", expand=False)
    table.add_column("Post", justify="right", style="id", no_wrap=True)
    table.add_column("Action", no_wrap=True)
    table.add_column("Size", justify="right", no_wrap=True)
    table.add_column("Destination", overflow="ellipsis", ratio=1)
    for result in results[:max_rows]:
        table.add_row(
            str(result.post_id),
            str(result.status.value),
            human_bytes(result.size) if result.size else "?",
            result.path or "",
        )
    if len(results) > max_rows:
        table.caption = f"… and {len(results) - max_rows} more"
    return table


def tags_preview(tags: str, limit: int = 10) -> str:
    """Short tag string for banners and prompts."""

    return truncate_tags(tags, limit)
