"""Helpers shared by the CLI commands: tag building, searching, downloading."""

from __future__ import annotations

import asyncio
import json
import logging
from collections.abc import Iterable, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import typer

from ..api import GelbooruClient
from ..app import AppContext, resolve_limit, run_async
from ..console import RichProgressReporter, print_summary
from ..downloader import DownloadOptions
from ..filters import PostFilter, build_filter, sort_key
from ..models import DownloadSummary, Post, PostPage
from ..utils import dedupe_preserve_order, human_bytes, parse_size, parse_tag_expression, shuffled

__all__ = [
    "DownloadRequest",
    "SearchOutcome",
    "build_filter_from_options",
    "build_search_tags",
    "collect_posts",
    "execute_download",
    "export_posts",
    "run_search",
    "search_posts",
    "size_option",
]

log = logging.getLogger("gelbooru.cli")

#: How many API results we are willing to scan per requested match when
#: client-side filters are active.
SCAN_MULTIPLIER = 4
#: Hard cap on pages walked in a single command, to stay polite to the API.
DEFAULT_MAX_PAGES = 50
#: Fallback page size when neither the CLI nor the config provides one.
DEFAULT_PAGE_SIZE = 100


# --------------------------------------------------------------------------- #
# Option parsing helpers
# --------------------------------------------------------------------------- #
def size_option(value: str | None, *, what: str = "size") -> int | None:
    """Typer callback converting ``5MB`` into an integer number of bytes."""

    if value is None:
        return None
    try:
        return parse_size(value)
    except ValueError as exc:
        raise typer.BadParameter(str(exc), param_hint=what) from exc


def split_csv(values: Iterable[str] | None) -> list[str]:
    """Split repeated ``--exclude a,b`` style options into a flat list."""

    result: list[str] = []
    for value in values or ():
        for part in str(value).split(","):
            cleaned = part.strip()
            if cleaned:
                result.append(cleaned.lstrip("-!"))
    return result


def build_search_tags(
    tags: str | None,
    *,
    ratings: Sequence[str] | None = None,
    exclude: Sequence[str] | None = None,
    order: str | None = None,
) -> str:
    """Build the tag expression for a *single* API query.

    ``rating:``/``order:``/``-tag`` are documented Gelbooru metatags, so they
    are pushed server-side; the client-side filter still re-checks the result.

    Only the first rating is used here — Gelbooru ANDs repeated ``rating:``
    tags, so OR-ing several ratings needs one query each (see
    :func:`plan_queries`).
    """

    query = parse_tag_expression(tags)
    parts: list[str] = list(query.include)
    parts.extend(f"-{tag}" for tag in query.exclude)
    for rating in split_csv(ratings)[:1]:
        metatag = f"rating:{rating.lower()}"
        if metatag not in parts:
            parts.append(metatag)
    for tag in split_csv(exclude):
        negated = f"-{tag.lower()}"
        if negated not in parts:
            parts.append(negated)
    if order:
        order_tag = f"order:{order.strip()}"
        if order_tag not in parts:
            parts.append(order_tag)
    return " ".join(parts)


def plan_queries(
    tags: str | None,
    *,
    ratings: Sequence[str] | None = None,
    exclude: Sequence[str] | None = None,
    order: str | None = None,
) -> list[str]:
    """Expand CLI options into the API queries needed to express OR.

    Gelbooru ANDs repeated ``rating:`` metatags (``rating:safe rating:general``
    matches nothing), so ``--rating safe,general`` becomes one query per rating
    and the results are merged and de-duplicated by the caller.

    >>> plan_queries("solo", ratings=["safe,general"])
    ['solo rating:safe', 'solo rating:general']
    >>> plan_queries("solo")
    ['solo']
    """

    rating_values = dedupe_preserve_order(value.lower() for value in split_csv(ratings))
    base = build_search_tags(tags, ratings=None, exclude=exclude, order=order)
    if not rating_values:
        return [base]
    return [f"{base} rating:{rating}".strip() for rating in rating_values]


