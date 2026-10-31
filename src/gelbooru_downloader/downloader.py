"""Asynchronous, bounded-concurrency image downloading.

Design notes:

* files are streamed to ``<name>.part`` and then renamed into place, so an
  interrupted run never leaves a truncated image behind that would look done;
* resumable downloads use HTTP ``Range`` requests and restart cleanly when the
  server ignores them;
* failures are per-file: one bad post never aborts the batch;
* every outcome (completed / skipped / failed / unavailable / dry-run) is
  reported through a progress callback and recorded in the history database.
"""

from __future__ import annotations

import asyncio
import contextlib
import hashlib
import logging
import os
import random
from collections.abc import Iterable, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Protocol
from urllib.parse import urlsplit

import httpx

from .config import Config
from .database import HistoryDatabase
from .exceptions import (
    ApiError,
    ContentRejectedError,
    DownloadError,
    DownloadUnavailableError,
    HistoryError,
)
from .filters import folder_name_for
from .models import DownloadResult, DownloadStatus, DownloadSummary, Post, dedupe_posts, utc_now_iso
from .utils import (
    RetryPolicy,
    content_type_to_extension,
    ensure_directory,
    extension_from_url,
    human_bytes,
    parse_retry_after,
    sanitize_filename,
    sanitize_path_component,
)

__all__ = [
    "DownloadEvent",
    "DownloadOptions",
    "DownloadRateLimitError",
    "Downloader",
    "ProgressReporter",
    "target_path",
]

log = logging.getLogger("gelbooru.download")

HTTP_OK = 200
HTTP_PARTIAL = 206
HTTP_NOT_FOUND = 404
HTTP_GONE = 410
HTTP_TOO_MANY_REQUESTS = 429
HTTP_SERVER_ERROR = 500
CHUNK_SIZE = 1 << 16

_RETRYABLE_STATUS = frozenset({HTTP_TOO_MANY_REQUESTS, HTTP_SERVER_ERROR, 502, 503, 504, 507, 509})

#: Content types we are willing to store. Anything else is probably an error page.
_ACCEPTED_PREFIXES = ("image/", "video/")
_ACCEPTED_EXACT = frozenset({"application/octet-stream", "binary/octet-stream"})

#: Leading bytes that indicate an HTML document rather than an image.
_HTML_SIGNATURES = (b"<!doctype html", b"<html", b"<?xml", b"<error")


class ProgressReporter(Protocol):
    """Callback surface used to drive a live progress display."""

    def begin(self, total: int, *, title: str) -> None:
        """Announce that ``total`` downloads are about to start."""

    def event(self, event: DownloadEvent) -> None:
        """Report the completion of a single post."""

    def finish(self) -> None:
        """Signal that no further events will be emitted."""


@dataclass(frozen=True, slots=True)
class DownloadEvent:
    """A single progress event emitted by the downloader."""

    post_id: int
    status: DownloadStatus
    size: int = 0
    path: Path | None = None
    message: str | None = None
    remaining: int = 0


class DownloadRateLimitError(DownloadError):
    """A 429 received while fetching a file (retryable, honours ``Retry-After``)."""

    def __init__(
        self,
        message: str,
        *,
        post_id: int | None = None,
        url: str | None = None,
        retry_after: float | None = None,
    ) -> None:
        super().__init__(message, post_id=post_id, url=url)
        self.retry_after = retry_after


@dataclass(slots=True)
class DownloadOptions:
    """Everything that controls how files are fetched and stored."""

    output_dir: Path
    concurrency: int = 5
    retries: int = 3
    timeout: float = 30.0
    skip_existing: bool = True
    overwrite: bool = False
    dry_run: bool = False
    duplicate_check: bool = False
    resume: bool = True
    archive_metadata: bool = False
    filename_template: str = "{id}"
    folder: str | None = None
    folder_by_tags: bool = False
    folder_tag_count: int = 2
    min_size: int | None = None
    max_size: int | None = None
    user_agent: str = "GelbooruCLI/1.0"
    #: Sent only for gelbooru.com hosts, which hotlink-protect their CDN.
    referer: str | None = None
    accepted_prefixes: tuple[str, ...] = _ACCEPTED_PREFIXES
    verify_content: bool = True

    @classmethod
    def from_config(cls, config: Config, **overrides: Any) -> DownloadOptions:
        """Build options from the application configuration plus CLI overrides."""

        general = config.general
        data: dict[str, Any] = {
            "output_dir": general.output_dir,
            "concurrency": general.concurrency,
            "retries": general.retries,
            "timeout": general.timeout,
            "skip_existing": general.skip_existing,
            "archive_metadata": general.archive_metadata,
            "filename_template": general.filename_template,
            "user_agent": config.network.user_agent,
            "referer": config.network.referer,
        }
        data.update({key: value for key, value in overrides.items() if value is not None})
        return cls(**data)

    @property
    def retry_policy(self) -> RetryPolicy:
        return RetryPolicy(attempts=self.retries + 1)


