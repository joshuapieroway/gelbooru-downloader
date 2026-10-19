"""Structured data models for posts, pages, and download results.

The models are deliberately defensive: Gelbooru omits fields for deleted posts,
non-image uploads and old records, so every field except ``id`` is optional and
coerced on the way in. Nothing is ever invented — if the API did not give us a
``file_url`` the post is simply reported as unavailable.
"""

from __future__ import annotations

import html
import math
from collections.abc import Iterable, Iterator, Sequence
from datetime import UTC, datetime
from enum import StrEnum
from typing import Any

from pydantic import BaseModel, ConfigDict, field_validator

__all__ = [
    "DownloadResult",
    "DownloadStatus",
    "DownloadSummary",
    "HistoryEntry",
    "HistoryStats",
    "Post",
    "PostPage",
    "Rating",
    "media_extensions",
]

#: Extensions Gelbooru serves, mapped to the general media "kind".
_KIND_BY_EXT = {
    "jpg": "image",
    "jpeg": "image",
    "jxl": "image",
    "png": "image",
    "gif": "image",
    "webp": "image",
    "webm": "video",
    "mp4": "video",
}


class Rating(StrEnum):
    """Content ratings as reported by Gelbooru."""

    SAFE = "safe"
    QUESTIONABLE = "questionable"
    SENSITIVE = "sensitive"
    EXPLICIT = "explicit"
    UNRATED = "unrated"
    #: Gelbooru returns "general" for a large share of posts; treat it as its own
    #: rating instead of an unknown value so ``--rating general`` works.
    GENERAL = "general"

    @classmethod
    def parse(cls, value: object) -> Rating | None:
        """Parse a rating from arbitrary API input, returning ``None`` if unknown."""

        if isinstance(value, Rating):
            return value
        if not isinstance(value, str):
            return None
        try:
            return cls(value.strip().lower())
        except ValueError:
            return None

    @classmethod
    def values(cls) -> list[str]:
        return [rating.value for rating in cls]


class DownloadStatus(StrEnum):
    """Terminal state of a single post's download."""

    COMPLETED = "completed"
    SKIPPED = "skipped"
    FAILED = "failed"
    UNAVAILABLE = "unavailable"
    DRY_RUN = "dry-run"
    DUPLICATE = "duplicate"
    PENDING = "pending"

    @property
    def is_success(self) -> bool:
        return self in {DownloadStatus.COMPLETED, DownloadStatus.SKIPPED, DownloadStatus.DUPLICATE}


def _as_int(value: Any) -> int | None:
    """Coerce API values to ``int`` without raising."""

    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        return value
    if isinstance(value, float):
        return int(value) if math.isfinite(value) else None
    if isinstance(value, str):
        text = value.strip()
        if not text:
            return None
        try:
            return int(text)
        except ValueError:
            try:
                return int(float(text))
            except ValueError:
                return None
    return None


def _as_str(value: Any) -> str | None:
    """Coerce API values to a non-empty ``str``, or ``None``."""

    if value is None or isinstance(value, bool):
        return None
    if isinstance(value, str):
        text = value.strip()
        return text or None
    if isinstance(value, (int, float)):
        return str(value)
    return None


def _as_tags(value: Any) -> str:
    """Normalise the ``tags`` field into a space separated string.

    Gelbooru HTML-escapes tag names inside its JSON payload (``>3<`` arrives as
    ``&gt;3&lt;``), so the text is unescaped once here.
    """

    text = _as_str(value)
    if text is None:
        return ""
    if "&" in text:
        text = html.unescape(text)
    return " ".join(text.split())


#: Gelbooru returns ``created_at`` as a human readable string in JSON responses
#: (e.g. "Fri Oct 16 18:15:34 -0500 2026"); XML responses use unix seconds.
_TIMESTAMP_FORMATS = (
    "%a %b %d %H:%M:%S %z %Y",
    "%a %b %d %H:%M:%S %Y",
    "%Y-%m-%d %H:%M:%S %z",
    "%Y-%m-%dT%H:%M:%S%z",
    "%Y-%m-%d %H:%M:%S",
)


def _as_timestamp(value: Any) -> int | None:
    """Coerce ``created_at`` into unix seconds, accepting Gelbooru's date string."""

    if isinstance(value, bool):
        return None
    if isinstance(value, int | float):
        return int(value)
    text = _as_str(value)
    if text is None:
        return None
    if text.isdigit():
        return int(text)
    for fmt in _TIMESTAMP_FORMATS:
        try:
            return int(datetime.strptime(text, fmt).timestamp())
        except ValueError:
            continue
    return None


