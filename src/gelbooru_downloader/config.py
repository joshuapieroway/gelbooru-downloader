"""Configuration handling: TOML files, environment overrides, user paths.

Precedence is ``defaults < config file < environment variables < CLI options``.
Secrets (API key) live either in the config file (created with ``0600``
permissions) or in the environment; they are never printed in full.
"""

from __future__ import annotations

import contextlib
import os
import sys
import tomllib
from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field, ValidationError, field_validator

from .exceptions import ConfigurationError, StorageError
from .utils import atomic_write_text, coerce_bool, coerce_float, coerce_int

__all__ = [
    "APP_NAME",
    "AppPaths",
    "Config",
    "DisplayConfig",
    "GeneralConfig",
    "NetworkConfig",
    "Settings",
    "config_template",
    "load_config",
    "redacted_config",
    "save_config",
]

APP_NAME = "gelbooru-downloader"

#: Environment variable prefix for every configuration override.
ENV_PREFIX = "GELBOORU_"

AuthMode = Literal["params", "basic", "none"]


class GeneralConfig(BaseModel):
    """Download behaviour defaults."""

    model_config = ConfigDict(extra="forbid")

    output_dir: Path = Path("./downloads")
    concurrency: int = Field(default=5, ge=1, le=64)
    page_size: int = Field(default=100, ge=1, le=100)
    retries: int = Field(default=3, ge=0, le=10)
    timeout: float = Field(default=30.0, gt=0, le=600)
    skip_existing: bool = True
    archive_metadata: bool = False
    filename_template: str = "{id}"
    max_downloads_per_run: int = Field(default=0, ge=0)

    @field_validator("output_dir", mode="before")
    @classmethod
    def _expand(cls, value: Any) -> Any:
        if isinstance(value, str):
            return Path(os.path.expandvars(value)).expanduser()
        return value

    @field_validator("filename_template")
    @classmethod
    def _check_template(cls, value: str) -> str:
        if not value.strip():
            msg = "filename_template cannot be empty"
            raise ValueError(msg)
        if "{" in value and "}" not in value:
            msg = "filename_template has an unclosed '{'"
            raise ValueError(msg)
        return value.strip()

    @field_validator("filename_template")
    @classmethod
    def _reject_traversal(cls, value: str) -> str:
        if any(sep in value for sep in ("/", "\\")) or ".." in value:
            msg = "filename_template must not contain path separators"
            raise ValueError(msg)
        return value


class NetworkConfig(BaseModel):
    """Everything about talking to the server."""

    model_config = ConfigDict(extra="forbid")

    base_url: str = "https://gelbooru.com"
    user_agent: str = "GelbooruCLI/1.0 (+https://github.com/joshuapieroway/gelbooru-downloader)"
    #: Gelbooru's CDN hotlink-protects original files: requests without a
    #: ``Referer`` are redirected to an HTML page instead of the image. This is
    #: only sent for gelbooru.com hosts, never to arbitrary mirrors.
    referer: str | None = "https://gelbooru.com/"
    api_key: str | None = None
    user_id: str | None = None
    auth_mode: AuthMode = "params"
    response_format: Literal["json", "xml", "auto"] = "auto"

    @field_validator("base_url")
    @classmethod
    def _check_base_url(cls, value: str) -> str:
        from .utils import is_http_url

        cleaned = value.strip().rstrip("/")
        if not is_http_url(cleaned):
            msg = f"base_url must be an http(s) URL, got {value!r}"
            raise ValueError(msg)
        return cleaned

    @field_validator("user_agent")
    @classmethod
    def _check_user_agent(cls, value: str) -> str:
        cleaned = value.strip()
        if not cleaned:
            msg = "user_agent cannot be empty"
            raise ValueError(msg)
        if "\n" in cleaned or "\r" in cleaned:
            msg = "user_agent must not contain newlines"
            raise ValueError(msg)
        return cleaned

    @field_validator("referer", mode="before")
    @classmethod
    def _blank_referer(cls, value: Any) -> Any:
        if isinstance(value, str) and not value.strip():
            return None
        return value

    @field_validator("api_key", "user_id", mode="before")
    @classmethod
    def _blank_to_none(cls, value: Any) -> Any:
        if isinstance(value, str) and not value.strip():
            return None
        return value

    @property
    def has_credentials(self) -> bool:
        return bool(self.api_key and self.user_id)


