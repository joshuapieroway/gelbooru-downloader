"""The ``gelbooru`` command line interface.

Global options live on the root callback (``--config``, ``--portable``,
``--no-color``, ``--quiet``, ``--verbose``); everything else is a sub-command.
Commands receive a ready-to-use :class:`~.app.AppContext` instead of building
their own configuration, database or console.
"""

from __future__ import annotations

import functools
import logging
import os
import sys
from collections.abc import Callable
from pathlib import Path
from typing import Annotated, Any

import typer
from rich.console import Console
from rich.logging import RichHandler

from . import __version__
from .app import AppContext, create_context, get_context, set_context
from .commands import batch as batch_commands
from .commands import browse as browse_commands
from .commands import config_cmd
from .commands import download as download_commands
from .commands import export as export_commands
from .commands import interactive as interactive_commands
from .commands import post as post_commands
from .commands import presets as preset_commands
from .commands import search as search_commands
from .console import error
from .exceptions import GelbooruError

__all__ = ["app", "main"]

log = logging.getLogger("gelbooru")

_HELP = """\
A fast terminal client for the Gelbooru image board.

[bold]Typical use[/]
  [info]gelbooru search "blue_hair solo" --limit 100[/]
  [info]gelbooru download "landscape" --limit 50 --output ./downloads/landscape[/]
  [info]gelbooru browse "blue_hair solo" -n 12 --images[/]
  [info]gelbooru export "blue_hair solo" json results.json[/]
  [info]gelbooru post info 1234567[/]
  [info]gelbooru presets add anime "blue_hair solo"[/]

[bold]Getting started[/]
  Gelbooru's API usually requires credentials:
  [info]export GELBOORU_USER_ID=1234 GELBOORU_API_KEY=…[/]

Run [info]gelbooru <command> --help[/] for command specific options.
"""


# --------------------------------------------------------------------------- #
# Logging
# --------------------------------------------------------------------------- #
class _RedactFilter(logging.Filter):
    """Keep credentials out of the logs."""

    def filter(self, record: logging.LogRecord) -> bool:
        try:
            message = record.getMessage()
        except Exception:
            return True
        secret = os.environ.get("GELBOORU_API_KEY", "")
        if secret and secret in message:
            record.msg = message.replace(secret, "***redacted***")
            record.args = ()
        return True


def configure_logging(verbose: int, quiet: bool, log_file: Path | None = None) -> None:
    """Set up console (and optional file) logging."""

    level = logging.WARNING
    if verbose == 1:
        level = logging.INFO
    elif verbose >= 2:
        level = logging.DEBUG
    if quiet:
        level = logging.ERROR

    console = Console(stderr=True)
    handlers: list[logging.Handler] = [
        RichHandler(console=console, rich_tracebacks=True, show_path=verbose >= 2)
    ]
    if log_file is not None:
        log_file.parent.mkdir(parents=True, exist_ok=True)
        file_handler = logging.FileHandler(log_file, encoding="utf-8")
        file_handler.setFormatter(
            logging.Formatter("%(asctime)s %(levelname)-7s %(name)s: %(message)s")
        )
        file_handler.setLevel(logging.DEBUG)
        handlers.append(file_handler)

    logging.basicConfig(level=level, handlers=handlers, force=True)
    for handler in handlers:
        handler.addFilter(_RedactFilter())


def command_guard[F: Callable[..., Any]](func: F) -> F:
    """Turn domain errors into friendly messages instead of tracebacks."""

    @functools.wraps(func)
    def wrapper(*args: Any, **kwargs: Any) -> Any:
        try:
            return func(*args, **kwargs)
        except GelbooruError as exc:
            error(_console_for(), str(exc), hint=exc.hint)
            raise typer.Exit(code=1) from exc
        except KeyboardInterrupt:  # pragma: no cover - interactive only
            _console_for().print("\n[warn]interrupted[/]")
            raise typer.Exit(code=130) from None
        except BrokenPipeError:  # pragma: no cover - `| head` and friends
            raise typer.Exit(code=0) from None

    return wrapper


def _console_for() -> Console:
    try:
        return get_context().console
    except RuntimeError:  # pragma: no cover - before the callback ran
        return Console(stderr=True)


# --------------------------------------------------------------------------- #
# Typer application
# --------------------------------------------------------------------------- #
app = typer.Typer(
    name="gelbooru",
    help=_HELP,
    add_completion=True,
    no_args_is_help=True,
    rich_markup_mode="rich",
    context_settings={"help_option_names": ["-h", "--help"]},
)

post_app = typer.Typer(no_args_is_help=True, help="Inspect and download individual posts by id.")
presets_app = typer.Typer(no_args_is_help=True, help="Save and reuse tag searches.")
config_app = typer.Typer(no_args_is_help=True, help="Inspect and change configuration and history.")


