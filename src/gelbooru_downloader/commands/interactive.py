"""``gelbooru interactive`` — a small REPL around the same functionality.

Interactive mode is strictly optional: every command it offers has a
non-interactive equivalent.
"""

from __future__ import annotations

import shlex
from pathlib import Path
from typing import Any

import typer
from rich.prompt import Prompt

from ..app import AppContext, get_context, run_async
from ..console import (
    _rating_text,
    format_post_table,
    ok,
    print_post_detail,
    print_search_summary,
    print_summary,
    warn,
)
from ..exceptions import GelbooruError
from ._common import (
    DownloadRequest,
    download_options,
    download_posts,
    export_posts,
    run_search,
)

__all__ = ["interactive_command", "run_interactive"]

_HELP = """[bold]commands[/]
  [info]search <tags> [-n 25][/]                search and show a result table
  [info]download <tags> [-n 25] [-o DIR][/]    search and download the results
  [info]browse <tags> [-n 25] [--images][/]    paged list with thumbnails and titles
  [info]info <post_id>[/]                      show one post in detail
  [info]dl <post_id...[/]                      download specific posts
  [info]export <tags> <json|csv> <file>[/]     save search results to a file
  [info]presets[/]                             list saved presets
  [info]history[/]                             recent downloads
  [info]help[/], [info]quit[/]                 this text, or leave

Example: [muted]download blue_hair solo -n 20 -o ./downloads/blue[/]
"""


def run_interactive(app: AppContext, *, output: Path | None = None, limit: int = 25) -> int:
    """Run the REPL loop; returns the process exit code."""

    from .. import __version__
    from ..console import banner

    if app.config.display.show_banner:
        app.console.print(banner(__version__, "interactive mode — type 'help' for commands"))
    app.console.print("[muted]Ctrl+C or 'quit' to leave.[/]")

    while True:
        try:
            raw = Prompt.ask("[brand]gelbooru[/]", default="help", show_default=False)
        except (EOFError, KeyboardInterrupt):
            app.console.print("\n[muted]bye[/]")
            return 0
        except Exception:  # pragma: no cover - click raises Abort on Ctrl+C in some setups
            app.console.print("\n[muted]bye[/]")
            return 0

        raw = raw.strip()
        if not raw:
            continue
        if raw.lower() in {"quit", "exit", "q"}:
            app.console.print("[muted]bye[/]")
            return 0
        try:
            _dispatch(app, raw, output=output, limit=limit)
        except GelbooruError as exc:
            warn(app.console, str(exc))
        except KeyboardInterrupt:
            app.console.print("[warn]operation cancelled[/]")
        except Exception as exc:
            app.console.print(f"[err]unexpected error:[/] {exc}")
        app.console.print("")

    return 0


def _dispatch(app: AppContext, line: str, *, output: Path | None, limit: int) -> None:
    """Parse and execute one REPL line."""

    try:
        parts = shlex.split(line)
    except ValueError as exc:
        warn(app.console, f"could not parse the command: {exc}")
        return
    if not parts:
        return

    command, args = parts[0].lower(), parts[1:]
    if command in {"help", "?"}:
        app.console.print(_HELP)
    elif command in {"search", "find", "s"}:
        _cmd_search(app, args, limit=limit)
    elif command in {"browse", "br", "b"}:
        _cmd_browse(app, args, limit=limit)
    elif command in {"download", "dl", "d"}:
        _cmd_download(app, args, output=output, limit=limit)
    elif command in {"info", "show", "post"}:
        _cmd_info(app, args)
    elif command in {"get", "fetch"}:
        _cmd_fetch(app, args, output=output)
    elif command == "export":
        _cmd_export(app, args, limit=limit)
    elif command in {"presets", "preset"}:
        _cmd_presets(app)
    elif command in {"history", "hist"}:
        from ..console import format_history_table

        app.console.print(format_history_table(app.history.history(limit=15)))
    else:
        warn(app.console, f"unknown command {command!r} — type 'help'")


def _parse_flags(
    args: list[str], known: dict[str, str]
) -> tuple[dict[str, Any], list[str], list[str]]:
    """Tiny flag parser.

    Returns ``(values, positional, unknown_flags)`` and understands
    ``-n 10``, ``--output dir`` and boolean flags listed in ``known`` as ``"flag"``.
    """

    values: dict[str, Any] = {}
    positional: list[str] = []
    unknown: list[str] = []
    index = 0
    while index < len(args):
        option_name = args[index]
        if not option_name.startswith("-") or option_name == "-":
            positional.append(option_name)
            index += 1
            continue
        option = known.get(option_name)
        if option is None:
            unknown.append(option_name)
            index += 1
            continue
        if option == "flag":
            values.setdefault("_flags", []).append(option_name.lstrip("-"))
            index += 1
            continue
        if index + 1 >= len(args):
            values[option] = ""
            break
        values[option] = args[index + 1]
        index += 2
    return values, positional, unknown