def build_filter_from_options(
    *,
    ratings: Sequence[str] | None = None,
    exclude: Sequence[str] | None = None,
    extensions: Sequence[str] | None = None,
    min_width: int | None = None,
    max_width: int | None = None,
    min_height: int | None = None,
    max_height: int | None = None,
    min_pixels: int | None = None,
    max_pixels: int | None = None,
    min_size: int | None = None,
    max_size: int | None = None,
    min_score: int | None = None,
) -> PostFilter:
    """Build the client-side :class:`PostFilter` from CLI options."""

    return build_filter(
        ratings=split_csv(ratings),
        exclude_tags=split_csv(exclude),
        extensions=split_csv(extensions),
        min_width=min_width,
        max_width=max_width,
        min_height=min_height,
        max_height=max_height,
        min_pixels=min_pixels,
        max_pixels=max_pixels,
        min_size=min_size,
        max_size=max_size,
        min_score=min_score,
    )


# --------------------------------------------------------------------------- #
# Searching
# --------------------------------------------------------------------------- #
@dataclass(slots=True)
class SearchOutcome:
    """What a search produced, including what was dropped and why."""

    posts: list[Post] = field(default_factory=list)
    fetched: int = 0
    rejected: list[tuple[Post, str]] = field(default_factory=list)
    total_available: int | None = None
    pages: int = 0
    elapsed: float = 0.0
    truncated: bool = False

    @property
    def matched(self) -> int:
        return len(self.posts)

    @property
    def filtered_out(self) -> int:
        return len(self.rejected)


