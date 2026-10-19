"""Small, dependency-light helpers shared across the application.

Everything here is pure (or filesystem-only) so it can be unit tested without
touching the network.
"""

from __future__ import annotations

import hashlib
import math
import os
import random
import re
import unicodedata
from collections.abc import Iterable, Iterator, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime
from email.utils import parsedate_to_datetime
from pathlib import Path
from typing import Any
from urllib.parse import unquote, urlsplit

__all__ = [
    "RetryPolicy",
    "TagQuery",
    "atomic_write_bytes",
    "atomic_write_text",
    "chunks",
    "coerce_bool",
    "coerce_float",
    "coerce_int",
    "compute_backoff",
    "content_type_to_extension",
    "dedupe_preserve_order",
    "ensure_directory",
    "extension_from_url",
    "format_timestamp",
    "human_bytes",
    "human_duration",
    "human_number",
    "is_http_url",
    "parse_retry_after",
    "parse_size",
    "parse_tag_expression",
    "sanitize_filename",
    "sanitize_path_component",
    "sha256_file",
    "shuffled",
    "truncate_tags",
]

#: Characters that are illegal in filenames on at least one supported platform.
_ILLEGAL_FILENAME_CHARS = re.compile(r'[<>:"/\\|?*\x00-\x1f\x7f]')

#: Windows reserved device names (case-insensitive, with or without extension).
_WINDOWS_RESERVED = frozenset(
    {"con", "prn", "aux", "nul"}
    | {f"com{i}" for i in range(1, 10)}
    | {f"lpt{i}" for i in range(1, 10)}
)

#: Maximum length we allow for a single path component.
_MAX_COMPONENT_LEN = 120

_SIZE_UNITS: dict[str, int] = {
    "": 1,
    "b": 1,
    "k": 1000,
    "kb": 1000,
    "kib": 1024,
    "m": 1000**2,
    "mb": 1000**2,
    "mib": 1024**2,
    "g": 1000**3,
    "gb": 1000**3,
    "gib": 1024**3,
    "t": 1000**4,
    "tb": 1000**4,
    "tib": 1024**4,
}

_CONTENT_TYPES: dict[str, str] = {
    "image/jpeg": "jpg",
    "image/jpg": "jpg",
    "image/pjpeg": "jpg",
    "image/png": "png",
    "image/apng": "png",
    "image/gif": "gif",
    "image/webp": "webp",
    "image/avif": "avif",
    "image/jxl": "jxl",
    "image/svg+xml": "svg",
    "image/x-icon": "ico",
    "image/bmp": "bmp",
    "image/tiff": "tiff",
    "image/heic": "heic",
    "video/webm": "webm",
    "video/mp4": "mp4",
    "application/octet-stream": "bin",
}


# --------------------------------------------------------------------------- #
# Numbers, sizes and durations
# --------------------------------------------------------------------------- #
def human_bytes(size: float | int | None) -> str:
    """Format a byte count using binary-ish units, e.g. ``1.4 MiB``."""

    if size is None:
        return "unknown"
    value = float(size)
    if value < 0:
        return f"-{human_bytes(-value)}"
    if value < 1024:
        return f"{int(value)} B"
    units = ("KiB", "MiB", "GiB", "TiB")
    for unit in units:
        value /= 1024
        if value < 1024:
            return f"{value:.1f} {unit}"
    return f"{value:.1f} PiB"


def human_number(value: int | None) -> str:
    """Format an integer with thin thousands separators."""

    if value is None:
        return "?"
    return f"{value:,}"


def human_duration(seconds: float | None) -> str:
    """Format a duration compactly (``0.4s``, ``1m 12s``, ``2h 03m``)."""

    if seconds is None:
        return "?"
    if seconds < 1:
        return f"{seconds * 1000:.0f}ms"
    if seconds < 60:
        return f"{seconds:.1f}s"
    minutes, secs = divmod(int(seconds), 60)
    if minutes < 60:
        return f"{minutes}m {secs:02d}s"
    hours, minutes = divmod(minutes, 60)
    return f"{hours}h {minutes:02d}m"


