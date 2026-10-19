"""Client-side filtering of API results.

Gelbooru's dapi supports a handful of server-side filters (``rating:``,
``order:``, ``score:``) but nothing about dimensions, file size or extension.
Those have to be applied here, on the posts the API returned.
"""

from __future__ import annotations

from collections.abc import Iterable, Iterator
from dataclasses import dataclass
from typing import Any

from .models import Post, Rating

__all__ = ["META_TAGS", "PostFilter", "build_filter", "folder_name_for", "matches_all", "sort_key"]

#: Tags that are never used as folder names.
META_TAGS = frozenset(
    {
        "rating",
        "rating_locked",
        "score",
        "fav",
        "favgroup",
        "source",
        "uploaded",
        "creator",
        "copyright",
        "copyright_holder",
        "character",
        "tagme",
    }
)


def is_meta_tag(tag: str) -> bool:
    """Whether ``tag`` is bookkeeping metadata rather than content."""

    return tag in META_TAGS or ":" in tag


@dataclass(frozen=True, slots=True)
class PostFilter:
    """A pure, side-effect free predicate over :class:`~.models.Post`."""

    ratings: frozenset[str] = frozenset()
    exclude_tags: frozenset[str] = frozenset()
    extensions: frozenset[str] = frozenset()
    min_width: int | None = None
    max_width: int | None = None
    min_height: int | None = None
    max_height: int | None = None
    min_pixels: int | None = None
    max_pixels: int | None = None
    min_size: int | None = None
    max_size: int | None = None
    min_score: int | None = None
    require_available: bool = True

    def __bool__(self) -> bool:
        """Whether this filter actually constrains anything."""

        constraints: tuple[Any, ...] = (
            self.ratings,
            self.exclude_tags,
            self.extensions,
            self.min_width,
            self.max_width,
            self.min_height,
            self.max_height,
            self.min_pixels,
            self.max_pixels,
            self.min_size,
            self.max_size,
            self.min_score,
        )
        return any(constraint not in (None, frozenset()) for constraint in constraints) or (
            self.require_available
        )

    def reject_reason(self, post: Post) -> str | None:
        """Return why ``post`` does not match, or ``None`` when it matches."""

        if self.require_available and not post.is_available:
            return post.unavailable_reason or "not available"

        if self.ratings:
            rating = (post.rating or "").strip().lower()
            if rating not in self.ratings:
                return f"rating {rating or '?'} not in {sorted(self.ratings)}"

        if self.exclude_tags:
            post_tags = {tag.lower() for tag in post.tag_list}
            bad = sorted(post_tags & {tag.lower() for tag in self.exclude_tags})
            if bad:
                return f"contains excluded tag(s): {', '.join(bad)}"

        if self.extensions:
            extension = (post.extension or "").lower().lstrip(".")
            if extension not in self.extensions:
                return f"extension {extension or '?'} not in {sorted(self.extensions)}"

        width, height = post.width, post.height
        if width is not None:
            if self.min_width is not None and width < self.min_width:
                return f"width {width} < {self.min_width}"
            if self.max_width is not None and width > self.max_width:
                return f"width {width} > {self.max_width}"
        if height is not None:
            if self.min_height is not None and height < self.min_height:
                return f"height {height} < {self.min_height}"
            if self.max_height is not None and height > self.max_height:
                return f"height {height} > {self.max_height}"
        if width and height:
            pixels = width * height
            if self.min_pixels is not None and pixels < self.min_pixels:
                return f"{width}x{height} below minimum resolution"
            if self.max_pixels is not None and pixels > self.max_pixels:
                return f"{width}x{height} above maximum resolution"

        if self.min_score is not None and (post.score or 0) < self.min_score:
            return f"score {(post.score or 0)} < {self.min_score}"

        size = post.file_size
        if size is not None:
            if self.min_size is not None and size < self.min_size:
                return f"size {size} B < {self.min_size} B"
            if self.max_size is not None and size > self.max_size:
                return f"size {size} B > {self.max_size} B"
        elif self.min_size is not None or self.max_size is not None:
            # Unknown size: cannot prove it is out of range, so keep the post.
            return None

        return None

    def matches(self, post: Post) -> bool:
        return self.reject_reason(post) is None

    def apply(self, posts: Iterable[Post]) -> list[Post]:
        """Return only the posts that match."""

        return [post for post in posts if self.matches(post)]

    def explain(self, posts: Iterable[Post]) -> list[tuple[Post, str]]:
        """Return ``(post, reason)`` pairs for every rejected post."""

        return [(post, reason) for post in posts if (reason := self.reject_reason(post))]

    def filter_with_reasons(
        self, posts: Iterable[Post]
    ) -> tuple[list[Post], list[tuple[Post, str]]]:
        """Split ``posts`` into ``(kept, [(rejected, reason), ...])``."""

        kept: list[Post] = []
        rejected: list[tuple[Post, str]] = []
        for post in posts:
            reason = self.reject_reason(post)
            if reason is None:
                kept.append(post)
            else:
                rejected.append((post, reason))
        return kept, rejected


