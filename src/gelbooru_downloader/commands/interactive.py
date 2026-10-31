"""``gelbooru interactive`` — a small REPL around the same functionality.

Interactive mode is strictly optional: every command it offers has a
non-interactive equivalent.
"""

from __future__ import annotations

import base64
import shlex
import sys
import time
from pathlib import Path
from typing import Any

import typer
from prompt_toolkit import PromptSession
from prompt_toolkit.auto_suggest import AutoSuggestFromHistory
from prompt_toolkit.completion import Completer, Completion
from prompt_toolkit.document import Document
from prompt_toolkit.history import InMemoryHistory
from prompt_toolkit.key_binding import KeyBindings
from prompt_toolkit.styles import Style
from rich.prompt import Prompt

from ..app import AppContext, get_context, run_async
from ..console import (
    GELBOORU_THEME,
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
  [info]<tags>[/]                              browse posts matching these tags (default)
  [info]search <tags> [-n 25][/]               search and show a result table
  [info]download <tags> [-n 25] [-o DIR][/]    search and download the results
  [info]browse <tags> [-n 25][/]               paged list with thumbnails and titles
  [info]info <post_id>[/]                      show one post in detail
  [info]dl <post_id...[/]                      download specific posts
  [info]export <tags> <json|csv> <file>[/]     save search results to a file
  [info]presets[/]                             list saved presets
  [info]history[/]                             recent downloads
  [info]help[/], [info]quit[/]                 this text, or leave

Example: [muted]blue_hair solo[/]              (browse by default)
Example: [muted]download blue_hair solo -n 20 -o ./downloads/blue[/]
"""


class TagCompleter(Completer):
    """Fetch tag suggestions from the Gelbooru API as the user types."""

    def __init__(self, app: AppContext) -> None:
        self.app = app
        self._cache: dict[str, list[tuple[str, str]]] = {}
        self._cache_ttl = 300.0  # 5 minutes
        self._cache_times: dict[str, float] = {}

    def get_completions(self, document: Document, complete_event: Any) -> Any:  # noqa: ARG002
        text = document.text_before_cursor
        # If text ends with a space, user is starting a new word
        if text and text[-1].isspace():
            # Don't autocomplete yet - user hasn't typed anything for the new word
            return
        parts = text.split()
        if not parts:
            return
        prefix = parts[-1].lower().lstrip("-!")
        if len(prefix) < 1:
            return

        now = time.time()
        cached = self._cache.get(prefix)
        if cached is not None and now - self._cache_times.get(prefix, 0) < self._cache_ttl:
            tags = cached
        else:
            tags = self._fetch_tags(prefix)
            self._cache[prefix] = tags
            self._cache_times[prefix] = now

        start_pos = -(len(parts[-1]))

        for item in tags:
            if isinstance(item, tuple) and len(item) >= 2:
                name = item[0]
                label = item[-1]
            else:
                name = str(item)
                label = name
            yield Completion(name, start_position=start_pos, display=label)

    def _fetch_tags(self, prefix: str) -> list[tuple[str, str]]:
        try:
            import httpx

            network = self.app.config.network
            from urllib.parse import urlencode, urlsplit, urlunsplit

            url_parts = urlsplit(network.base_url)
            # Use the website's autocomplete endpoint (returns tags sorted by count)
            params = {"page": "autocomplete2", "term": prefix, "limit": "10"}
            path = url_parts.path.rstrip("/") + "/index.php"
            url = urlunsplit((url_parts.scheme, url_parts.netloc, path, urlencode(params), ""))

            headers = {"User-Agent": network.user_agent}
            with httpx.Client(timeout=5.0) as client:
                response = client.get(url, headers=headers)
                if response.status_code != 200:
                    return []
                data = response.json()
                if not isinstance(data, list):
                    return []

            tags = []
            for item in data:
                if not isinstance(item, dict):
                    continue
                name = item.get("value", "") or item.get("label", "")
                count_str = item.get("post_count", "0")
                try:
                    count = int(count_str)
                except (ValueError, TypeError):
                    count = 0
                if name:
                    label = f"{name}  ({count})" if count else name
                    tags.append((name, count, label))

            # Already sorted by count from the API, but sort locally to be safe
            tags.sort(key=lambda t: t[1], reverse=True)
            return [(t[0], t[2]) for t in tags[:10]]
        except Exception:
            return []


def _rendered_rows(console: Any, renderable: Any) -> int:
    """Return how many terminal rows ``renderable`` occupies at this width."""

    from io import StringIO

    from rich.console import Console

    buffer = StringIO()
    probe = Console(file=buffer, width=console.width, theme=GELBOORU_THEME)
    probe.print(renderable)
    return max(1, len(buffer.getvalue().splitlines()))


def _write_control(console: Any, sequence: str) -> None:
    """Write a raw terminal control sequence, ignoring non-terminal streams."""

    stream = getattr(console, "file", None)
    if stream is None:
        return
    try:
        stream.write(sequence)
        stream.flush()
    except (AttributeError, OSError, ValueError):
        pass


def _pin_banner(app: AppContext, renderable: Any) -> int:
    """Print the banner and pin it to the top of the terminal.

    A DECSTBM scroll region is set below the banner, so command output scrolls
    underneath it instead of pushing it off-screen.  Returns the number of
    reserved rows (``0`` when the banner could not be pinned, e.g. when output
    is redirected or the terminal is too short).
    """

    console = app.console
    if console.is_terminal:
        console.clear()
    console.print(renderable)

    if not console.is_terminal or sys.platform == "win32":
        return 0

    rows = console.size.height
    height = _rendered_rows(console, renderable)
    if rows < height + 4:
        return 0

    # DECSTBM homes the cursor, so move it back below the banner afterwards.
    _write_control(console, f"\x1b[{height + 1};{rows}r\x1b[{height + 1};1H")
    return height


def _unpin_banner(app: AppContext, rows: int) -> None:
    """Release the scroll region reserved by :func:`_pin_banner`."""

    if rows:
        # Resetting DECSTBM also homes the cursor; save and restore it so the
        # shell prompt continues where the REPL left off.
        _write_control(app.console, "\x1b7\x1b[r\x1b8")


def run_interactive(app: AppContext, *, output: Path | None = None, limit: int = 25) -> int:
    """Run the REPL loop; returns the process exit code."""

    from .. import __version__
    from ..console import banner as make_banner

    pinned_rows = 0
    if app.config.display.show_banner:
        pinned_rows = _pin_banner(
            app,
            make_banner(
                __version__,
                "interactive mode — type 'help' for commands",
                interactive=True,
            ),
        )
    app.console.print("[muted]Ctrl+C or 'quit' to leave. Tab to autocomplete, ↑↓ for history.[/]")
    app.console.print(
        "[muted]Type any tags to browse, or a command (search, download, info, ...).[/]"
    )

    # Build completer with tag suggestions from the Gelbooru API
    commands = [
        "search",
        "find",
        "s",
        "download",
        "dl",
        "d",
        "browse",
        "br",
        "b",
        "info",
        "show",
        "post",
        "get",
        "fetch",
        "export",
        "presets",
        "preset",
        "history",
        "hist",
        "help",
        "?",
        "quit",
        "exit",
        "q",
    ]
    preset_names = [p.name for p in app.presets]
    tag_completer = TagCompleter(app)
    from prompt_toolkit.completion import WordCompleter

    command_completer = WordCompleter(commands + preset_names, ignore_case=True)

    # Use a single combined completer that shows tag suggestions for
    # tag-like input and command suggestions for command-like input
    from prompt_toolkit.completion import Completer

    class CombinedCompleter(Completer):
        def get_completions(self, document: Document, complete_event: Any) -> Any:
            text = document.text_before_cursor.lstrip()
            if not text:
                return
            first_word = text.split()[0].lower() if text.split() else ""
            if len(text.split()) == 1 and len(first_word) >= 1:
                # Single word: show tag suggestions
                tag_results = list(tag_completer.get_completions(document, complete_event))
                if tag_results:
                    yield from tag_results
                # Show command suggestions only for 2+ char inputs matching a command
                if len(first_word) >= 2:
                    matching_cmds = [
                        c for c in commands + preset_names if c.lower().startswith(first_word)
                    ]
                    if matching_cmds:
                        yield from command_completer.get_completions(document, complete_event)
            else:
                # Multi-word input: show tag suggestions for the last word
                tag_results = list(tag_completer.get_completions(document, complete_event))
                if tag_results:
                    yield from tag_results

    completer = CombinedCompleter()

    # Custom style for the prompt
    style = Style.from_dict(
        {
            "prompt": "ansicyan bold",
            "bottom-toolbar": "bg:#333333 #ffffff",
        }
    )

    # Key bindings
    kb = KeyBindings()

    @kb.add("c-c")
    def _(event: Any) -> None:
        event.app.exit(exception=KeyboardInterrupt)

    @kb.add("c-d")
    def _(event: Any) -> None:
        event.app.exit()

    history = InMemoryHistory()

    session = PromptSession(
        history=history,
        auto_suggest=AutoSuggestFromHistory(),
        completer=completer,
        style=style,
        key_bindings=kb,
        bottom_toolbar=lambda: (
            "Type tags to browse | Commands: search download browse info export presets history | Tab=complete ↑↓=history"
        ),
    )

    try:
        while True:
            try:
                raw = session.prompt("gelbooru> ")
            except (EOFError, KeyboardInterrupt):
                app.console.print("\n[muted]bye[/]")
                return 0
            except Exception:  # pragma: no cover
                app.console.print("\n[muted]bye[/]")
                return 0

            raw = raw.strip()
            if not raw:
                continue
            if raw.lower() in {"quit", "exit", "q"}:
                app.console.print("[muted]bye[/]")
                return 0
            try:
                _dispatch(app, raw, output=output, limit=limit, banner_rows=pinned_rows)
            except GelbooruError as exc:
                warn(app.console, str(exc))
            except KeyboardInterrupt:
                app.console.print("[warn]operation cancelled[/]")
            except Exception as exc:
                app.console.print(f"[err]unexpected error:[/] {exc}")
            app.console.print("")
    finally:
        _unpin_banner(app, pinned_rows)

    return 0


def _dispatch(
    app: AppContext,
    line: str,
    *,
    output: Path | None,
    limit: int,
    banner_rows: int = 0,
) -> None:
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
        _cmd_browse(app, args, limit=limit, banner_rows=banner_rows)
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
    elif command in {"quit", "exit", "q"}:
        raise EOFError
    else:
        # Treat unknown input as a browse query (default action)
        _cmd_browse(app, parts, limit=limit, banner_rows=banner_rows)


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
        "-r": "rating",
        "--rating": "rating",
        "-x": "exclude",
        "--exclude": "exclude",
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

    ratings = parsed.get("rating")
    exclude = parsed.get("exclude")

    with app.console.status(f"[brand]searching {tags}…", spinner="dots"):
        outcome = run_search(
            app,
            tags=tags,
            limit=requested,
            ratings=ratings.split(",") if ratings else None,
            exclude=exclude.split(",") if exclude else None,
            order=parsed.get("order"),
            page=page,
            page_size=app.config.general.page_size,
        )
    if not outcome.posts:
        warn(app.console, f"nothing matched '{tags}'")
        tag_list = tags.split()
        if len(tag_list) > 1:
            app.console.print(
                f"  [muted]hint: try searching tags individually: {', '.join(tag_list)}[/]"
            )
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


def _text_thumbnail(console: Any, url: str | None, *, width: int = 30) -> None:
    """Render a tiny ASCII/Unicode preview via chafa or viu if available.

    Falls back gracefully (no output) when neither tool is installed or the
    terminal cannot render images. This keeps ``browse`` usable everywhere
    instead of silently showing nothing on non-Kitty terminals.
    """

    if not url:
        return
    try:
        from urllib.parse import urlsplit

        import httpx

        host = (urlsplit(url).hostname or "").lower()
        headers: dict[str, str] = {}
        if host == "gelbooru.com" or host.endswith(".gelbooru.com"):
            headers["Referer"] = "https://gelbooru.com/"
        response = httpx.get(url, headers=headers, timeout=15.0, follow_redirects=True)
        response.raise_for_status()
        content_type = (response.headers.get("content-type") or "").split(";", 1)[0].strip().lower()
        if not content_type.startswith("image/"):
            return

        for tool in ("chafa", "viu"):
            import subprocess

            result = subprocess.run(  # noqa: S603
                [tool, "--size", f"{width}", "-"],
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
            )
            if result.returncode == 0:
                console.print(f"[muted]{result.stdout.decode('utf-8', 'replace')}[/]", end="")
                return
        return
    except Exception:
        return


def _render_preview(
    console: Any,
    url: str | None,
    *,
    force_text: bool = False,
    place: tuple[int, int, int] | None = None,
) -> None:
    """Show a preview image using the kitty graphics protocol.

    When ``place`` is given as ``(columns, rows, top_row)`` the image is fitted
    into that exact screen rectangle.  This keeps previews aligned inside the
    interactive scroll region, where a tall image would otherwise scroll the
    page and drift away from its post line.
    """

    if not url:
        return
    if force_text or not _supports_inline_images():
        _text_thumbnail(console, url)
        return

    try:
        import io
        from urllib.parse import urlsplit

        import httpx

        host = (urlsplit(url).hostname or "").lower()
        headers: dict[str, str] = {}
        if host == "gelbooru.com" or host.endswith(".gelbooru.com"):
            headers["Referer"] = "https://gelbooru.com/"
        response = httpx.get(url, headers=headers, timeout=15.0, follow_redirects=True)
        response.raise_for_status()
        content_type = (response.headers.get("content-type") or "").split(";", 1)[0].strip().lower()
        if not content_type.startswith("image/"):
            console.print(f"   [muted](not an image: {content_type})[/]")
            return

        import tempfile

        # Save the image to a temp file
        with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as handle:
            handle.write(response.content)
            path = handle.name

        try:
            if place is not None:
                import shutil
                import subprocess

                columns, rows, top = place
                executable = shutil.which("kitty")
                if executable is None:
                    console.print("   [muted](preview unavailable: kitty not found)[/]")
                    return
                result = subprocess.run(  # noqa: S603
                    [
                        executable,
                        "+kitten",
                        "icat",
                        "--align",
                        "left",
                        "--place",
                        f"{columns}x{rows}@0x{top}",
                        path,
                    ],
                    check=False,
                )
                if result.returncode != 0:
                    console.print("   [muted](preview unavailable)[/]")
                return

            import os

            # Use kitty +kitten icat to display the image.
            # This runs in the user's terminal and handles all the
            # protocol details.
            ret = os.system(f"kitty +kitten icat --align left {path}")  # noqa: S605
            if ret != 0:
                console.print(f"   [muted](icat exited with code {ret})[/]")
                # Fall back to kitty graphics protocol
                try:
                    from PIL import Image

                    img = Image.open(io.BytesIO(response.content))
                    max_width = 80
                    if img.width > max_width:
                        ratio = max_width / img.width
                        new_height = int(img.height * ratio)
                        img = img.resize((max_width, new_height))

                    png_buffer = io.BytesIO()
                    img.save(png_buffer, format="PNG")
                    png_data = png_buffer.getvalue()
                    b64_data = base64.b64encode(png_data).decode("ascii")

                    chunk_size = 4096
                    chunks = [
                        b64_data[i : i + chunk_size] for i in range(0, len(b64_data), chunk_size)
                    ]

                    sys.stdout.flush()
                    if len(chunks) == 1:
                        sys.stdout.write(f"\x1b_Ga=T,f=100,t=d;{chunks[0]}\x1b\\")
                    else:
                        sys.stdout.write(f"\x1b_Ga=T,f=100,t=d,m=1;{chunks[0]}\x1b\\")
                        for i, chunk in enumerate(chunks[1:], 1):
                            m = "1" if i < len(chunks) - 1 else "0"
                            sys.stdout.write(f"\x1b_Gm={m};{chunk}\x1b\\")
                    sys.stdout.flush()
                    sys.stdout.write("\n")
                    sys.stdout.flush()
                except Exception:
                    console.print("   [muted](image could not be rendered)[/]")
            else:
                console.print()
        finally:
            import contextlib
            from pathlib import Path

            with contextlib.suppress(OSError):
                Path(path).unlink(missing_ok=True)
    except Exception as exc:
        console.print(f"   [muted](image render failed: {exc})[/]")


def _clear_content_area(app: AppContext, banner_rows: int) -> None:
    """Erase everything below the pinned banner and drop old kitty images."""

    if _supports_inline_images():
        _write_control(app.console, "\x1b_Ga=d,d=A\x1b\\")
    _write_control(app.console, f"\x1b[{banner_rows + 1};1H\x1b[J")


def _browse_image_box(app: AppContext, banner_rows: int, count: int) -> tuple[int, int]:
    """Return image ``(rows, columns)`` that let a page fit below the banner."""

    content_rows = max(1, app.console.size.height - banner_rows)
    usable = max(1, content_rows - 2)  # page header + prompt
    block_rows = max(2, usable // max(1, count))
    return max(1, block_rows - 1), max(10, min(app.console.size.width, 40))


def _cmd_browse(app: AppContext, args: list[str], *, limit: int, banner_rows: int = 0) -> None:
    known = {
        "-n": "limit",
        "--limit": "limit",
        "-r": "rating",
        "--rating": "rating",
        "-x": "exclude",
        "--exclude": "exclude",
    }
    parsed, positional, unknown = _parse_flags(args, known)
    _flag_help(app, unknown)
    tags = " ".join(positional)
    if not tags:
        warn(app.console, "usage: browse <tags> [-n 25]")
        return
    try:
        requested = int(parsed.get("limit") or limit)
    except ValueError:
        warn(app.console, "-n/--limit must be a number")
        return
    want_images = True

    ratings = parsed.get("rating")
    exclude = parsed.get("exclude")

    with app.console.status(f"[brand]searching {tags}…", spinner="dots"):
        outcome = run_search(
            app,
            tags=tags,
            limit=requested,
            ratings=ratings.split(",") if ratings else None,
            exclude=exclude.split(",") if exclude else None,
            page_size=app.config.general.page_size,
        )
    if not outcome.posts:
        warn(app.console, f"nothing matched '{tags}'")
        # Suggest trying individual tags
        tag_list = tags.split()
        if len(tag_list) > 1:
            app.console.print(
                f"  [muted]hint: try searching tags individually: {', '.join(tag_list)}[/]"
            )
        return

    posts = outcome.posts
    page_size = 3
    page = 0
    total_pages = max(1, -(-len(posts) // page_size))
    inline_layout = bool(banner_rows) and _supports_inline_images()
    while True:
        if inline_layout:
            _clear_content_area(app, banner_rows)
            image_rows, image_cols = _browse_image_box(app, banner_rows, page_size)
            row = banner_rows + 2  # first content row is the page header

        app.console.print(f"[muted]page {page + 1}/{total_pages} · {len(posts)} post(s)[/]")
        start = page * page_size
        for offset, post in enumerate(posts[start : start + page_size], start):
            app.console.print(
                f"[id]{offset + 1}.[/] [id]{post.id}[/]  {_rating_text(post.rating)}  "
                f"{post.width or '?'}×{post.height or '?'}  {post.summary(4)}"
            )
            if want_images:
                if inline_layout:
                    _render_preview(
                        app.console,
                        post.preview_url or post.file_url,
                        place=(image_cols, image_rows, row),
                    )
                    row += 1 + image_rows
                    _write_control(app.console, f"\x1b[{row};1H")
                else:
                    _render_preview(app.console, post.preview_url or post.file_url)
            elif post.preview_url:
                app.console.print(f"   [muted]{post.preview_url}[/]")
                if inline_layout:
                    row += 1
        if not inline_layout:
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
            if inline_layout:
                _clear_content_area(app, banner_rows)
                _write_control(app.console, f"\x1b[{banner_rows + 1};1H")
            return
        raw = raw.strip().lower()
        if raw in {"q", "quit", "exit"}:
            if inline_layout:
                _clear_content_area(app, banner_rows)
                _write_control(app.console, f"\x1b[{banner_rows + 1};1H")
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
        "-r": "rating",
        "--rating": "rating",
        "-x": "exclude",
        "--exclude": "exclude",
        "-F": "filename",
        "--filename": "filename",
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

    ratings = parsed.get("rating")
    exclude = parsed.get("exclude")
    filename = parsed.get("filename")
    shuffle = "shuffle" in flags

    # Default to score sorting unless shuffle is requested
    sort_order = "score" if not shuffle else None

    # Auto-detect sort order in tags
    if filename is None and (
        sort_order == "score" or "sort:" in tags.lower() or "order:" in tags.lower()
    ):
        filename = "{index}_{id}"

    request = DownloadRequest(
        limit=requested,
        output=Path(parsed["output"]).expanduser() if parsed.get("output") else output,
        concurrency=concurrency,
        overwrite="overwrite" in flags,
        dry_run="dry-run" in flags,
        duplicate_check="no-duplicates" in flags,
        archive="archive" in flags or None,
        ratings=ratings.split(",") if ratings else [],
        exclude=exclude.split(",") if exclude else [],
        filename_template=filename,
        shuffle=shuffle,
        order=sort_order,
    )
    with app.console.status(f"[brand]searching {tags}…", spinner="dots"):
        outcome = run_search(
            app,
            tags=tags,
            limit=requested,
            ratings=request.ratings,
            exclude=request.exclude,
            order=request.order,
            page_size=app.config.general.page_size,
        )
    if not outcome.posts:
        warn(app.console, f"nothing matched '{tags}'")
        tag_list = tags.split()
        if len(tag_list) > 1:
            app.console.print(
                f"  [muted]hint: try searching tags individually: {', '.join(tag_list)}[/]"
            )
        return
    summary = run_async(
        download_posts(app, posts=outcome.posts, request=request, title="Downloading")
    )
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
    raise typer.Exit(run_interactive(app, output=output, limit=limit))