class Post(BaseModel):
    """A single Gelbooru post as returned by ``/index.php?page=dapi``.

    Only ``id`` is mandatory; everything else is optional because Gelbooru
    returns partial records for deleted, held or pending posts.
    """

    model_config = ConfigDict(frozen=True, extra="ignore", populate_by_name=True)

    id: int
    md5: str | None = None
    creator_id: int | None = None
    file_name: str | None = None
    file_url: str | None = None
    file_size: int | None = None
    width: int | None = None
    height: int | None = None
    source: str | None = None
    preview_url: str | None = None
    rating: str | None = None
    score: int | None = None
    total_score: int | None = None
    up_score: int | None = None
    down_score: int | None = None
    comment_count: int | None = None
    tags: str = ""
    created_at: int | None = None
    status: str | None = None
    type: str | None = None
    is_pending: bool | None = None
    is_held: bool | None = None

    @field_validator("tags", mode="before")
    @classmethod
    def _coerce_tags(cls, value: Any) -> str:
        return _as_tags(value)

    @classmethod
    def from_api(cls, raw: Any) -> Post:
        """Build a :class:`Post` from a raw API record.

        Raises:
            ValueError: if the record carries no usable numeric ``id``.
        """

        if not isinstance(raw, dict):
            msg = f"expected a JSON object for a post, got {type(raw).__name__}"
            raise ValueError(msg)

        # Gelbooru returns every field as a string; coerce the ones we model on.
        coerced: dict[str, Any] = {}
        int_fields = (
            "id",
            "creator_id",
            "file_size",
            "width",
            "height",
            "score",
            "total_score",
            "up_score",
            "down_score",
            "comment_count",
        )
        str_fields = (
            "md5",
            "file_name",
            "file_url",
            "source",
            "preview_url",
            "rating",
            "status",
            "type",
            "tags",
        )

        for key in int_fields:
            if key in raw:
                coerced[key] = _as_int(raw[key])
        for key in str_fields:
            if key in raw:
                coerced[key] = _as_str(raw[key])
        if "created_at" in raw:
            coerced["created_at"] = _as_timestamp(raw["created_at"])
        for key in ("is_pending", "is_held"):
            if key in raw:
                raw_value = raw[key]
                coerced[key] = (
                    raw_value if isinstance(raw_value, bool) else bool(_as_int(raw_value))
                )

        post_id = coerced.get("id")
        if post_id is None:
            msg = f"post record without a valid id: {raw!r:.160}"
            raise ValueError(msg)
        return cls.model_validate(coerced)

    @property
    def rating_value(self) -> Rating | None:
        return Rating.parse(self.rating)

    @property
    def extension(self) -> str | None:
        """Best-effort file extension for this post."""

        if self.type:
            ext = self.type.strip().lower().lstrip(".")
            if ext.isalnum() and 0 < len(ext) <= 5:
                return ext
        if self.file_name:
            _, _, ext = self.file_name.rpartition(".")
            if ext and ext.isalnum() and len(ext) <= 5:
                return ext.lower()
        if self.file_url:
            path = self.file_url.split("?", 1)[0].split("#", 1)[0]
            _, _, ext = path.rpartition(".")
            if ext and ext.isalnum() and len(ext) <= 5:
                return ext.lower()
        return None

    @property
    def media_kind(self) -> str:
        ext = (self.extension or "").lower()
        return _KIND_BY_EXT.get(ext, "unknown")

    @property
    def tag_list(self) -> list[str]:
        return [tag for tag in self.tags.split(" ") if tag]

    @property
    def primary_tags(self) -> list[str]:
        """The artist-ish tags Gelbooru puts first: everything except meta tags."""

        ignored = {
            "tagme",
            "rating",
            "rating_locked",
            "score",
            "fav",
            "favgroup",
            "source",
            "uploaded",
            "creator",
            "copyright",
            "character",
            "copyright_holder",
        }
        return [tag for tag in self.tag_list if tag not in ignored]

    @property
    def is_available(self) -> bool:
        """Whether the original file can plausibly be downloaded.

        A post is unavailable when Gelbooru withheld the URL, marked it
        ``deleted``/``held``, or reported a zero dimension. We never guess a URL.
        """

        if not self.file_url:
            return False
        if (self.status or "active").strip().lower() in {"deleted", "held", "pending"}:
            return False
        if self.is_held or self.is_pending:
            return False
        return self.width is None or self.width > 0

    @property
    def unavailable_reason(self) -> str | None:
        """Human readable reason why the post cannot be downloaded."""

        if self.is_available:
            return None
        if not self.file_url:
            return "no file URL supplied by the API"
        if self.is_held:
            return "post is held"
        if self.is_pending:
            return "post is pending approval"
        if (self.status or "").strip().lower() == "deleted":
            return "post is deleted"
        if self.width is not None and self.width <= 0:
            return "post reports no dimensions (likely removed file)"
        return "file is not downloadable"

    @property
    def created_at_dt(self) -> datetime | None:
        """``created_at`` (unix seconds) as a timezone aware datetime."""

        if self.created_at is None:
            return None
        try:
            return datetime.fromtimestamp(self.created_at, tz=UTC)
        except (OverflowError, OSError, ValueError):
            return None

    def to_dict(self) -> dict[str, Any]:
        """Serialise for JSON/CSV export."""

        return {
            "id": self.id,
            "md5": self.md5,
            "creator_id": self.creator_id,
            "file_name": self.file_name,
            "file_url": self.file_url,
            "file_size": self.file_size,
            "width": self.width,
            "height": self.height,
            "source": self.source,
            "preview_url": self.preview_url,
            "rating": self.rating,
            "score": self.score,
            "comment_count": self.comment_count,
            "tags": self.tags,
            "created_at": self.created_at,
            "status": self.status,
            "type": self.type,
            "available": self.is_available,
        }

    def summary(self, tag_count: int = 6) -> str:
        """Short one-line description used in tables."""

        tags = self.primary_tags[:tag_count]
        if len(self.primary_tags) > tag_count:
            tags = [*tags, "…"]
        return " ".join(tags) if tags else "(no tags)"


