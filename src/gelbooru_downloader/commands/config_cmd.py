"""``gelbooru config`` — inspect and modify configuration, view history."""

from __future__ import annotations

from pathlib import Path
from typing import Annotated, Any

import typer
from rich.table import Table
from rich.text import Text

from ..app import get_context
from ..config import config_template, save_config
from ..console import ok, warn
from ..exceptions import ConfigurationError
from ..models import DownloadStatus
from ..utils import format_timestamp, human_bytes


def show_config(ctx: typer.Context) -> None:
    """Print the effective configuration (secrets redacted)."""

    app = get_context(ctx)
    config = app.config

    table = Table(header_style="table_header", border_style="grey30", expand=False)
    table.add_column("Setting", style="id", no_wrap=True)
    table.add_column("Value", overflow="fold", ratio=1)
    table.add_column("Source", no_wrap=True)

    file_values: dict[str, Any] = {}
    if app.paths.config_file.is_file():
        from ..config import _read_toml

        try:
            raw = _read_toml(app.paths.config_file)
            for section, values in raw.items():
                if isinstance(values, dict):
                    for key, value in values.items():
                        file_values[f"{section}.{key}"] = value
        except Exception as exc:  # pragma: no cover - malformed file is reported elsewhere
            warn(app.console, f"could not read {app.paths.config_file}: {exc}")

    import os

    env_names = {key: f"GELBOORU_{suffix}" for suffix, key in _env_map().items()}

    for dotted, _section, value, _type_name in config.iter_fields():
        shown = _display(dotted, value)
        source = "default"
        if dotted in file_values:
            source = "config file"
        env_var = env_names.get(dotted)
        if env_var and os.environ.get(env_var):
            source = f"${env_var}"
        table.add_row(dotted, shown, Text(source, style="muted"))

    app.console.print(table)
    app.console.print(
        f"[muted]config file:[/] {app.paths.config_file}{'' if app.paths.config_file.is_file() else ' (not created yet)'}"
    )
    app.console.print(f"[muted]presets:[/]     {app.paths.presets_file}")
    app.console.print(f"[muted]history db:[/]  {app.paths.database_file}")
    if not config.network.has_credentials:
        app.console.print(
            "[warn]no API credentials configured.[/] Gelbooru's dapi often answers "
            "[bold]401[/] without them — set $GELBOORU_USER_ID and $GELBOORU_API_KEY."
        )


SECRET_KEYS = frozenset({"network.api_key"})


def _display(key: str, value: Any) -> str:
    """Render a setting for display, masking anything that is a secret."""

    if key in SECRET_KEYS and value:
        return "[muted]********[/] [muted](set)[/]"
    if value is None:
        return "[muted]—[/]"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, Path):
        return str(value)
    return str(value)


def _env_map() -> dict[str, str]:
    from ..config import ENV_MAP

    return ENV_MAP


def set_config(
    ctx: typer.Context,
    key: Annotated[
        str,
        typer.Argument(help="Setting name, e.g. concurrency or general.concurrency."),
    ],
    value: Annotated[str, typer.Argument(help="New value.")],
) -> None:
    """Set a configuration value and write it to the config file."""

    app = get_context(ctx)
    try:
        app.config.set(key, value)
    except ConfigurationError:
        raise
    except ValueError as exc:
        raise typer.BadParameter(str(exc), param_hint=key) from exc

    app.paths.ensure()
    target = save_config(app.config, app.paths.config_file)
    ok(app.console, f"{key} = {_display(key, app.config.get(key))}  [muted](saved to {target})[/]")


def unset_config(
    ctx: typer.Context,
    key: Annotated[str, typer.Argument(help="Setting to reset to its default.")],
) -> None:
    """Reset a setting back to its built-in default."""

    app = get_context(ctx)
    from ..config import Config

    defaults = Config()
    try:
        app.config.set(key, defaults.get(key))
    except ConfigurationError:
        raise
    except ValueError as exc:
        raise typer.BadParameter(str(exc), param_hint=key) from exc
    app.paths.ensure()
    save_config(app.config, app.paths.config_file)
    ok(app.console, f"{key} reset to {_display(key, app.config.get(key))}")


def init_config(
    ctx: typer.Context,
    force: Annotated[
        bool, typer.Option("--force", "-f", help="Overwrite an existing config file.")
    ] = False,
) -> None:
    """Create a commented config.toml in the user configuration directory."""

    app = get_context(ctx)
    target = app.paths.config_file
    if target.exists() and not force:
        warn(app.console, f"{target} already exists (use --force to overwrite)")
        raise typer.Exit(1)
    app.paths.ensure()
    from ..utils import atomic_write_text

    atomic_write_text(target, config_template(), mode=0o600)
    ok(app.console, f"wrote {target}")


def path_config(ctx: typer.Context) -> None:
    """Show where configuration, presets and history are stored."""

    app = get_context(ctx)
    app.console.print(app.paths.describe())


def show_history(
    ctx: typer.Context,
    limit: Annotated[int, typer.Option("--limit", "-n", min=1, help="How many rows to show.")] = 20,
    status: Annotated[
        str | None,
        typer.Option(
            "--status", "-s", help="Filter by status: completed, failed, skipped, unavailable."
        ),
    ] = None,
    stats: Annotated[
        bool, typer.Option("--stats", help="Show aggregate statistics instead of rows.")
    ] = False,
) -> None:
    """Show the download history database."""

    from ..console import format_history_table

    app = get_context(ctx)
    normalized = status.strip().lower() if status else None
    if normalized and normalized not in {item.value for item in DownloadStatus}:
        raise typer.BadParameter(
            f"unknown status {status!r} (use {', '.join(item.value for item in DownloadStatus)})",
            param_hint="--status",
        )

    if stats:
        statistics = app.history.stats()
        table = Table.grid(padding=(0, 2))
        table.add_column(justify="right", style="bold")
        table.add_column()
        table.add_row("Entries", str(statistics.total))
        table.add_row("Completed", str(statistics.completed))
        table.add_row("Skipped", str(statistics.skipped))
        table.add_row("Duplicates", str(statistics.duplicates))
        table.add_row("Failed", str(statistics.failed))
        table.add_row("Unavailable", str(statistics.unavailable))
        table.add_row("Bytes", human_bytes(statistics.bytes_downloaded))
        table.add_row("First", format_timestamp(statistics.first_download))
        table.add_row("Last", format_timestamp(statistics.last_download))
        app.console.print(table)
        return

    entries = app.history.history(limit=limit, status=normalized)
    app.console.print(format_history_table(entries, max_rows=app.config.display.table_max_rows))


def clear_history(
    ctx: typer.Context,
    yes: Annotated[bool, typer.Option("--yes", "-y", help="Do not ask for confirmation.")] = False,
    prune: Annotated[
        bool, typer.Option("--prune", help="Only delete rows whose files no longer exist.")
    ] = False,
) -> None:
    """Delete download history rows (files on disk are never touched)."""

    app = get_context(ctx)
    if prune:
        removed = app.history.forget_missing()
        ok(app.console, f"removed {removed} row(s) whose files are gone")
        return

    total = app.history.stats().total
    if not total:
        app.console.print("[muted]history is already empty[/]")
        return
    if not yes:
        confirmed = typer.confirm(f"delete {total} history row(s)?", default=False)
        if not confirmed:
            app.console.print("[muted]aborted[/]")
            return
    removed = app.history.clear()
    ok(app.console, f"cleared {removed} row(s); downloaded files were left untouched")