def target_path(
    options: DownloadOptions,
    post: Post,
    *,
    extension: str | None = None,
    index: int | None = None,
) -> Path:
    """Resolve the destination path for ``post`` under ``options.output_dir``.

    Pure function: safe to call for previews and ``--dry-run`` without opening a
    network client. Every component is sanitised and the result is verified to
    stay inside the output directory.
    """

    ext = (extension or post.extension or extension_from_url(post.file_url or "") or "jpg").lower()
    ext = ext.lstrip(".") or "jpg"
    # First few tags joined by underscores for filename use
    tags_slug = "_".join(post.tag_list[:5]) if post.tag_list else ""
    stem_fields = {
        "id": str(post.id),
        "md5": post.md5 or "",
        "rating": post.rating or "unrated",
        "index": f"{index:04d}" if index is not None else "",
        "score": str(post.score) if post.score is not None else "",
        "tags": tags_slug,
    }
    try:
        stem = options.filename_template.format(**stem_fields)
    except (KeyError, IndexError, ValueError) as exc:
        msg = (
            f"invalid filename_template {options.filename_template!r}: {exc}. "
            "Available fields: {id}, {md5}, {rating}, {index}, {score}, {tags}"
        )
        raise DownloadError(msg, post_id=post.id) from exc

    name = sanitize_filename(f"{stem}.{ext}", fallback=str(post.id))
    directory = options.output_dir.expanduser()
    if options.folder:
        directory = directory / sanitize_path_component(options.folder, fallback="collection")
    if options.folder_by_tags:
        directory = directory / sanitize_path_component(
            folder_name_for(post, max_tags=options.folder_tag_count), fallback="untagged"
        )

    root = options.output_dir.expanduser().resolve()
    candidate = (directory / name).resolve()
    if not candidate.is_relative_to(root):
        msg = f"refusing to write outside the output directory: {candidate}"
        raise DownloadError(msg)
    return directory / name