class DisplayConfig(BaseModel):
    """Terminal presentation."""

    model_config = ConfigDict(extra="forbid")

    color: bool = True
    show_banner: bool = True
    show_progress: bool = True
    table_max_rows: int = Field(default=40, ge=1, le=500)
    tag_preview_count: int = Field(default=6, ge=0, le=50)


class Config(BaseModel):
    """The full application configuration."""

    model_config = ConfigDict(extra="forbid")

    general: GeneralConfig = Field(default_factory=GeneralConfig)
    network: NetworkConfig = Field(default_factory=NetworkConfig)
    display: DisplayConfig = Field(default_factory=DisplayConfig)

    # --- lookups ---------------------------------------------------------- #
    def section_models(self) -> dict[str, type[BaseModel]]:
        return {"general": GeneralConfig, "network": NetworkConfig, "display": DisplayConfig}

    def iter_fields(self) -> Iterator[tuple[str, str, Any, str]]:
        """Yield ``(dotted_key, section, raw_value, type_name)`` for every setting."""

        for section in self.section_models():
            sub = getattr(self, section)
            for name, field in type(sub).model_fields.items():
                yield f"{section}.{name}", section, getattr(sub, name), str(field.annotation)

    def get(self, dotted_key: str) -> Any:
        """Get a setting by dotted key (``general.concurrency``) or bare name."""

        section, name = self._split_key(dotted_key)
        return getattr(getattr(self, section), name)

    def set(self, dotted_key: str, value: Any) -> None:
        """Set a setting by dotted key or bare name, validating the value."""

        section, name = self._split_key(dotted_key)
        model = getattr(self, section)
        raw_annotation = type(model).model_fields[name].annotation
        coerced = _coerce_like(value, raw_annotation)
        updated = model.model_validate({**model.model_dump(), name: coerced})
        setattr(self, section, updated)

    def _split_key(self, key: str) -> tuple[str, str]:
        text = str(key).strip()
        if "." in text:
            section, _, name = text.partition(".")
        else:
            matches = [
                section_name
                for section_name, section_model in self.section_models().items()
                if text in section_model.model_fields
            ]
            if len(matches) != 1:
                msg = (
                    f"unknown configuration key {key!r}"
                    if not matches
                    else f"ambiguous key {key!r}: use {', '.join(f'{m}.{text}' for m in matches)}"
                )
                raise ConfigurationError(msg)
            section, name = matches[0], text
        if section not in self.section_models():
            msg = f"unknown configuration section {section!r} (use general, network or display)"
            raise ConfigurationError(msg)
        if name not in self.section_models()[section].model_fields:
            valid = ", ".join(sorted(self.section_models()[section].model_fields))
            msg = f"unknown configuration key {section}.{name} (valid keys: {valid})"
            raise ConfigurationError(msg)
        return section, name

    def to_dict(self) -> dict[str, Any]:
        data = self.model_dump(mode="python")
        for value in data.values():
            if isinstance(value, dict) and isinstance(value.get("output_dir"), Path):
                value["output_dir"] = str(value["output_dir"])
        return data

    def redacted(self) -> dict[str, Any]:
        """A dict safe to print: secrets replaced by ``********``."""

        data = self.to_dict()
        network = data.get("network", {})
        for secret in ("api_key",):
            if network.get(secret):
                network[secret] = "********"
        return data

    def with_overrides(self, overrides: dict[str, Any]) -> Config:
        """Return a copy with ``{dotted_key: value}`` applied (highest precedence)."""

        clone = self.model_copy(deep=True)
        for key, value in overrides.items():
            if value is None:
                continue
            clone.set(key, value)
        return clone