def parse_size(value: str | int | float) -> int:
    """Parse a human size such as ``5MB``, ``1.5 GiB`` or ``2048`` into bytes.

    Raises:
        ValueError: if the value cannot be interpreted.
    """

    if isinstance(value, bool):
        msg = f"invalid size: {value!r}"
        raise ValueError(msg)
    if isinstance(value, (int, float)):
        if value < 0:
            msg = f"size must not be negative: {value!r}"
            raise ValueError(msg)
        return int(value)

    text = str(value).strip().replace("_", "")
    if not text:
        msg = "empty size value"
        raise ValueError(msg)

    match = re.fullmatch(r"(?i)\s*([0-9]*\.?[0-9]+)\s*([a-z]*)\s*", text)
    if not match:
        msg = f"invalid size: {value!r} (try e.g. 5MB or 1.5GiB)"
        raise ValueError(msg)
    number, unit = match.groups()
    unit = unit.lower()
    if unit not in _SIZE_UNITS:
        msg = f"unknown size unit: {unit!r} (use B, KB, MB, GB, TB or their i-variants)"
        raise ValueError(msg)
    return int(float(number) * _SIZE_UNITS[unit])


def format_timestamp(value: str | int | float | None) -> str:
    """Render a stored ISO-8601 or unix timestamp in a readable local form."""

    if value is None:
        return "unknown"
    if isinstance(value, (int, float)):
        return datetime.fromtimestamp(value, tz=UTC).astimezone().strftime("%Y-%m-%d %H:%M")
    text = str(value)
    try:
        parsed = datetime.fromisoformat(text)
    except ValueError:
        return text
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=UTC)
    return parsed.astimezone().strftime("%Y-%m-%d %H:%M")


# --------------------------------------------------------------------------- #
# Coercion helpers (used by config loading)
# --------------------------------------------------------------------------- #
_TRUE_VALUES = {"1", "true", "yes", "y", "on", "enable", "enabled"}
_FALSE_VALUES = {"0", "false", "no", "n", "off", "disable", "disabled"}


def coerce_bool(value: Any) -> bool:
    """Coerce config/CLI input to a boolean."""

    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return bool(value)
    text = str(value).strip().lower()
    if text in _TRUE_VALUES:
        return True
    if text in _FALSE_VALUES:
        return False
    msg = f"invalid boolean value: {value!r} (use true/false)"
    raise ValueError(msg)


def coerce_int(value: Any, *, minimum: int | None = None, maximum: int | None = None) -> int:
    """Coerce to ``int`` with optional bounds checking."""

    try:
        number = int(str(value).strip())
    except (TypeError, ValueError):
        msg = f"invalid integer value: {value!r}"
        raise ValueError(msg) from None
    if minimum is not None and number < minimum:
        msg = f"value must be >= {minimum} (got {number})"
        raise ValueError(msg)
    if maximum is not None and number > maximum:
        msg = f"value must be <= {maximum} (got {number})"
        raise ValueError(msg)
    return number


def coerce_float(
    value: Any, *, minimum: float | None = None, maximum: float | None = None
) -> float:
    """Coerce to ``float`` with optional bounds checking."""

    try:
        number = float(str(value).strip())
    except (TypeError, ValueError):
        msg = f"invalid numeric value: {value!r}"
        raise ValueError(msg) from None
    if not math.isfinite(number):
        msg = f"value must be a finite number (got {value!r})"
        raise ValueError(msg)
    if minimum is not None and number < minimum:
        msg = f"value must be >= {minimum} (got {number})"
        raise ValueError(msg)
    if maximum is not None and number > maximum:
        msg = f"value must be <= {maximum} (got {number})"
        raise ValueError(msg)
    return number


# --------------------------------------------------------------------------- #
# Names, paths and URLs
# --------------------------------------------------------------------------- #
def _strip_control(value: str) -> str:
    return "".join(char for char in value if unicodedata.category(char) != "Cc")


def sanitize_filename(
    name: str, *, fallback: str = "file", max_length: int = _MAX_COMPONENT_LEN
) -> str:
    """Make ``name`` safe to use as a single filename component.

    Removes directory separators, control characters, reserved Windows device
    names, leading dots and trailing dots/spaces, and truncates long names.
    Path traversal (``, ``, ``, absolute paths) cannot survive this function.
    """

    text = _strip_control(str(name)).replace("\u202e", "").replace("\u200e", "")
    # Defensive: normalise unicode so look-alikes cannot hide separators.
    text = unicodedata.normalize("NFC", text)
    text = _ILLEGAL_FILENAME_CHARS.sub("_", text)
    text = text.replace(os.sep, "_")
    if os.altsep:
        text = text.replace(os.altsep, "_")
    text = text.strip().strip(".").strip()
    text = re.sub(r"\s+", " ", text).strip()

    if not text:
        return fallback
    if text.lower().split(".", 1)[0] in _WINDOWS_RESERVED:
        text = f"_{text}"

    if len(text) > max_length:
        stem, dot, ext = text.rpartition(".")
        if dot and 0 < len(ext) <= 8 and len(stem) > max_length - len(ext) - 1:
            text = f"{stem[: max_length - len(ext) - 1]}.{ext}"
        else:
            text = text[:max_length]

    text = text.strip(". ")
    return text or fallback