class Downloader:
    """Downloads posts concurrently, safely and idempotently."""

    def __init__(
        self,
        options: DownloadOptions,
        *,
        history: HistoryDatabase | None = None,
        transport: httpx.AsyncBaseTransport | None = None,
        client: httpx.AsyncClient | None = None,
        sleep: Any = asyncio.sleep,
        reporter: ProgressReporter | None = None,
    ) -> None:
        self.options = options
        self.history = history
        self.retry_policy = options.retry_policy
        self._sleep = sleep
        self._reporter = reporter
        self._owns_client = client is None
        self._client = client or httpx.AsyncClient(
            timeout=httpx.Timeout(options.timeout),
            follow_redirects=True,
            transport=transport,
            headers={
                "User-Agent": options.user_agent,
                "Accept": "image/*,video/*;q=0.9,*/*;q=0.8",
            },
        )
        self._semaphore = asyncio.Semaphore(max(1, options.concurrency))
        self._total = 0
        self._done = 0

    # -- lifecycle --------------------------------------------------------- #
    async def __aenter__(self) -> Downloader:
        return self

    async def __aexit__(self, *exc_info: object) -> None:
        await self.aclose()

    async def aclose(self) -> None:
        if self._owns_client:
            await self._client.aclose()

    # -- paths ------------------------------------------------------------- #
    def output_root(self) -> Path:
        return Path(self.options.output_dir).expanduser()

    def directory_for(self, post: Post) -> Path:
        """Resolve the (sanitised) directory a post should be stored in."""

        return target_path(self.options, post).parent

    def target_for(
        self, post: Post, *, extension: str | None = None, index: int | None = None
    ) -> Path:
        """Resolve the absolute destination path for ``post``."""

        return target_path(self.options, post, extension=extension, index=index)

    # -- public API -------------------------------------------------------- #
    async def download_many(
        self,
        posts: Iterable[Post],
        *,
        title: str = "Downloading",
        reporter: ProgressReporter | None = None,
    ) -> DownloadSummary:
        """Download every post concurrently, honouring the configured limit.

        Never raises for individual failures — the summary carries them.
        """

        unique = dedupe_posts(posts)
        tasks = [
            asyncio.create_task(self._guarded(post, index)) for index, post in enumerate(unique, 1)
        ]

        active_reporter = reporter or self._reporter
        self._total = len(unique)
        self._done = 0
        if active_reporter is not None:
            active_reporter.begin(len(unique), title=title)

        started = asyncio.get_running_loop().time()
        try:
            gathered = await asyncio.gather(*tasks, return_exceptions=True)
        finally:
            elapsed = asyncio.get_running_loop().time() - started
            if active_reporter is not None:
                active_reporter.finish()

        results: list[DownloadResult] = []
        for outcome in gathered:
            if isinstance(outcome, BaseException):
                log.exception("unexpected download error", exc_info=outcome)
                results.append(
                    DownloadResult(
                        post_id=-1,
                        status=DownloadStatus.FAILED,
                        error=f"internal error: {outcome}",
                    )
                )
            else:
                results.append(outcome)
        return DownloadSummary(results=tuple(results), elapsed=elapsed)

    async def download_post(self, post: Post, *, index: int | None = None) -> DownloadResult:
        """Download a single post (no progress reporting)."""

        return await self._guarded(post, index)

    async def _guarded(self, post: Post, index: int | None = None) -> DownloadResult:
        async with self._semaphore:
            try:
                result = await self._download_with_retries(post, index=index)
            except DownloadError as exc:
                result = DownloadResult(
                    post_id=post.id,
                    status=_status_for(exc),
                    url=post.file_url,
                    attempts=exc.attempts,
                    error=str(exc),
                    tags=post.tags,
                )
            except (OSError, httpx.HTTPError, ApiError) as exc:  # pragma: no cover - safety net
                result = DownloadResult(
                    post_id=post.id,
                    status=DownloadStatus.FAILED,
                    url=post.file_url,
                    attempts=self.retry_policy.attempts,
                    error=str(exc) or exc.__class__.__name__,
                    tags=post.tags,
                )
            finally:
                self._done += 1

        self._record(result, post)
        reporter = self._reporter
        if reporter is not None:
            reporter.event(
                DownloadEvent(
                    post_id=post.id,
                    status=result.status,
                    size=result.size,
                    path=Path(result.path) if result.path else None,
                    message=result.error,
                    remaining=max(0, self._total - self._done),
                )
            )
        return result

    # -- single file ------------------------------------------------------- #
    async def _download_with_retries(
        self, post: Post, *, index: int | None = None
    ) -> DownloadResult:
        if not post.is_available:
            reason = post.unavailable_reason or "file is not available"
            log.info("post %s is unavailable: %s", post.id, reason)
            return DownloadResult(
                post_id=post.id,
                status=DownloadStatus.UNAVAILABLE,
                url=post.file_url,
                error=reason,
                tags=post.tags,
            )

        url = str(post.file_url)
        declared = post.file_size
        if (
            declared is not None
            and self.options.min_size is not None
            and declared < self.options.min_size
        ):
            return self._skip(
                post,
                f"file size {human_bytes(declared)} is below the "
                f"{human_bytes(self.options.min_size)} minimum",
                url,
            )
        if (
            declared is not None
            and self.options.max_size is not None
            and declared > self.options.max_size
        ):
            return self._skip(
                post,
                f"file size {human_bytes(declared)} is above the "
                f"{human_bytes(self.options.max_size)} maximum",
                url,
            )

        target = self.target_for(post, index=index)
        if self._should_skip(post, target):
            return DownloadResult(
                post_id=post.id,
                status=DownloadStatus.SKIPPED,
                path=str(target),
                url=url,
                size=target.stat().st_size if target.exists() else 0,
                tags=post.tags,
            )
        if self.options.dry_run:
            return DownloadResult(
                post_id=post.id,
                status=DownloadStatus.DRY_RUN,
                path=str(target),
                url=url,
                size=post.file_size or 0,
                tags=post.tags,
            )

        ensure_directory(target.parent)
        attempt = 0
        last_error: DownloadError | None = None
        while attempt < self.retry_policy.attempts:
            attempt += 1
            try:
                return await self._download_once(post, url, target, attempt=attempt)
            except DownloadError as exc:
                last_error = exc
                exc.attempts = attempt
                if not _is_retryable_download_error(exc) or attempt >= self.retry_policy.attempts:
                    raise
                delay = self._delay_for(exc, attempt)
                log.info("post %s: %s — retrying in %.2fs", post.id, exc, delay)
                await self._sleep(delay)
        raise last_error or DownloadError("download failed", post_id=post.id, url=url)

    def _skip(self, post: Post, message: str, url: str | None) -> DownloadResult:
        return DownloadResult(
            post_id=post.id,
            status=DownloadStatus.SKIPPED,
            url=url,
            error=message,
            tags=post.tags,
        )

    def _should_skip(self, post: Post, target: Path) -> bool:
        """Whether this post should be skipped because its target already exists.

        A file recorded in the history only counts when it *is* the requested
        destination: asking for a different ``--output`` (or a different
        collection) always downloads again, because the user asked for a copy in
        that place. Use ``--no-duplicates`` to suppress identical content instead.
        """

        if self.options.overwrite or not self.options.skip_existing:
            return False
        if target.exists():
            try:
                if target.stat().st_size > 0:
                    return True
            except OSError:  # pragma: no cover - unreadable file
                return False

        if self.history is not None:
            entry = self.history.get(post.id)
            if entry is not None and entry.status == DownloadStatus.COMPLETED.value and entry.path:
                recorded = Path(entry.path)
                if recorded.exists() and _same_path(recorded, target):
                    return True
        return False

    def _request_headers(self, url: str) -> dict[str, str]:
        """Headers for a file request.

        Gelbooru's CDN (img*.gelbooru.com) answers requests without a
        ``Referer`` with an HTML redirect page, so one is sent for gelbooru.com
        hosts — and only for them, so custom mirrors are not misrepresented.
        """

        headers: dict[str, str] = {}
        referer = self.options.referer
        if referer and _is_gelbooru_host(url):
            headers["Referer"] = referer
        return headers

    async def _download_once(
        self,
        post: Post,
        url: str,
        target: Path,
        *,
        attempt: int,
    ) -> DownloadResult:
        part = target.with_name(target.name + ".part")
        offset = self._resume_offset(part, target)

        headers: dict[str, str] = self._request_headers(url)
        if offset:
            headers["Range"] = f"bytes={offset}-"

        try:
            response = await self._client.get(url, headers=headers)
        except httpx.TimeoutException as exc:
            msg = f"timed out after {self.options.timeout:g}s"
            raise DownloadError(msg, post_id=post.id, url=url) from exc
        except httpx.HTTPError as exc:
            raise DownloadError(f"network error: {exc}", post_id=post.id, url=url) from exc

        self._raise_for_status(response, post, url, retry_after_header=True)
        self._reject_content_type(response, post, url)

        extension = self._extension_from(post, response, target)
        if extension and extension.lower() != (target.suffix.lstrip(".") or ""):
            # The URL had no useful suffix; keep the file extension truthful and
            # drop any partial file that belonged to the old name.
            self._discard(target.with_name(target.name + ".part"))
            target = target.with_suffix(f".{extension.lower()}")
            part = target.with_name(target.name + ".part")
            offset = self._resume_offset(part, target)

        if offset and response.status_code == HTTP_PARTIAL:
            mode = "ab"
        else:
            if offset:
                log.debug("server ignored Range for post %s; restarting download", post.id)
                offset = 0
            mode = "wb"
            self._discard(part)

        declared = _content_length(response)
        if (
            declared is not None
            and self.options.max_size is not None
            and declared > self.options.max_size
        ):
            raise DownloadError(
                f"file is {human_bytes(declared)}, above the {human_bytes(self.options.max_size)} maximum",
                post_id=post.id,
                url=url,
            )

        written = offset
        digest = hashlib.sha256()
        try:
            with part.open(mode) as handle:
                if mode == "ab" and offset:
                    # Hash the existing prefix so --no-duplicates still works.
                    digest = _seed_digest(part)
                first_chunk = True
                async for chunk in response.aiter_bytes(CHUNK_SIZE):
                    if first_chunk:
                        self._reject_html(chunk, post, url)
                        first_chunk = False
                    written += len(chunk)
                    if self.options.max_size is not None and written > self.options.max_size:
                        raise DownloadError(
                            f"download exceeded the {human_bytes(self.options.max_size)} maximum",
                            post_id=post.id,
                            url=url,
                        )
                    if self.options.duplicate_check:
                        digest.update(chunk)
                    handle.write(chunk)
                handle.flush()
                os.fsync(handle.fileno())
        except DownloadError:
            self._discard(part)
            raise
        except OSError as exc:
            self._discard(part)
            raise DownloadError(f"cannot write file: {exc}", post_id=post.id, url=url) from exc

        total = written
        if total == 0:
            self._discard(part)
            raise DownloadError("server returned an empty file", post_id=post.id, url=url)
        if self.options.min_size is not None and total < self.options.min_size:
            self._discard(part)
            return self._skip(
                post,
                f"file is {human_bytes(total)}, below the {human_bytes(self.options.min_size)} minimum",
                url,
            )

        try:
            part.replace(target)
        except OSError as exc:
            self._discard(part)
            raise DownloadError(f"cannot finalise file: {exc}", post_id=post.id, url=url) from exc

        # The recorded size must match the bytes actually on disk, even when a
        # resumed transfer had to restart because the server ignored Range.
        try:
            on_disk = target.stat().st_size
            if on_disk > 0:
                total = on_disk
        except OSError:  # pragma: no cover - just replaced, so it exists
            pass

        sha256 = digest.hexdigest() if self.options.duplicate_check else None
        if sha256 and self.history is not None:
            duplicate = self.history.find_by_hash(sha256)
            if duplicate is not None and duplicate.post_id != post.id:
                # Identical content already on disk: drop the copy we just made.
                with contextlib.suppress(OSError):
                    target.unlink()
                return DownloadResult(
                    post_id=post.id,
                    status=DownloadStatus.DUPLICATE,
                    path=str(target),
                    url=url,
                    size=total,
                    attempts=attempt,
                    sha256=sha256,
                    error=f"identical content already downloaded as post {duplicate.post_id}",
                    tags=post.tags,
                )

        if self.options.archive_metadata:
            self._write_sidecar(post, target)

        log.debug("downloaded post %s -> %s (%s)", post.id, target, human_bytes(total))
        return DownloadResult(
            post_id=post.id,
            status=DownloadStatus.COMPLETED,
            path=str(target),
            url=url,
            size=total,
            attempts=attempt,
            sha256=sha256,
            tags=post.tags,
        )

    # -- helpers ----------------------------------------------------------- #
    def _delay_for(self, exc: DownloadError, attempt: int) -> float:
        retry_after = getattr(exc, "retry_after", None)
        if isinstance(exc, DownloadRateLimitError) and retry_after is not None:
            return min(max(float(retry_after), 0.0), self.retry_policy.max_retry_after)
        return self.retry_policy.delay_for(attempt, rng=random.Random(attempt))

    def _reject_content_type(self, response: httpx.Response, post: Post, url: str) -> None:
        """Refuse to store HTML error pages, JSON blobs, etc."""

        if not self.options.verify_content:
            return
        content_type = (response.headers.get("content-type") or "").split(";", 1)[0].strip().lower()
        if not content_type:
            return
        if content_type in _ACCEPTED_EXACT:
            return
        if any(content_type.startswith(prefix) for prefix in self.options.accepted_prefixes):
            return
        raise ContentRejectedError(
            f"server returned '{content_type}' instead of image data",
            post_id=post.id,
            url=url,
        )

    def _resume_offset(self, part: Path, target: Path) -> int:
        if not self.options.resume:
            return 0
        try:
            if part.exists() and not target.exists():
                size = part.stat().st_size
                return size if size > 0 else 0
        except OSError:  # pragma: no cover
            return 0
        return 0

    @staticmethod
    def _discard(part: Path) -> None:
        with contextlib.suppress(OSError):  # pragma: no cover - windows file locking
            part.unlink(missing_ok=True)

    def _extension_from(self, post: Post, response: httpx.Response, target: Path) -> str | None:
        """Decide the final extension: post metadata, then URL, then headers."""

        current = (target.suffix.lstrip(".") or "").lower()
        if post.extension and current == post.extension.lower():
            return None
        guessed = post.extension or extension_from_url(str(post.file_url or ""))
        if guessed:
            return guessed if guessed.lower() != current else None
        disposition = response.headers.get("content-disposition", "")
        if "filename=" in disposition:
            name = disposition.split("filename=", 1)[1].strip().strip('";')
            guessed = extension_from_url(name)
            if guessed:
                return guessed
        return content_type_to_extension(
            response.headers.get("content-type"), default=current or "jpg"
        )

    def _raise_for_status(
        self,
        response: httpx.Response,
        post: Post,
        url: str,
        *,
        retry_after_header: bool = False,
    ) -> None:
        status = response.status_code
        if status in (HTTP_OK, HTTP_PARTIAL):
            return
        retry_after = (
            parse_retry_after(response.headers.get("Retry-After")) if retry_after_header else None
        )
        detail = ""
        if retry_after:
            detail = f" (retry after {retry_after:.0f}s)"
        if status in (HTTP_NOT_FOUND, HTTP_GONE):
            raise DownloadUnavailableError(
                "file is not available on the server (the post may be deleted or the file removed)",
                post_id=post.id,
                url=url,
            )
        if status == HTTP_TOO_MANY_REQUESTS:
            raise DownloadRateLimitError(
                f"rate limited while downloading (HTTP 429){detail}",
                post_id=post.id,
                url=url,
                retry_after=retry_after,
            )
        if HTTP_SERVER_ERROR <= status < 600:
            error = DownloadError(
                f"server error while downloading (HTTP {status}){detail}",
                post_id=post.id,
                url=url,
            )
            error.status_code = status
            raise error
        content_type = response.headers.get("content-type", "")
        if "text/html" in content_type:
            error = ContentRejectedError(
                "server returned an HTML page instead of an image (the file is probably gone)",
                post_id=post.id,
                url=url,
            )
            error.status_code = status
            raise error
        error = DownloadError(
            f"unexpected HTTP {status} while downloading",
            post_id=post.id,
            url=url,
        )
        error.status_code = status
        raise error

    def _reject_html(self, chunk: bytes, post: Post, url: str) -> None:
        """Guard against error pages served with an ``image/*`` content type."""

        head = chunk[:64].lstrip().lower()
        if any(head.startswith(signature) for signature in _HTML_SIGNATURES):
            raise ContentRejectedError(
                "server returned an HTML/XML document instead of image data",
                post_id=post.id,
                url=url,
            )

    def _write_sidecar(self, post: Post, target: Path) -> None:
        import json

        sidecar = target.with_suffix(target.suffix + ".json")
        try:
            sidecar.write_text(
                json.dumps(
                    {**post.to_dict(), "downloaded_at": utc_now_iso(), "local_path": str(target)},
                    indent=2,
                    ensure_ascii=False,
                ),
                encoding="utf-8",
            )
        except OSError as exc:  # pragma: no cover - read-only directory
            log.warning("could not write metadata sidecar for post %s: %s", post.id, exc)

    def _record(self, result: DownloadResult, post: Post) -> None:
        if self.history is None:
            return
        try:
            self.history.record(result, tags=post.tags)
        except HistoryError as exc:  # pragma: no cover - database trouble
            log.warning("could not record post %s in the download history: %s", result.post_id, exc)