@app.callback()
def main_callback(
    ctx: typer.Context,
    config_path: Annotated[
        Path | None,
        typer.Option(
            "--config", "-c", help="Use this config file (presets and history live beside it)."
        ),
    ] = None,
    portable: Annotated[
        bool,
        typer.Option("--portable", help="Store configuration and history beside the executable."),
    ] = False,
    color: Annotated[
        bool | None,
        typer.Option("--color/--no-color", help="Force coloured output on or off."),
    ] = None,
    quiet: Annotated[
        bool, typer.Option("--quiet", "-q", help="Only show warnings and errors.")
    ] = False,
    verbose: Annotated[
        int,
        typer.Option("--verbose", "-v", count=True, help="Increase logging (-v info, -vv debug)."),
    ] = 0,
    log_file: Annotated[
        Path | None,
        typer.Option("--log-file", help="Also write debug logs to this file."),
    ] = None,
) -> None:
    """Set up configuration, logging and the shared application context."""

    configure_logging(verbose, quiet, log_file)
    context: AppContext = create_context(
        config_path=config_path, portable=portable, color=color, quiet=quiet
    )
    set_context(context)
    ctx.obj = context


@app.command("version")
@command_guard
def version_command(ctx: typer.Context) -> None:
    """Show the application version and runtime details."""

    import platform

    import httpx

    context = get_context(ctx)
    context.console.print(f"[brand]gelbooru[/] [bold]{__version__}[/]")
    context.console.print(
        f"[muted]python[/]     {platform.python_version()} ({platform.system()} {platform.machine()})"
    )
    context.console.print(f"[muted]httpx[/]      {httpx.__version__}")
    context.console.print(f"[muted]config[/]     {context.paths.config_file}")
    context.console.print(f"[muted]database[/]   {context.paths.database_file}")
    context.console.print(f"[muted]api[/]        {context.config.network.base_url}")
    if not context.config.network.has_credentials:
        context.console.print(
            "[warn]no API credentials configured ($GELBOORU_USER_ID / $GELBOORU_API_KEY)[/]"
        )


app.command("search", help="Search Gelbooru for posts.")(command_guard(search_commands.search))
app.command("download", help="Search for posts and download them.")(
    command_guard(download_commands.download)
)
app.command("batch", help="Run many searches from a file.")(command_guard(batch_commands.batch))
app.command("browse", help="Browse search results as a paged list with thumbnails.")(
    command_guard(browse_commands.browse)
)
app.command("export", help="Export search results to JSON or CSV.")(
    command_guard(export_commands.export)
)
app.command("watch", help="Repeat a search on an interval and fetch new posts.")(
    command_guard(batch_commands.watch)
)
app.command("interactive", help="Explore search, inspect and download in a REPL.")(
    command_guard(interactive_commands.interactive_command)
)

post_app.command("info", help="Show everything the API knows about one post.")(
    command_guard(post_commands.post_info)
)
post_app.command("download", help="Download one or more posts by id.")(
    command_guard(post_commands.post_download)
)

presets_app.command("list", help="List saved presets.")(command_guard(preset_commands.list_presets))
presets_app.command("add", help="Save a tag search as a preset.")(
    command_guard(preset_commands.add_preset)
)
presets_app.command("remove", help="Delete a preset.")(command_guard(preset_commands.remove_preset))
presets_app.command("show", help="Show one preset.")(command_guard(preset_commands.show_preset))
presets_app.command("download", help="Download everything matching a preset.")(
    command_guard(preset_commands.download_preset)
)

config_app.command("show", help="Show the effective configuration.")(
    command_guard(config_cmd.show_config)
)
config_app.command("set", help="Set a configuration value.")(command_guard(config_cmd.set_config))
config_app.command("unset", help="Reset a configuration value to its default.")(
    command_guard(config_cmd.unset_config)
)
config_app.command("init", help="Write a commented config.toml.")(
    command_guard(config_cmd.init_config)
)
config_app.command("path", help="Show where configuration, presets and history live.")(
    command_guard(config_cmd.path_config)
)
config_app.command("history", help="Show the download history.")(
    command_guard(config_cmd.show_history)
)
config_app.command("clear", help="Clear the download history (never touches files).")(
    command_guard(config_cmd.clear_history)
)

app.add_typer(post_app, name="post")
app.add_typer(presets_app, name="presets")
app.add_typer(config_app, name="config")


def main(argv: list[str] | None = None) -> int:
    """Console script entry point."""

    try:
        app(args=argv, standalone_mode=True)
    except SystemExit as exc:  # click/typer exits on its own in normal operation
        return int(exc.code or 0)
    except KeyboardInterrupt:  # pragma: no cover - interactive only
        _console_for().print("\n[warn]interrupted[/]")
        return 130
    finally:
        try:
            context = get_context()
        except RuntimeError:  # pragma: no cover - help/version paths
            context = None
        if context is not None:
            context.close()
            set_context(None)
    return 0


if __name__ == "__main__":  # pragma: no cover
    sys.exit(main())