def _flag_help(app: AppContext, unknown: list[str]) -> None:
    if unknown:
        warn(app.console, f"ignored unknown option(s): {', '.join(unknown)}")


def _cmd_search(app: AppContext, args: list[str], *, limit: int) -> None:
    known = {
        "-n": "limit",
        "--limit": "limit",
        "-p": "page",
        "--page": "page",
        "-O": "order",
        "--order": "order",
    }
    parsed, positional, unknown = _parse_flags(args, known)
    _flag_help(app, unknown)
    tags = " ".join(positional)
    if not tags:
        warn(app.console, "usage: search <tags> [-n 25]")
        return
    try:
        requested = int(parsed.get("limit") or limit)
        page = int(parsed.get("page") or 1)
    except ValueError:
        warn(app.console, "-n/--limit and -p/--page must be numbers")
        return

    with app.console.status(f"[brand]searching {tags}…", spinner="dots"):
        outcome = run_search(
            app,
            tags=tags,
            limit=requested,
            order=parsed.get("order"),
            page=page,
            page_size=app.config.general.page_size,
        )
    if not outcome.posts:
        warn(app.console, f"nothing matched '{tags}'")
        return

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
        elapsed=outcome.elapsed,
    )


def _supports_inline_images() -> bool:
    import os

    return bool(os.environ.get("KITTY_WINDOW_ID")) or "kitty" in os.environ.get("TERM", "")


def _render_preview(url: str | None) -> None:
    if not url:
        return
    if not _supports_inline_images():
        return
    try:
        import subprocess
        import tempfile

        import httpx

        response = httpx.get(url, timeout=15.0, follow_redirects=True)
        response.raise_for_status()
        suffix = ".png"
        lowered = url.lower().split("?", 1)[0]
        for ext in (".jpg", ".jpeg", ".webp", ".gif"):
            if lowered.endswith(ext):
                suffix = ext
                break
        with tempfile.NamedTemporaryFile(suffix=suffix, delete=False) as handle:
            handle.write(response.content)
            path = handle.name
        try:
            subprocess.run(  # noqa: S603
                ["kitty", "+kitten", "icat", "--align", "left", path],  # noqa: S607
                check=False,
                stdout=None,
                stderr=subprocess.DEVNULL,
            )
        finally:
            import contextlib
            from pathlib import Path

            with contextlib.suppress(OSError):
                Path(path).unlink(missing_ok=True)
    except Exception:
        return


def _cmd_browse(app: AppContext, args: list[str], *, limit: int) -> None:

    known = {
        "-n": "limit",
        "--limit": "limit",
        "--images": "flag",
    }
    parsed, positional, unknown = _parse_flags(args, known)
    _flag_help(app, unknown)
    tags = " ".join(positional)
    if not tags:
        warn(app.console, "usage: browse <tags> [-n 25] [--images]")
        return
    try:
        requested = int(parsed.get("limit") or limit)
    except ValueError:
        warn(app.console, "-n/--limit must be a number")
        return
    want_images = "images" in parsed.get("_flags", []) or _supports_inline_images()

    with app.console.status(f"[brand]searching {tags}…", spinner="dots"):
        outcome = run_search(
            app,
            tags=tags,
            limit=requested,
            page_size=app.config.general.page_size,
        )
    if not outcome.posts:
        warn(app.console, f"nothing matched '{tags}'")
        return

    posts = outcome.posts
    page_size = 4
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
                _render_preview(post.preview_url or post.file_url)
            elif post.preview_url:
                app.console.print(f"   [muted]{post.preview_url}[/]")
        app.console.print("")
        choices = []
        if page > 0:
            choices.append("p")
        if page < total_pages - 1:
            choices.append("n")
        choices.append("q")
        prompt = "/".join(choices)
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