def sanitize_path_component(name: str, *, fallback: str = "collection") -> str:
    """Make ``name`` safe as a single directory name (same rules as filenames)."""

    return sanitize_filename(name, fallback=fallback, max_length=64)


def is_http_url(url: str) -> bool:
    """Whether ``url`` is an absolute http(s) URL we are willing to fetch."""

    try:
        parts = urlsplit(str(url))
    except ValueError:
        return False
    return parts.scheme in {"http", "https"} and bool(parts.netloc)


def extension_from_url(url: str, *, default: str | None = None) -> str | None:
    """Extract a plausible file extension from a URL path."""

    if not url:
        return default
    try:
        path = urlsplit(url).path
    except ValueError:
        return default
    name = unquote(path).rsplit("/", 1)[-1]
    _, dot, ext = name.rpartition(".")
    if dot and ext.isalnum() and 0 < len(ext) <= 5:
        return ext.lower()
    return default


def content_type_to_extension(content_type: str | None, *, default: str = "jpg") -> str | None:
    """Map a ``Content-Type`` header onto a file extension."""

    if not content_type:
        return default
    mime = content_type.split(";", 1)[0].strip().lower()
    if mime in _CONTENT_TYPES:
        return _CONTENT_TYPES[mime]
    subtype = mime.rpartition("/")[2]
    subtype = subtype.split("+", 1)[0]
    if subtype.isalnum() and 0 < len(subtype) <= 5:
        return subtype
    return default


def truncate_tags(tags: str, limit: int, *, separator: str = " ") -> str:
    """Truncate a tag string to ``limit`` tags."""

    parts = [part for part in tags.split(separator) if part]
    if len(parts) <= limit:
        return separator.join(parts)
    return separator.join(parts[:limit])


def dedupe_preserve_order(values: Iterable[str]) -> list[str]:
    """Remove duplicates while preserving the first-seen order."""

    seen: set[str] = set()
    result: list[str] = []
    for value in values:
        if value not in seen:
            seen.add(value)
            result.append(value)
    return result


def shuffled(items: Sequence[Any], *, seed: int | None = None) -> list[Any]:
    """Return a shuffled copy of ``items`` (used by ``--shuffle``)."""

    result = list(items)
    random.Random(seed).shuffle(result)
    return result


# --------------------------------------------------------------------------- #
# Tag expressions
# --------------------------------------------------------------------------- #
@dataclass(frozen=True, slots=True)
class TagQuery:
    """A parsed Gelbooru tag expression.

    Gelbooru supports ``tag`` (AND), ``tag1 tag2`` and the negative form
    ``-tag`` / ``!tag`` to exclude a tag.
    """

    include: tuple[str, ...] = ()
    exclude: tuple[str, ...] = ()
    raw: str = ""

    @property
    def tags(self) -> str:
        """The include side as a space separated string."""

        return " ".join(self.include)

    @property
    def search_tags(self) -> str:
        """Include tags plus negated tags, ready for the API."""

        return " ".join([*self.include, *(f"-{tag}" for tag in self.exclude)])

    def __bool__(self) -> bool:
        return bool(self.include or self.exclude)


def parse_tag_expression(text: str | None) -> TagQuery:
    """Split a tag expression into include/exclude lists.

    ``parse_tag_expression("blue_hair solo -nude !blurry")`` yields
    ``TagQuery(include=("blue_hair", "solo"), exclude=("nude", "blurry"))``.
    """

    if not text:
        return TagQuery()
    include: list[str] = []
    exclude: list[str] = []
    for token in str(text).split():
        negative = token[0] in {"-", "!"}
        tag = token[1:] if negative else token
        if not tag:
            continue
        # Metatags (rating:/order:/score:) are valid Gelbooru queries; a negated
        # one must stay negated ("-rating:safe" excludes safe posts).
        if not negative and tag.startswith(("rating:", "order:", "score:")):
            include.append(tag)
            continue
        (exclude if negative else include).append(tag)
    return TagQuery(
        include=tuple(dedupe_preserve_order(include)),
        exclude=tuple(dedupe_preserve_order(exclude)),
        raw=str(text),
    )