# Backwards friendly alias used in the CLI.
Settings = Config


def _coerce_like(value: Any, annotation: Any) -> Any:
    """Coerce a CLI string into the type the config field expects."""

    text = str(annotation)
    if annotation is bool or "bool" in text:
        return coerce_bool(value)
    if annotation is int or ("int" in text and "print" not in text):
        return coerce_int(value)
    if annotation is float or "float" in text:
        return coerce_float(value)
    if annotation is Path or "Path" in text:
        return Path(str(value)).expanduser()
    if "Literal" in text:
        return str(value).strip().lower()
    return value


# --------------------------------------------------------------------------- #
# Paths
# --------------------------------------------------------------------------- #
@dataclass(frozen=True, slots=True)
class AppPaths:
    """Where configuration, presets and the history database live."""

    config_dir: Path
    data_dir: Path
    config_file: Path
    presets_file: Path
    database_file: Path

    @classmethod
    def discover(cls, *, home: Path | None = None, portable: bool = False) -> AppPaths:
        """Resolve platform-appropriate directories.

        * portable mode keeps everything next to the executable
        * macOS uses ``~/Library/Application Support``
        * Windows uses ``%APPDATA%``/``%LOCALAPPDATA%``
        * everything else follows the XDG base directory spec
        """

        if home is not None:
            root = Path(home).expanduser().resolve()
            config_dir = data_dir = root / APP_NAME
            return cls._build(config_dir, data_dir)

        if portable:
            base = _executable_dir()
            return cls._build(base / f".{APP_NAME}", base / f".{APP_NAME}")

        if sys.platform == "darwin":
            support = Path.home() / "Library" / "Application Support"
            config_dir = support / APP_NAME
            data_dir = support / APP_NAME
            return cls._build(config_dir, data_dir)

        if os.name == "nt":
            appdata = os.environ.get("APPDATA") or str(Path.home() / "AppData" / "Roaming")
            local = os.environ.get("LOCALAPPDATA") or str(Path.home() / "AppData" / "Local")
            return cls._build(Path(appdata) / APP_NAME, Path(local) / APP_NAME)

        config_home = Path(os.environ.get("XDG_CONFIG_HOME") or Path.home() / ".config")
        data_home = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local" / "share")
        return cls._build(config_home / APP_NAME, data_home / APP_NAME)

    @classmethod
    def _build(cls, config_dir: Path, data_dir: Path) -> AppPaths:
        config_dir = config_dir.expanduser()
        data_dir = data_dir.expanduser()
        return cls(
            config_dir=config_dir,
            data_dir=data_dir,
            config_file=config_dir / "config.toml",
            presets_file=config_dir / "presets.json",
            database_file=data_dir / "history.db",
        )

    def ensure(self) -> AppPaths:
        """Create the config and data directories with sane permissions."""

        for directory in (self.config_dir, self.data_dir):
            directory.mkdir(parents=True, exist_ok=True)
            with contextlib.suppress(OSError):  # pragma: no cover - unusual filesystems
                directory.chmod(0o700)
        return self

    def describe(self) -> str:
        return (
            f"config   {self.config_file}\n"
            f"presets  {self.presets_file}\n"
            f"history  {self.database_file}"
        )


def _executable_dir() -> Path:
    """Directory of the running interpreter/executable, best effort."""

    if getattr(sys, "frozen", False):  # PyInstaller / pipx shim
        return Path(sys.executable).resolve().parent
    argv0 = sys.argv[0] if sys.argv and sys.argv[0] else ""
    if argv0:
        candidate = Path(argv0).expanduser()
        if candidate.is_file() or candidate.exists():
            return candidate.resolve().parent
        if candidate.parent.exists():
            return candidate.parent.resolve()
    return Path.cwd()