def _cmd_download(app: AppContext, args: list[str], *, output: Path | None, limit: int) -> None:
    known = {
        "-n": "limit",
        "--limit": "limit",
        "-o": "output",
        "--output": "output",
        "-c": "concurrency",
        "--concurrency": "concurrency",
        "--dry-run": "flag",
        "--overwrite": "flag",
        "--no-duplicates": "flag",
        "--archive": "flag",
    }
    parsed, positional, unknown = _parse_flags(args, known)
    _flag_help(app, unknown)
    tags = " ".join(positional)
    if not tags:
        warn(app.console, "usage: download <tags> [-n 25] [-o DIR]")
        return
    flags = parsed.get("_flags", [])
    try:
        requested = int(parsed.get("limit") or limit)
        concurrency = int(parsed["concurrency"]) if parsed.get("concurrency") else None
    except ValueError:
        warn(app.console, "-n/--limit and -c/--concurrency must be numbers")
        return

    request = DownloadRequest(
        limit=requested,
        output=Path(parsed["output"]).expanduser() if parsed.get("output") else output,
        concurrency=concurrency,
        overwrite="overwrite" in flags,
        dry_run="dry-run" in flags,
        duplicate_check="no-duplicates" in flags,
        archive="archive" in flags or None,
    )
    with app.console.status(f"[brand]searching {tags}…", spinner="dots"):
        outcome = run_search(
            app, tags=tags, limit=requested, page_size=app.config.general.page_size
        )
    if not outcome.posts:
        warn(app.console, f"nothing matched '{tags}'")
        return
    summary = run_async(download_posts(app, outcome.posts, request, title="Downloading"))
    print_summary(app.console, summary, output_dir=download_options(app, request).output_dir)


def _cmd_info(app: AppContext, args: list[str]) -> None:
    if not args:
        warn(app.console, "usage: info <post_id>")
        return
    try:
        post_id = int(args[0])
    except ValueError:
        warn(app.console, f"{args[0]!r} is not a post id")
        return
    from .post import fetch_posts

    posts, missing = run_async(fetch_posts(app, [post_id]))
    if missing:
        warn(app.console, f"post {post_id} was not found")
        return
    print_post_detail(app.console, posts[0])


def _cmd_fetch(app: AppContext, args: list[str], *, output: Path | None) -> None:
    if not args:
        warn(app.console, "usage: dl <post_id> [post_id ...]")
        return
    ids: list[int] = []
    for token in args:
        try:
            ids.append(int(token))
        except ValueError:
            warn(app.console, f"{token!r} is not a post id")
            return

    from .post import fetch_posts

    posts, missing = run_async(fetch_posts(app, ids))
    for post_id in missing:
        warn(app.console, f"post {post_id}: not found")
    if not posts:
        return
    request = DownloadRequest(output=output)
    summary = run_async(download_posts(app, posts, request, title="Downloading"))
    print_summary(app.console, summary, output_dir=download_options(app, request).output_dir)


def _cmd_export(app: AppContext, args: list[str], *, limit: int) -> None:
    parsed, positional, unknown = _parse_flags(args, {"-n": "limit", "--limit": "limit"})
    _flag_help(app, unknown)
    if len(positional) < 3:
        warn(app.console, "usage: export <tags> <json|csv> <file>")
        return
    fmt, destination = positional[-2].lower(), Path(positional[-1]).expanduser()
    tags = " ".join(positional[:-2])
    try:
        requested = int(parsed.get("limit") or limit)
    except ValueError:
        warn(app.console, "-n/--limit must be a number")
        return

    outcome = run_search(app, tags=tags, limit=requested, page_size=app.config.general.page_size)
    if not outcome.posts:
        warn(app.console, "nothing to export")
        return
    try:
        path = export_posts(outcome.posts, destination, fmt)
    except ValueError as exc:
        warn(app.console, str(exc))
        return
    ok(app.console, f"exported {len(outcome.posts)} post(s) to {path}")


def _cmd_presets(app: AppContext) -> None:
    from rich.table import Table

    from ..utils import format_timestamp

    presets = list(app.presets)
    if not presets:
        app.console.print(
            '[muted]no presets saved yet — add one with[/] [info]gelbooru presets add NAME "tags"[/]'
        )
        return
    table = Table(header_style="table_header", border_style="grey30", expand=False)
    table.add_column("Name", style="id", no_wrap=True)
    table.add_column("Tags", overflow="fold", ratio=1)
    table.add_column("Updated", no_wrap=True)
    for preset in presets:
        table.add_row(preset.name, preset.tags, format_timestamp(preset.updated_at))
    app.console.print(table)


def interactive_command(
    ctx: typer.Context,
    output: Path | None = None,
    limit: int = 25,
) -> None:
    """Start an interactive session (search, inspect and download without typing full commands)."""

    app = get_context(ctx)
    app.banner("interactive")
    raise typer.Exit(run_interactive(app, output=output, limit=limit))
