"""Application wiring shared by the CLI, interactive mode and tests.

The :class:`AppContext` owns the configuration, the resolved paths, the Rich
console, the history database and factory helpers for API clients and
downloaders. Commands receive one and never construct those objects themselves,
which keeps them short and makes them trivial to test.
"""

from __future__ import annotations

import asyncio
import logging
from collections.abc import Awaitable, Callable
from contextlib import asynccontextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import httpx
import typer

from .api import GelbooruClient
from .config import AppPaths, Config, load_config
from .console import ConsoleFactory
from .database import HistoryDatabase
from .downloader import Downloader, DownloadOptions, ProgressReporter
from .exceptions import GelbooruError
from .presets import PresetStore

__all__ = [
    "AppContext",
    "GelbooruError",
    "api_client",
    "build_client",
    "create_context",
    "get_context",
    "resolve_limit",
    "run_async",
    "set_context",
]

log = logging.getLogger("gelbooru")

#: The context for the current CLI invocation (``None`` outside of a command).
_current: AppContext | None = None


def set_context(context: AppContext | None) -> None:
    """Remember the active application context for command functions."""

    global _current  # noqa: PLW0603 - one CLI process, one context
    _current = context


def get_context(ctx: typer.Context | None = None) -> AppContext:
    """Return the context created by the root CLI callback.

    Falls back to a default configuration when used as a library, so importing
    and calling a command helper from Python does not explode.
    """

    if ctx is not None and isinstance(ctx.obj, AppContext):
        return ctx.obj
    if _current is None:
        raise RuntimeError(
            "application context is not initialised — run this through the gelbooru CLI"
        )
    return _current


def run_async[T](coro: Awaitable[T]) -> T:
    """Run a coroutine from synchronous CLI code."""

    try:
        asyncio.get_running_loop()
    except RuntimeError:
        return asyncio.run(coro)  # type: ignore[arg-type]
    msg = "run_async() cannot be called from inside a running event loop"
    raise RuntimeError(msg)


def build_client(
    config: Config,
    *,
    transport: httpx.AsyncBaseTransport | None = None,
    timeout: float | None = None,
    retries: int | None = None,
    sleep: Callable[[float], Awaitable[None]] | None = None,
) -> GelbooruClient:
    """Create a :class:`~.api.GelbooruClient` from the configuration."""

    kwargs: dict[str, Any] = {"transport": transport}
    if timeout is not None:
        kwargs["timeout"] = timeout
    if retries is not None:
        kwargs["retries"] = retries
    if sleep is not None:
        kwargs["sleep"] = sleep
    return GelbooruClient(config, **kwargs)


@dataclass
class AppContext:
    """Everything a command needs to do its job."""

    config: Config
    paths: AppPaths
    console_factory: ConsoleFactory
    quiet: bool = False
    _history: HistoryDatabase | None = None
    _presets: PresetStore | None = None

    # -- lazily created collaborators -------------------------------------- #
    @property
    def console(self) -> Any:
        return self.console_factory.console

    @property
    def history(self) -> HistoryDatabase:
        if self._history is None:
            self.paths.ensure()
            self._history = HistoryDatabase(self.paths.database_file)
        return self._history

    @property
    def presets(self) -> PresetStore:
        if self._presets is None:
            self._presets = PresetStore(self.paths.presets_file)
        return self._presets

    def close(self) -> None:
        if self._history is not None:
            self._history.close()
            self._history = None

    def __enter__(self) -> AppContext:
        return self

    def __exit__(self, *exc_info: object) -> None:
        self.close()

    # -- factories --------------------------------------------------------- #
    def client(self, **kwargs: Any) -> GelbooruClient:
        return build_client(self.config, **kwargs)

    def downloader(
        self,
        *,
        history: HistoryDatabase | None = None,
        reporter: ProgressReporter | None = None,
        transport: httpx.AsyncBaseTransport | None = None,
        sleep: Callable[[float], Awaitable[None]] | None = None,
        **option_overrides: Any,
    ) -> Downloader:
        """Build a :class:`~.downloader.Downloader` bound to this context."""

        options = DownloadOptions.from_config(self.config, **option_overrides)
        return self.downloader_with_options(
            options,
            history=history,
            reporter=reporter,
            transport=transport,
            sleep=sleep,
        )

    def downloader_with_options(
        self,
        options: DownloadOptions,
        *,
        history: HistoryDatabase | None = None,
        reporter: ProgressReporter | None = None,
        transport: httpx.AsyncBaseTransport | None = None,
        sleep: Callable[[float], Awaitable[None]] | None = None,
    ) -> Downloader:
        """Build a downloader from a fully built :class:`DownloadOptions`."""

        return Downloader(
            options,
            history=history if history is not None else self.history,
            reporter=reporter,
            transport=transport,
            sleep=sleep or asyncio.sleep,
        )

    # -- helpers ----------------------------------------------------------- #
    def notify(self, message: str) -> None:
        if not self.quiet:
            self.console.print(f"[muted]{message}[/]")

    def banner(self, subtitle: str = "") -> None:
        from . import __version__
        from .console import banner as make_banner

        if self.config.display.show_banner and not self.quiet:
            self.console.print(make_banner(__version__, subtitle))


def create_context(
    *,
    config_path: Path | None = None,
    portable: bool = False,
    color: bool | None = None,
    quiet: bool = False,
    overrides: dict[str, Any] | None = None,
    use_env: bool = True,
    environ: dict[str, str] | None = None,
    force_terminal: bool | None = None,
) -> AppContext:
    """Load configuration and build an :class:`AppContext`."""

    merged: dict[str, Any] = dict(overrides or {})
    if color is not None:
        merged["display.color"] = color

    config, paths = load_config(
        config_path,
        portable=portable,
        overrides=merged,
        use_env=use_env,
        environ=environ,
    )
    factory = ConsoleFactory(color=config.display.color, force_terminal=force_terminal)
    return AppContext(config=config, paths=paths, console_factory=factory, quiet=quiet)


@asynccontextmanager
async def api_client(app: AppContext, **kwargs: Any) -> Any:
    """Async context manager yielding a configured API client."""

    client = app.client(**kwargs)
    try:
        yield client
    finally:
        await client.aclose()


def resolve_limit(requested: int | None, config: Config) -> int | None:
    """Apply ``general.max_downloads_per_run`` as a safety cap on a limit."""

    cap = config.general.max_downloads_per_run
    if cap and cap > 0:
        if requested is None:
            return cap
        return min(requested, cap)
    return requested