# --------------------------------------------------------------------------- #
# Environment overrides
# --------------------------------------------------------------------------- #
#: ``env var -> dotted config key``.
ENV_MAP: dict[str, str] = {
    "OUTPUT_DIR": "general.output_dir",
    "CONCURRENCY": "general.concurrency",
    "PAGE_SIZE": "general.page_size",
    "RETRIES": "general.retries",
    "TIMEOUT": "general.timeout",
    "SKIP_EXISTING": "general.skip_existing",
    "ARCHIVE_METADATA": "general.archive_metadata",
    "FILENAME_TEMPLATE": "general.filename_template",
    "MAX_DOWNLOADS_PER_RUN": "general.max_downloads_per_run",
    "BASE_URL": "network.base_url",
    "USER_AGENT": "network.user_agent",
    "REFERER": "network.referer",
    "API_KEY": "network.api_key",
    "USER_ID": "network.user_id",
    "AUTH_MODE": "network.auth_mode",
    "RESPONSE_FORMAT": "network.response_format",
    "COLOR": "display.color",
    "SHOW_BANNER": "display.show_banner",
    "SHOW_PROGRESS": "display.show_progress",
    "TABLE_MAX_ROWS": "display.table_max_rows",
    "TAG_PREVIEW_COUNT": "display.tag_preview_count",
}


def env_overrides(environ: dict[str, str] | None = None) -> dict[str, str]:
    """Collect configuration overrides from the environment."""

    source = os.environ if environ is None else environ
    overrides: dict[str, str] = {}
    for suffix, key in ENV_MAP.items():
        value = source.get(f"{ENV_PREFIX}{suffix}")
        if value is not None and value.strip():
            overrides[key] = value.strip()
    return overrides


def portable_from_env(environ: dict[str, str] | None = None) -> bool:
    """Whether ``GELBOORU_PORTABLE`` requests portable mode."""

    source = os.environ if environ is None else environ
    raw = source.get(f"{ENV_PREFIX}PORTABLE", "")
    if not raw.strip():
        return False
    try:
        return coerce_bool(raw)
    except ValueError:
        return False


# --------------------------------------------------------------------------- #
# Load / save
# --------------------------------------------------------------------------- #
def config_template() -> str:
    """A commented starter configuration file."""

    return """\
# Gelbooru Downloader configuration
# Docs: https://github.com/joshuapieroway/gelbooru-downloader#configuration

[general]
# Where downloads are written (relative paths resolve against the current directory).
output_dir = "./downloads"
# Number of simultaneous image downloads (1-64).
concurrency = 5
# Posts requested per API page (1-100; Gelbooru hard limit).
page_size = 100
# Retries for transient network/HTTP failures.
retries = 3
# Per-request timeout in seconds.
timeout = 30.0
# Skip posts whose file already exists on disk or in the history database.
skip_existing = true
# Write a <post>.json sidecar next to every downloaded file.
archive_metadata = false
# Filename template; available fields: {id} {md5} {index} {rating}
filename_template = "{id}"
# 0 = unlimited safety cap for a single run.
max_downloads_per_run = 0

[network]
base_url = "https://gelbooru.com"
user_agent = "GelbooruCLI/1.0 (+https://github.com/joshuapieroway/gelbooru-downloader)"
# Gelbooru's CDN hotlink-protects original files and redirects requests without a
# Referer to an HTML page. Sent for gelbooru.com hosts only; set to "" to disable.
referer = "https://gelbooru.com/"
# Credentials are optional in the file but recommended: the dapi may answer 401.
# Prefer GELBOORU_USER_ID / GELBOORU_API_KEY environment variables instead.
api_key = ""
user_id = ""
# "params" (default), "basic" (HTTP basic auth) or "none".
auth_mode = "params"
# "json", "xml" or "auto" (auto prefers JSON and falls back to XML).
response_format = "auto"

[display]
color = true
show_banner = true
show_progress = true
# Maximum number of rows rendered in result tables.
table_max_rows = 40
# Number of tags shown per post in tables.
tag_preview_count = 6
"""


def _read_toml(path: Path) -> dict[str, Any]:
    try:
        with path.open("rb") as handle:
            return tomllib.load(handle)
    except tomllib.TOMLDecodeError as exc:
        msg = f"{path} is not valid TOML: {exc}"
        raise ConfigurationError(msg) from exc
    except OSError as exc:
        msg = f"cannot read configuration file {path}: {exc}"
        raise StorageError(msg) from exc