class PostPage(BaseModel):
    """One page of search results."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    posts: tuple[Post, ...] = ()
    #: Total number of posts matching the query, as reported by the API (``-1`` when unknown).
    total: int | None = None
    #: Offset of the first post in this page, as reported by the API.
    offset: int | None = None
    page: int = 1
    per_page: int = 0

    @property
    def count(self) -> int:
        return len(self.posts)

    @property
    def has_more(self) -> bool:
        """Whether more posts exist beyond this page.

        Gelbooru's ``offset`` attribute is not dependable (it echoes the last
        ``pid``), so the page/per-page pair recorded when the page was fetched is
        what decides this. When the total is unknown (``-1``) a full page implies
        there may be more.
        """

        if self.total is None or self.total < 0:
            return self.count >= self.per_page > 0
        start = max(0, self.page - 1) * self.per_page
        return start + self.count < self.total

    @property
    def total_pages(self) -> int | None:
        if self.total is None or self.total < 0 or self.per_page <= 0:
            return None
        return math.ceil(self.total / self.per_page)

    def __iter__(self) -> Iterator[Post]:  # type: ignore[override]
        return iter(self.posts)

    def __len__(self) -> int:
        return len(self.posts)


class DownloadResult(BaseModel):
    """Outcome of downloading (or trying to download) a single post."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    post_id: int
    status: DownloadStatus
    path: str | None = None
    url: str | None = None
    size: int = 0
    attempts: int = 0
    error: str | None = None
    sha256: str | None = None
    tags: str = ""

    @property
    def ok(self) -> bool:
        return self.status.is_success or self.status is DownloadStatus.DRY_RUN

    def describe(self) -> str:
        if self.status is DownloadStatus.COMPLETED and self.path:
            return f"{self.post_id} -> {self.path}"
        if self.status is DownloadStatus.SKIPPED:
            return f"{self.post_id} already downloaded"
        if self.status is DownloadStatus.DUPLICATE:
            return f"{self.post_id} duplicate of an existing file"
        if self.status is DownloadStatus.DRY_RUN:
            return f"{self.post_id} would be downloaded"
        return f"{self.post_id} failed: {self.error or 'unknown error'}"