def _normalise_ratings(ratings: Iterable[Any] | None) -> frozenset[str]:
    if not ratings:
        return frozenset()
    values: set[str] = set()
    for rating in ratings:
        parsed = Rating.parse(rating)
        values.add((parsed.value if parsed else str(rating)).strip().lower())
    values.discard("")
    return frozenset(values)


def _normalise_extensions(extensions: Iterable[str] | None) -> frozenset[str]:
    """Normalise extensions to bare lowercase values (``"jpg"``, not ``".JPG"``)."""

    if not extensions:
        return frozenset()
    values: set[str] = set()
    for extension in extensions:
        cleaned = extension.strip().lower().lstrip(".")
        if cleaned:
            values.add(cleaned)
    return frozenset(values)


def build_filter(
    *,
    ratings: Iterable[Any] | None = None,
    exclude_tags: Iterable[str] | None = None,
    extensions: Iterable[str] | None = None,
    min_width: int | None = None,
    max_width: int | None = None,
    min_height: int | None = None,
    max_height: int | None = None,
    min_pixels: int | None = None,
    max_pixels: int | None = None,
    min_size: int | None = None,
    max_size: int | None = None,
    min_score: int | None = None,
    require_available: bool = True,
) -> PostFilter:
    """Build a :class:`PostFilter` from raw CLI/config values.

    Extensions are normalised to their bare form (``"jpg"``) so they can be
    compared directly against :attr:`Post.extension`.
    """

    return PostFilter(
        ratings=_normalise_ratings(ratings),
        exclude_tags=frozenset(tag.strip().lower() for tag in (exclude_tags or []) if tag.strip()),
        extensions=_normalise_extensions(extensions),
        min_width=min_width,
        max_width=max_width,
        min_height=min_height,
        max_height=max_height,
        min_pixels=min_pixels,
        max_pixels=max_pixels,
        min_size=min_size,
        max_size=max_size,
        min_score=min_score,
        require_available=require_available,
    )


def matches_all(posts: Iterable[Post], post_filter: PostFilter) -> Iterator[Post]:
    """Iterate over ``posts`` keeping only those matching ``post_filter``."""

    for post in posts:
        if post_filter.matches(post):
            yield post


def folder_name_for(post: Post, *, max_tags: int = 2, fallback: str = "untagged") -> str:
    """Derive a folder name from a post's leading content tags.

    Gelbooru metatags (``rating:safe``, ``source:…``, ``score:…``) are skipped so
    folders are named after the artwork, not the database bookkeeping.
    """

    candidates = [tag for tag in post.primary_tags if not is_meta_tag(tag)]
    if not candidates:
        candidates = [tag for tag in post.tag_list if not is_meta_tag(tag)]
    if not candidates or max_tags <= 0:
        return fallback
    return "_".join(candidates[:max_tags])


def sort_key(post: Post, order: str = "") -> tuple[Any, ...]:
    """Sort key implementing Gelbooru's ``order:`` meta-tag semantics."""

    normalized = order.strip().lower()
    id_key = post.id
    if normalized in {"id", "id:desc"}:
        return (id_key,)
    if normalized in {"id:asc", ""}:
        return (-id_key,)
    score = post.score or 0
    width = (post.width or 0) * (post.height or 0)
    if normalized in {"score", "score:desc"}:
        return (-score, -id_key)
    if normalized == "score:asc":
        return (score, -id_key)
    if normalized in {"width", "width:desc"}:
        return (-(post.width or 0), -id_key)
    if normalized == "width:asc":
        return (post.width or 0, -id_key)
    if normalized in {"height", "height:desc"}:
        return (-(post.height or 0), -id_key)
    if normalized == "height:asc":
        return (post.height or 0, -id_key)
    if normalized in {"filesize", "filesize:desc"}:
        return (-(post.file_size or 0), -id_key)
    if normalized == "filesize:asc":
        return (post.file_size or 0, -id_key)
    if normalized in {"random"}:
        # Randomness is applied by the caller (``--shuffle``); keep a stable key.
        return (width,)
    return (-id_key,)