def _is_gelbooru_host(url: str) -> bool:
    """Whether ``url`` points at Gelbooru (site or CDN), not a third-party host."""

    host = (urlsplit(url).hostname or "").lower()
    return host == "gelbooru.com" or host.endswith(".gelbooru.com")


def _same_path(left: Path, right: Path) -> bool:
    """Whether two paths point at the same file (best effort, never raises)."""

    try:
        return left.resolve() == right.resolve()
    except OSError:  # pragma: no cover - unresolvable path
        return str(left) == str(right)


def _status_for(exc: DownloadError) -> DownloadStatus:
    if isinstance(exc, DownloadUnavailableError):
        return DownloadStatus.UNAVAILABLE
    return DownloadStatus.FAILED


def _is_retryable_download_error(exc: DownloadError) -> bool:
    if isinstance(exc, (DownloadUnavailableError, ContentRejectedError)):
        return False
    if isinstance(exc, DownloadRateLimitError):
        return True
    status = getattr(exc, "status_code", None)
    return status is not None and status in _RETRYABLE_STATUS


def _content_length(response: httpx.Response) -> int | None:
    raw = response.headers.get("content-length")
    if raw is None:
        return None
    try:
        return int(raw)
    except ValueError:
        return None


def _seed_digest(part: Path) -> hashlib._Hash:
    """Seed a SHA-256 with the already-downloaded prefix of a resumed file."""

    digest = hashlib.sha256()
    try:
        with part.open("rb") as handle:
            for chunk in iter(lambda: handle.read(CHUNK_SIZE), b""):
                digest.update(chunk)
    except OSError:  # pragma: no cover
        return hashlib.sha256()
    return digest


def planned_targets(downloader: Downloader, posts: Sequence[Post]) -> list[tuple[Post, Path]]:
    """Resolve the destination path for each post without downloading anything."""

    return [(post, downloader.target_for(post, index=index)) for index, post in enumerate(posts, 1)]