def load_config(
    path: Path | str | None = None,
    *,
    portable: bool = False,
    environ: dict[str, str] | None = None,
    use_env: bool = True,
    overrides: dict[str, Any] | None = None,
) -> tuple[Config, AppPaths]:
    """Load the configuration, resolving paths and applying precedence.

    Args:
        path: explicit config file; when given, presets and the database live
            next to it (handy for portable setups and for tests).
        portable: keep state beside the executable instead of in the user dirs.
        environ: environment mapping (defaults to :data:`os.environ`).
        use_env: set to ``False`` to ignore environment overrides.
        overrides: highest-precedence values, typically CLI options.

    Returns:
        The effective :class:`Config` and the resolved :class:`AppPaths`.
    """

    source = os.environ if environ is None else environ
    env_home = source.get(f"{ENV_PREFIX}HOME") if use_env else None
    env_config = source.get(f"{ENV_PREFIX}CONFIG") if use_env else None

    explicit = Path(path).expanduser() if path else None
    if explicit is None and env_config:
        explicit = Path(env_config).expanduser()

    if explicit is not None:
        root = explicit.parent.expanduser().resolve()
        # A config file's directory also holds presets and the history database.
        paths = AppPaths(
            config_dir=root,
            data_dir=root,
            config_file=explicit,
            presets_file=root / "presets.json",
            database_file=root / "history.db",
        )
    else:
        use_portable = portable or (use_env and portable_from_env(source))
        if env_home:
            paths = AppPaths.discover(home=Path(env_home))
        else:
            paths = AppPaths.discover(portable=use_portable)

    config = Config()
    if paths.config_file.is_file():
        data = _read_toml(paths.config_file)
        config = _config_from_mapping(data, paths.config_file)

    if use_env:
        for key, value in env_overrides(source).items():
            config.set(key, value)

    if overrides:
        config = config.with_overrides(overrides)

    return config, paths


def _config_from_mapping(data: dict[str, Any], source: Path) -> Config:
    unknown = set(data) - {"general", "network", "display"}
    if unknown:
        msg = f"{source} contains unknown sections: {', '.join(sorted(unknown))}"
        raise ConfigurationError(msg)
    try:
        return Config.model_validate(data)
    except ValidationError as exc:
        details = "; ".join(
            f"{'.'.join(str(part) for part in error['loc']) or '?'}: {error['msg']}"
            for error in exc.errors()
        )
        msg = f"invalid configuration in {source}: {details}"
        raise ConfigurationError(msg) from exc


def save_config(config: Config, path: Path | str, *, secure: bool = True) -> Path:
    """Write ``config`` to ``path`` as TOML with ``0600`` permissions."""

    target = Path(path).expanduser()
    target.parent.mkdir(parents=True, exist_ok=True)
    lines: list[str] = [
        "# Gelbooru Downloader configuration",
        f"# Generated by gelbooru {__import__('gelbooru_downloader').__version__}",
        "",
    ]
    for section, model in config.section_models().items():
        lines.append(f"[{section}]")
        sub = getattr(config, section)
        for name in model.model_fields:
            value = getattr(sub, name)
            if value is None:
                rendered = '""'
            elif isinstance(value, bool):
                rendered = "true" if value else "false"
            elif isinstance(value, (int, float)):
                rendered = repr(value)
            else:
                rendered = f'"{value}"'
            lines.append(f"{name} = {rendered}")
        lines.append("")
    body = "\n".join(lines)

    mode = 0o600 if secure else 0o644
    atomic_write_text(target, body, mode=mode)
    with contextlib.suppress(OSError):  # pragma: no cover - exotic filesystems
        target.chmod(mode)
    return target


def redacted_config(config: Config) -> dict[str, Any]:
    """Public alias for :meth:`Config.redacted`."""

    return config.redacted()