class DownloadSummary(BaseModel):
    """Aggregate result of a download batch."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    results: tuple[DownloadResult, ...] = ()
    elapsed: float = 0.0

    def _count(self, status: DownloadStatus) -> int:
        return sum(1 for result in self.results if result.status is status)

    @property
    def completed(self) -> int:
        return self._count(DownloadStatus.COMPLETED)

    @property
    def skipped(self) -> int:
        return self._count(DownloadStatus.SKIPPED)

    @property
    def duplicates(self) -> int:
        return self._count(DownloadStatus.DUPLICATE)

    @property
    def failed(self) -> int:
        return self._count(DownloadStatus.FAILED)

    @property
    def unavailable(self) -> int:
        return self._count(DownloadStatus.UNAVAILABLE)

    @property
    def dry_run(self) -> int:
        return self._count(DownloadStatus.DRY_RUN)

    @property
    def total(self) -> int:
        return len(self.results)

    @property
    def bytes_downloaded(self) -> int:
        return sum(
            result.size for result in self.results if result.status is DownloadStatus.COMPLETED
        )

    @property
    def remaining(self) -> int:
        return self.failed + self.unavailable

    @property
    def failures(self) -> list[DownloadResult]:
        return [
            r
            for r in self.results
            if r.status in {DownloadStatus.FAILED, DownloadStatus.UNAVAILABLE}
        ]

    def extend(self, other: DownloadSummary) -> DownloadSummary:
        return DownloadSummary(
            results=(*self.results, *other.results), elapsed=self.elapsed + other.elapsed
        )


class HistoryEntry(BaseModel):
    """A row of the SQLite download history."""

    model_config = ConfigDict(frozen=True, extra="forbid", from_attributes=True)

    post_id: int
    url: str | None = None
    path: str | None = None
    timestamp: str | None = None
    file_size: int = 0
    status: str = DownloadStatus.COMPLETED.value
    tags: str = ""
    sha256: str | None = None
    attempts: int = 0
    error: str | None = None


class HistoryStats(BaseModel):
    """Aggregate download history statistics."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    total: int = 0
    completed: int = 0
    skipped: int = 0
    failed: int = 0
    unavailable: int = 0
    duplicates: int = 0
    bytes_downloaded: int = 0
    first_download: str | None = None
    last_download: str | None = None

    @classmethod
    def from_rows(cls, rows: Iterable[Sequence[Any]]) -> HistoryStats:
        """Aggregate ``(status, size, timestamp)`` rows into statistics."""

        counters: dict[str, int] = {status.value: 0 for status in DownloadStatus}
        total = 0
        completed_bytes = 0
        timestamps: list[str] = []
        for row in rows:
            status = str(row[0])
            size = _as_int(row[1]) or 0
            timestamp = _as_str(row[2])
            total += 1
            if status in counters:
                counters[status] += 1
            if status == DownloadStatus.COMPLETED.value:
                completed_bytes += size
            if timestamp:
                timestamps.append(timestamp)
        return cls(
            total=total,
            completed=counters[DownloadStatus.COMPLETED.value],
            skipped=counters[DownloadStatus.SKIPPED.value],
            failed=counters[DownloadStatus.FAILED.value],
            unavailable=counters[DownloadStatus.UNAVAILABLE.value],
            duplicates=counters[DownloadStatus.DUPLICATE.value],
            bytes_downloaded=completed_bytes,
            first_download=min(timestamps) if timestamps else None,
            last_download=max(timestamps) if timestamps else None,
        )

    def merge(self, other: HistoryStats) -> HistoryStats:
        return HistoryStats(
            total=self.total + other.total,
            completed=self.completed + other.completed,
            skipped=self.skipped + other.skipped,
            failed=self.failed + other.failed,
            unavailable=self.unavailable + other.unavailable,
            duplicates=self.duplicates + other.duplicates,
            bytes_downloaded=self.bytes_downloaded + other.bytes_downloaded,
            first_download=min(x for x in (self.first_download, other.first_download) if x)
            if self.first_download and other.first_download
            else (self.first_download or other.first_download),
            last_download=max(x for x in (self.last_download, other.last_download) if x)
            if self.last_download and other.last_download
            else (self.last_download or other.last_download),
        )


def media_extensions() -> set[str]:
    """Extensions Gelbooru is known to serve."""

    return set(_KIND_BY_EXT)


def dedupe_posts(posts: Iterable[Post]) -> list[Post]:
    """Remove duplicate post ids, preserving order."""

    seen: set[int] = set()
    unique: list[Post] = []
    for post in posts:
        if post.id in seen:
            continue
        seen.add(post.id)
        unique.append(post)
    return unique


def utc_now_iso() -> str:
    """Current UTC timestamp in ISO-8601 form (used for history rows)."""

    return datetime.now(UTC).isoformat(timespec="seconds")