# --------------------------------------------------------------------------- #
# Retry helpers
# --------------------------------------------------------------------------- #
@dataclass(slots=True)
class RetryPolicy:
    """Exponential backoff configuration."""

    attempts: int = 3
    base_delay: float = 0.5
    max_delay: float = 30.0
    multiplier: float = 2.0
    jitter: float = 0.1
    max_retry_after: float = 120.0

    def delay_for(self, attempt: int, *, rng: random.Random | None = None) -> float:
        """Delay in seconds before retry number ``attempt`` (1-based)."""

        return compute_backoff(
            attempt,
            base_delay=self.base_delay,
            multiplier=self.multiplier,
            max_delay=self.max_delay,
            jitter=self.jitter,
            rng=rng,
        )


def compute_backoff(
    attempt: int,
    *,
    base_delay: float = 0.5,
    multiplier: float = 2.0,
    max_delay: float = 30.0,
    jitter: float = 0.1,
    rng: random.Random | None = None,
) -> float:
    """Exponential backoff with proportional jitter.

    ``attempt`` is 1-based: attempt 1 waits roughly ``base_delay``.
    """

    attempt = max(attempt, 1)
    raw = base_delay * (multiplier ** (attempt - 1))
    capped = min(raw, max_delay)
    if jitter <= 0:
        return capped
    generator = rng or random
    spread = capped * min(jitter, 1.0)
    return max(0.0, capped + generator.uniform(-spread, spread))


def parse_retry_after(
    value: str | int | float | None, *, now: datetime | None = None
) -> float | None:
    """Parse a ``Retry-After`` header value into seconds.

    Supports both the delta-seconds and the HTTP-date forms. Returns ``None``
    when the value is missing or unparseable.
    """

    if value is None:
        return None
    if isinstance(value, (int, float)):
        seconds = float(value)
        return max(0.0, seconds)

    text = str(value).strip()
    if not text:
        return None
    try:
        return max(0.0, float(text))
    except ValueError:
        pass
    try:
        target = parsedate_to_datetime(text)
    except (TypeError, ValueError):
        return None
    if target is None:
        return None
    if target.tzinfo is None:
        target = target.replace(tzinfo=UTC)
    reference = now or datetime.now(UTC)
    if reference.tzinfo is None:
        reference = reference.replace(tzinfo=UTC)
    delta = (target - reference).total_seconds()
    return max(0.0, delta)


# --------------------------------------------------------------------------- #
# Filesystem helpers
# --------------------------------------------------------------------------- #
def ensure_directory(path: Path) -> Path:
    """Create ``path`` (and parents) if needed and return it."""

    path.mkdir(parents=True, exist_ok=True)
    return path


def atomic_write_bytes(path: Path, data: bytes, *, mode: int = 0o600) -> Path:
    """Write ``data`` to ``path`` atomically, creating parents as needed.

    The file is written to a temporary sibling and then ``os.replace``d, so a
    crash can never leave a half-written file behind.
    """

    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f".{path.name}.tmp{os.getpid()}")
    try:
        with tmp.open("wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        tmp.chmod(mode)
        tmp.replace(path)
    finally:
        if tmp.exists():  # pragma: no cover - only on write failure
            tmp.unlink(missing_ok=True)
    return path


def atomic_write_text(path: Path, text: str, *, mode: int = 0o600) -> Path:
    """Atomically write UTF-8 ``text`` to ``path``."""

    return atomic_write_bytes(path, text.encode("utf-8"), mode=mode)


def sha256_file(path: Path, *, chunk_size: int = 1 << 20) -> str:
    """Compute the SHA-256 of a file's contents."""

    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(chunk_size), b""):
            digest.update(chunk)
    return digest.hexdigest()


def chunks(total: int, size: int = 1 << 16) -> Iterator[int]:
    """Yield offsets ``0, size, 2*size, ...`` for byte-range arithmetic."""

    offset = 0
    while offset < total:
        yield offset
        offset += size