async def collect_posts(
    client: GelbooruClient,
    *,
    tags: str,
    limit: int | None,
    page: int = 1,
    page_size: int | None = None,
    post_filter: PostFilter | None = None,
    order: str | None = None,
    shuffle: bool = False,
    max_pages: int = DEFAULT_MAX_PAGES,
    on_page: Any = None,
) -> SearchOutcome:
    """Walk API pages until ``limit`` matching posts are collected.

    Client-side filters may reject posts, so more than ``limit`` API results are
    scanned (bounded by ``max_pages``) before giving up.
    """

    post_filter = post_filter or PostFilter()
    per_page = max(1, int(page_size or DEFAULT_PAGE_SIZE))
    scan_budget = None if not limit else limit * max(1, SCAN_MULTIPLIER)
    outcome = SearchOutcome()
    started = asyncio.get_running_loop().time()

    def _capture_page(page: PostPage) -> None:
        """Remember how many posts the query matched in total."""

        if outcome.total_available is None and page.total is not None and page.total >= 0:
            outcome.total_available = page.total

    async for post in client.iter_posts(
        tags=tags,
        start_page=max(1, page),
        page_size=per_page,
        max_posts=scan_budget,
        on_page=_capture_page,
    ):
        outcome.fetched += 1
        reason = post_filter.reject_reason(post)
        if reason:
            outcome.rejected.append((post, reason))
        else:
            outcome.posts.append(post)
            if on_page is not None:
                on_page(outcome)
            if limit and len(outcome.posts) >= limit:
                outcome.truncated = True
                break
        if outcome.fetched // per_page >= max_pages:
            outcome.truncated = limit is not None and len(outcome.posts) < limit
            break

    if order and not order.startswith("random"):
        outcome.posts.sort(key=lambda post: sort_key(post, order))
    if shuffle:
        outcome.posts = shuffled(outcome.posts)
    if limit:
        outcome.posts = outcome.posts[:limit]
    outcome.pages = max(1, -(-outcome.fetched // per_page))
    outcome.elapsed = asyncio.get_running_loop().time() - started
    return outcome


async def search_posts(
    app: AppContext,
    *,
    tags: str,
    limit: int | None = None,
    ratings: Sequence[str] | None = None,
    exclude: Sequence[str] | None = None,
    order: str | None = None,
    page: int = 1,
    page_size: int | None = None,
    post_filter: PostFilter | None = None,
    shuffle: bool = False,
    max_pages: int = DEFAULT_MAX_PAGES,
) -> SearchOutcome:
    """Search Gelbooru, expanding OR-ed ratings into one query each.

    ``ratings`` accepts comma separated values (``"safe,general"``); each one
    becomes its own API request and the results are merged in order, de-duplicated
    by post id, then limited.
    """

    plans = plan_queries(tags, ratings=ratings, exclude=exclude, order=order)
    if len(plans) == 1:
        async with app.client() as client:
            return await collect_posts(
                client,
                tags=plans[0],
                limit=limit,
                page=page,
                page_size=page_size,
                post_filter=post_filter,
                order=order,
                shuffle=shuffle,
                max_pages=max_pages,
            )

    async with app.client() as client:
        outcomes = await asyncio.gather(
            *(
                collect_posts(
                    client,
                    tags=plan,
                    limit=limit,
                    page=page,
                    page_size=page_size,
                    post_filter=post_filter,
                    order=order,
                    shuffle=False,
                    max_pages=max_pages,
                )
                for plan in plans
            )
        )

    merged = SearchOutcome()
    seen: set[int] = set()
    totals: list[int] = []
    for outcome in outcomes:
        merged.fetched += outcome.fetched
        merged.rejected.extend(outcome.rejected)
        merged.pages += outcome.pages
        if outcome.total_available is not None:
            totals.append(outcome.total_available)
        for post in outcome.posts:
            if post.id in seen:
                continue
            seen.add(post.id)
            merged.posts.append(post)

    # Post counts per rating are disjoint, so the sum is the OR total.
    merged.total_available = sum(totals) if totals else None
    if order and not order.startswith("random"):
        merged.posts.sort(key=lambda post: sort_key(post, order))
    if shuffle:
        merged.posts = shuffled(merged.posts)
    if limit:
        merged.truncated = len(merged.posts) > limit
        merged.posts = merged.posts[:limit]
    merged.elapsed = max((outcome.elapsed for outcome in outcomes), default=0.0)
    return merged


def run_search(app: AppContext, **kwargs: Any) -> SearchOutcome:
    """Synchronous wrapper around :func:`search_posts` (used by the CLI)."""

    return run_async(search_posts(app, **kwargs))


# --------------------------------------------------------------------------- #
# Downloading
# --------------------------------------------------------------------------- #
@dataclass(slots=True)
class DownloadRequest:
    """Every knob the ``download``-style commands accept.

    Keeping them in one dataclass lets ``download``, ``batch``, ``presets
    download`` and ``watch`` share a single code path.
    """

    limit: int | None = None
    page: int = 1
    page_size: int | None = None
    output: Path | None = None
    concurrency: int | None = None
    retries: int | None = None
    timeout: float | None = None
    skip_existing: bool | None = None
    overwrite: bool = False
    dry_run: bool = False
    duplicate_check: bool = False
    shuffle: bool = False
    archive: bool | None = None
    collection: str | None = None
    folder_by_tags: bool = False
    filename_template: str | None = None
    ratings: Sequence[str] = ()
    exclude: Sequence[str] = ()
    extensions: Sequence[str] = ()
    min_width: int | None = None
    max_width: int | None = None
    min_height: int | None = None
    max_height: int | None = None
    min_size: int | None = None
    max_size: int | None = None
    min_score: int | None = None
    order: str | None = None
    max_pages: int = DEFAULT_MAX_PAGES

    def post_filter(self) -> PostFilter:
        return build_filter_from_options(
            ratings=self.ratings,
            exclude=self.exclude,
            extensions=self.extensions,
            min_width=self.min_width,
            max_width=self.max_width,
            min_height=self.min_height,
            max_height=self.max_height,
            min_size=self.min_size,
            max_size=self.max_size,
            min_score=self.min_score,
        )


def download_options(app: AppContext, request: DownloadRequest) -> DownloadOptions:
    """Translate a :class:`DownloadRequest` into :class:`DownloadOptions`."""

    return DownloadOptions.from_config(
        app.config,
        output_dir=request.output,
        concurrency=request.concurrency,
        retries=request.retries,
        timeout=request.timeout,
        skip_existing=request.skip_existing,
        overwrite=True if request.overwrite else None,
        dry_run=True if request.dry_run else None,
        duplicate_check=True if request.duplicate_check else None,
        archive_metadata=True if request.archive else None,
        folder=request.collection,
        folder_by_tags=True if request.folder_by_tags else None,
        filename_template=request.filename_template,
        min_size=request.min_size,
        max_size=request.max_size,
    )


async def download_posts(
    app: AppContext,
    posts: Sequence[Post],
    request: DownloadRequest,
    *,
    title: str = "Downloading",
    show_progress: bool | None = None,
) -> DownloadSummary:
    """Download ``posts`` with a live progress bar and a final report."""

    options = download_options(app, request)
    want_progress = app.config.display.show_progress if show_progress is None else show_progress
    reporter = RichProgressReporter(
        app.console,
        enabled=want_progress and not request.dry_run,
        transient=True,
        title=title,
    )
    with reporter:
        async with app.downloader_with_options(options, reporter=reporter) as downloader:
            summary = await downloader.download_many(posts, title=title, reporter=reporter)
    return summary


def execute_download(
    app: AppContext,
    tags: str,
    request: DownloadRequest,
    *,
    title: str = "Downloading",
) -> DownloadSummary:
    """Search for ``tags`` and download everything that matches."""

    limit = resolve_limit(request.limit, app.config)
    outcome = run_search(
        app,
        tags=tags,
        limit=limit,
        ratings=request.ratings,
        exclude=request.exclude,
        order=request.order,
        page=request.page,
        page_size=request.page_size or app.config.general.page_size,
        post_filter=request.post_filter(),
        shuffle=request.shuffle,
        max_pages=request.max_pages,
    )
    queries = plan_queries(
        tags, ratings=request.ratings, exclude=request.exclude, order=request.order
    )
    if not outcome.posts:
        from ..console import warn

        detail = ""
        if outcome.fetched == 0:
            detail = " — the API returned no posts for this query"
        elif outcome.filtered_out:
            detail = f" — {outcome.filtered_out} result(s) were filtered out"
        warn(
            app.console,
            f"No posts to download for '{' | '.join(queries) or '(empty query)'}'{detail}",
        )
        return DownloadSummary()

    summary = run_async(download_posts(app, outcome.posts, request, title=title))
    print_summary(app.console, summary, output_dir=download_options(app, request).output_dir)
    _report_filtered(app, outcome)
    return summary


def _report_filtered(app: AppContext, outcome: SearchOutcome) -> None:
    if not outcome.rejected:
        return
    from ..console import warn

    counts: dict[str, int] = {}
    for _post, reason in outcome.rejected:
        key = reason.split(":")[0].split(" not in")[0].strip()
        counts[key] = counts.get(key, 0) + 1
    top = ", ".join(
        f"{reason} ×{count}" for reason, count in sorted(counts.items(), key=lambda kv: -kv[1])[:3]
    )
    warn(app.console, f"filtered out {len(outcome.rejected)} post(s): {top}")


def print_collect_preview(
    app: AppContext,
    outcome: SearchOutcome,
    *,
    options: DownloadOptions | None = None,
    limit: int = 20,
) -> None:
    """Show which posts a download would fetch (``--dry-run``)."""

    from ..console import render_results_preview
    from ..downloader import DownloadResult, DownloadStatus, target_path

    resolved = options or DownloadOptions.from_config(app.config)
    rows = [
        DownloadResult(
            post_id=post.id,
            status=DownloadStatus.DRY_RUN,
            path=str(target_path(resolved, post, index=index)),
            size=post.file_size or 0,
        )
        for index, post in enumerate(outcome.posts, 1)
    ]
    app.console.print(render_results_preview(rows, max_rows=limit))


# --------------------------------------------------------------------------- #
# Export
# --------------------------------------------------------------------------- #
CSV_COLUMNS = (
    "id",
    "rating",
    "width",
    "height",
    "file_size",
    "type",
    "score",
    "md5",
    "file_url",
    "source",
    "created_at",
    "available",
    "tags",
)


def export_posts(posts: Sequence[Post], destination: Path, fmt: str = "json") -> Path:
    """Write search results to ``destination`` as JSON or CSV."""

    destination = Path(destination).expanduser()
    destination.parent.mkdir(parents=True, exist_ok=True)
    normalized = fmt.strip().lower()

    if normalized == "json":
        payload = {
            "count": len(posts),
            "posts": [post.to_dict() for post in posts],
        }
        destination.write_text(json.dumps(payload, indent=2, ensure_ascii=False), encoding="utf-8")
        return destination

    if normalized == "csv":
        import csv

        with destination.open("w", encoding="utf-8", newline="") as handle:
            writer = csv.writer(handle)
            writer.writerow(CSV_COLUMNS)
            for post in posts:
                row = post.to_dict()
                writer.writerow([row.get(column, "") for column in CSV_COLUMNS])
        return destination

    msg = f"unsupported export format {fmt!r} (use json or csv)"
    raise ValueError(msg)


def summarize_sizes(posts: Sequence[Post]) -> str:
    """Total known size of a result set, for the search footer."""

    known = [post.file_size for post in posts if post.file_size]
    if not known:
        return "unknown size"
    return human_bytes(sum(known))
