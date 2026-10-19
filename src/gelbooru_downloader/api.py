"""Async Gelbooru ``dapi`` client.

Only the documented API is used (``/index.php?page=dapi``); the site is never
scraped. The client:

* authenticates with ``user_id``/``api_key`` (query parameters or HTTP basic),
* parses both the JSON and the XML response formats defensively,
* paginates transparently and honours a maximum number of posts,
* retries transient failures with exponential backoff and honours ``Retry-After``,
* raises typed exceptions instead of fabricating data.
"""

from __future__ import annotations

import asyncio
import json
import logging
import xml.etree.ElementTree as ET
from collections.abc import AsyncIterator, Callable, Iterable, Mapping, Sequence
from datetime import UTC, datetime
from typing import Any
from urllib.parse import urlencode, urlsplit, urlunsplit

import httpx

from .config import Config, NetworkConfig
from .exceptions import (
    ApiError,
    AuthenticationError,
    MalformedResponseError,
    NetworkError,
    NotFoundError,
    RateLimitError,
)
from .models import Post, PostPage
from .utils import RetryPolicy, parse_retry_after

__all__ = [
    "API_PATH",
    "DEFAULT_BASE_URL",
    "GelbooruClient",
    "parse_post_payload",
]

log = logging.getLogger("gelbooru.api")

DEFAULT_BASE_URL = "https://gelbooru.com"
API_PATH = "/index.php"

#: Gelbooru's documented pagination parameter.
#:
#: ``page`` selects the ``dapi`` route itself, so the page number has to travel
#: in ``pid`` — sending a second ``page=`` parameter makes the router fall back
#: to the HTML site (HTTP 302). ``pid`` is a **zero-based** page index: the
#: server reads ``offset = pid * limit``, so ``pid=0`` is the newest page.
PAGE_PARAM = "pid"

#: Gelbooru enforces a hard limit of 100 posts per request.
MAX_PAGE_SIZE = 100

HTTP_OK = 200
HTTP_PARTIAL = 206
HTTP_BAD_REQUEST = 400
HTTP_UNAUTHORIZED = 401
HTTP_FORBIDDEN = 403
HTTP_NOT_FOUND = 404
HTTP_TOO_MANY_REQUESTS = 429
HTTP_SERVER_ERROR = 500

_RETRYABLE_STATUS = frozenset({HTTP_TOO_MANY_REQUESTS, HTTP_SERVER_ERROR, 502, 503, 504, 507, 509})

#: Field names that identify a dict as a post record.
_POST_FIELDS = frozenset(
    {
        "file_url",
        "file_name",
        "preview_url",
        "tags",
        "width",
        "height",
        "rating",
        "md5",
        "type",
    }
)


def _looks_like_post(record: dict[str, Any]) -> bool:
    """Whether ``record`` carries post fields (used to surface bad records)."""

    return any(key in record for key in _POST_FIELDS)


# --------------------------------------------------------------------------- #
# Parsing
# --------------------------------------------------------------------------- #
def _posts_from_records(records: Any) -> list[dict[str, Any]]:
    """Normalise the many shapes Gelbooru returns into a list of dicts."""

    if records is None:
        return []
    if isinstance(records, dict):
        # {"post": [...]} already unwrapped, or a single post object.
        for key in ("post", "posts"):
            if key in records:
                return _posts_from_records(records[key])
        attributes = records.get("@attributes")
        if isinstance(attributes, dict):
            rest = {key: value for key, value in records.items() if key != "@attributes"}
            if rest:
                # A single post that carries its own attributes.
                return [{**attributes, **rest}]
            # Envelope only — a zero-result response looks exactly like this:
            # {"@attributes": {"limit": 100, "offset": 0, "count": 0}}
            return []
        return [records] if "id" in records or _looks_like_post(records) else []
    if isinstance(records, list | tuple):
        flat: list[dict[str, Any]] = []
        for item in records:
            flat.extend(_posts_from_records(item))
        return flat
    return []


def _total_from(payload: Any) -> int | None:
    if not isinstance(payload, dict):
        return None
    attributes = payload.get("@attributes")
    if isinstance(attributes, dict):
        for key in ("count", "total"):
            value = attributes.get(key)
            if isinstance(value, int):
                return value
    value = payload.get("count")
    if isinstance(value, int):
        return value
    if isinstance(value, str):
        try:
            return int(value)
        except ValueError:
            return None
    return None


def _offset_from(payload: Any) -> int | None:
    if not isinstance(payload, dict):
        return None
    attributes = payload.get("@attributes")
    value = attributes.get("offset") if isinstance(attributes, dict) else payload.get("offset")
    if isinstance(value, int):
        return value
    if isinstance(value, str):
        try:
            return int(value)
        except ValueError:
            return None
    return None


def _parse_json(content: str) -> PostPage:
    try:
        payload = json.loads(content)
    except (ValueError, TypeError) as exc:
        msg = f"API returned invalid JSON: {exc}"
        raise MalformedResponseError(msg) from exc

    if isinstance(payload, dict):
        error = payload.get("error")
        if error:
            msg = f"API reported an error: {error}"
            raise ApiError(msg)
        records = payload
    elif isinstance(payload, list):
        records = payload
    else:  # pragma: no cover - json.loads only returns these two types
        msg = f"unexpected JSON payload of type {type(payload).__name__}"
        raise MalformedResponseError(msg)

    raw_posts = _posts_from_records(records)
    return PostPage(
        posts=tuple(_build_post(raw, raw_posts) for raw in raw_posts),
        total=_total_from(records if isinstance(records, dict) else None),
        offset=_offset_from(records if isinstance(records, dict) else None),
    )


def _parse_xml(content: str) -> PostPage:
    try:
        # Gelbooru's own dapi XML: no external entities, no DTD support needed.
        root = ET.fromstring(content.strip() or "<posts/>")  # noqa: S314
    except ET.ParseError as exc:
        msg = f"API returned invalid XML: {exc}"
        raise MalformedResponseError(msg) from exc

    if root.tag == "error":
        message = "".join(root.itertext()).strip() or "unknown error"
        msg = f"API reported an error: {message}"
        raise ApiError(msg)

    container = root if root.tag == "posts" else root.find("posts")
    if container is None:
        # Some deployments answer a bare <post/> for id lookups.
        if root.tag == "post":
            container = ET.Element("posts")
            container.append(root)
        else:
            msg = f"unexpected XML root element <{root.tag}>"
            raise MalformedResponseError(msg)

    raw_posts = [dict(element.attrib) for element in container.findall("post")]
    attributes = container.attrib
    total: int | None = None
    offset: int | None = None
    for key, target in (("count", "total"), ("offset", "offset")):
        raw_value = attributes.get(key)
        if raw_value is not None:
            try:
                value: int | None = int(raw_value)
            except ValueError:
                value = None
            if target == "total":
                total = value
            else:
                offset = value

    return PostPage(
        posts=tuple(_build_post(raw, raw_posts) for raw in raw_posts),
        total=total,
        offset=offset,
    )


def _build_post(raw: Mapping[str, Any], siblings: Sequence[Mapping[str, Any]]) -> Post:
    """Convert one raw record into a :class:`Post`, with a clear error message."""

    try:
        return Post.from_api(dict(raw))
    except ValueError as exc:
        msg = f"API returned an unusable post record ({exc})"
        log.debug("raw record: %r (siblings: %d)", raw, len(siblings))
        raise MalformedResponseError(msg) from exc


def parse_post_payload(content: str | bytes, response_format: str = "auto") -> PostPage:
    """Parse a ``dapi`` response body in JSON or XML form.

    ``response_format="auto"`` (the default) sniffs the body, so it works with
    both ``json=1`` and the classic XML endpoint.
    """

    text = content.decode("utf-8", errors="replace") if isinstance(content, bytes) else content
    stripped = text.lstrip()
    fmt = response_format.lower()

    if fmt == "xml" or (fmt == "auto" and stripped.startswith("<")):
        return _parse_xml(text)
    if fmt == "json" or stripped.startswith(("{", "[")):
        return _parse_json(text)
    # "auto" with an unrecognised body: try JSON, then XML.
    try:
        return _parse_json(text)
    except MalformedResponseError:
        return _parse_xml(text)


# --------------------------------------------------------------------------- #
# Client
# --------------------------------------------------------------------------- #
class GelbooruClient:
    """Async wrapper around the Gelbooru ``dapi`` endpoints."""

    def __init__(
        self,
        config: Config | NetworkConfig,
        *,
        transport: httpx.AsyncBaseTransport | None = None,
        sleep: Any = asyncio.sleep,
        timeout: float | None = None,
        retries: int | None = None,
        client: httpx.AsyncClient | None = None,
    ) -> None:
        self.network = config.network if isinstance(config, Config) else config
        general = config.general if isinstance(config, Config) else None
        self.timeout = timeout if timeout is not None else (general.timeout if general else 30.0)
        self.retry_policy = RetryPolicy(
            attempts=(retries if retries is not None else (general.retries if general else 3)) + 1
        )
        self._sleep = sleep
        self._owns_client = client is None
        self._client = client or httpx.AsyncClient(
            timeout=httpx.Timeout(self.timeout),
            follow_redirects=True,
            transport=transport,
            headers=self._headers(),
        )

    # -- plumbing ---------------------------------------------------------- #
    def _headers(self) -> dict[str, str]:
        headers = {
            "User-Agent": self.network.user_agent,
            "Accept": "application/json, text/xml;q=0.9, */*;q=0.8",
        }
        if self.network.auth_mode == "basic" and self.network.has_credentials:
            import base64

            token = base64.b64encode(
                f"{self.network.user_id}:{self.network.api_key}".encode()
            ).decode()
            headers["Authorization"] = f"Basic {token}"
        return headers

    async def __aenter__(self) -> GelbooruClient:
        return self

    async def __aexit__(self, *exc_info: object) -> None:
        await self.aclose()

    async def aclose(self) -> None:
        if self._owns_client:
            await self._client.aclose()

    def build_url(self, params: Mapping[str, Any]) -> str:
        """Build a fully qualified ``dapi`` URL (used by tests and ``--dry-run``)."""

        parts = urlsplit(self.network.base_url)
        path = parts.path.rstrip("/") + API_PATH
        query = {key: str(value) for key, value in params.items() if value not in (None, "")}
        return urlunsplit((parts.scheme, parts.netloc, path, urlencode(query), ""))

    def _api_params(self, params: dict[str, Any]) -> dict[str, Any]:
        """Add the documented ``dapi`` routing parameters to ``params``."""

        query = dict(params)
        query.setdefault("page", "dapi")  # route selector — never a page number
        query.setdefault("s", "post")
        query.setdefault("q", "index")
        if self.network.response_format != "xml":
            query["json"] = "1"
        if self.network.auth_mode == "params" and self.network.has_credentials:
            query["user_id"] = self.network.user_id
            query["api_key"] = self.network.api_key
        return query

    def _classify(self, response: httpx.Response) -> None:
        """Raise a typed exception for error responses."""

        status = response.status_code
        if status in {HTTP_OK, HTTP_PARTIAL}:
            return

        detail = ""
        content_type = response.headers.get("content-type", "")
        if "json" in content_type or "xml" in content_type or "text" in content_type:
            body = response.text[:400].strip()
            detail = _extract_error_message(body)

        url = str(response.url)
        if status in (HTTP_UNAUTHORIZED, HTTP_FORBIDDEN):
            if "rate limit" in detail.lower() or "too many" in detail.lower():
                raise RateLimitError(
                    f"Gelbooru rate limited the request (HTTP {status}).",
                    retry_after=parse_retry_after(response.headers.get("Retry-After")),
                    status_code=status,
                    url=url,
                )
            raise AuthenticationError(status_code=status, url=url)
        if status == HTTP_TOO_MANY_REQUESTS:
            raise RateLimitError(
                f"Gelbooru rate limited the request (HTTP {status}).",
                retry_after=parse_retry_after(response.headers.get("Retry-After")),
                status_code=status,
                url=url,
            )
        if status == HTTP_NOT_FOUND:
            raise NotFoundError(
                f"Gelbooru has no such endpoint or post (HTTP 404). {detail}".strip(),
                status_code=status,
                url=url,
            )
        if HTTP_SERVER_ERROR <= status < 600:
            raise ApiError(
                f"Gelbooru returned a server error (HTTP {status}). {detail}".strip(),
                status_code=status,
                url=url,
            )
        raise ApiError(
            f"Unexpected response from Gelbooru (HTTP {status}). {detail}".strip(),
            status_code=status,
            url=url,
        )

    async def _get(self, params: Mapping[str, Any]) -> httpx.Response:
        """Perform a GET with retries, backoff and ``Retry-After`` support."""

        url = self.build_url(params)
        attempt = 0
        while True:
            attempt += 1
            try:
                response = await self._client.get(url)
            except httpx.TimeoutException as exc:
                error: ApiError = NetworkError(
                    f"Request to Gelbooru timed out after {self.timeout:g}s.",
                    url=url,
                    hint="Increase network.timeout (gelbooru config set network.timeout 60).",
                )
                error.__cause__ = exc
            except httpx.HTTPError as exc:
                error = NetworkError(
                    f"Could not reach Gelbooru: {exc}",
                    url=url,
                    hint="Check your internet connection and network.base_url.",
                )
                error.__cause__ = exc
            else:
                try:
                    self._classify(response)
                except ApiError as exc:
                    if not self._is_retryable(exc) or attempt >= self.retry_policy.attempts:
                        raise
                    await self._backoff(exc, attempt)
                    continue
                return response

            if attempt >= self.retry_policy.attempts:
                raise error
            await self._backoff(error, attempt)

    def _is_retryable(self, error: ApiError) -> bool:
        return isinstance(error, (RateLimitError, NetworkError)) or (
            error.status_code is not None and error.status_code in _RETRYABLE_STATUS
        )

    async def _backoff(self, error: ApiError, attempt: int) -> None:
        delay = self._delay_for(error, attempt)
        log.warning(
            "API request failed (%s); retrying in %.2fs (attempt %d/%d)",
            error,
            delay,
            attempt,
            self.retry_policy.attempts,
        )
        await self._sleep(delay)

    def _delay_for(self, error: ApiError, attempt: int) -> float:
        retry_after = getattr(error, "retry_after", None)
        if isinstance(error, RateLimitError) and retry_after is not None:
            return min(max(float(retry_after), 0.0), self.retry_policy.max_retry_after)
        return self.retry_policy.delay_for(attempt)

    async def _request_page(self, params: Mapping[str, Any]) -> PostPage:
        response = await self._get(params)
        if not response.content:
            msg = "Gelbooru returned an empty response body."
            raise MalformedResponseError(msg, url=str(response.url))

        content_type = response.headers.get("content-type", "").lower()
        head = response.text.lstrip()[:16].lower()
        is_markup_page = head.startswith("<!doctype") or head.startswith("<html")
        looks_like_data = head[:1] in ("{", "[") or (head.startswith("<") and not is_markup_page)
        if ("html" in content_type or "text/plain" in content_type) and not looks_like_data:
            # Some 200 responses carry a notice instead of data, e.g.
            # "Too deep! Pull it back some." for pages beyond pid 20000.
            snippet = " ".join(response.text.split())[:200] or "no message"
            raise ApiError(
                f"Gelbooru refused the request: {snippet}",
                url=str(response.url),
                hint=(
                    "The dapi only serves roughly the newest 20,000 posts and needs "
                    "API credentials. Try a smaller --page, narrower tags, or set "
                    "GELBOORU_USER_ID / GELBOORU_API_KEY."
                ),
            )

        # "auto" prefers JSON and falls back to XML when the body is not JSON.
        fmt = "xml" if self.network.response_format == "xml" or "xml" in content_type else "auto"
        return parse_post_payload(response.content, fmt)

    # -- public API -------------------------------------------------------- #
    async def search(
        self,
        tags: str | None = None,
        *,
        page: int = 1,
        limit: int | None = None,
        extra: Mapping[str, Any] | None = None,
    ) -> PostPage:
        """Fetch a single page of posts.

        Args:
            tags: space separated tag expression (``"blue_hair solo -nude"``).
            page: 1-based page number (page 1 is the newest posts). It is sent to
                Gelbooru as the zero-based ``pid`` index.
            limit: page size (clamped to :data:`MAX_PAGE_SIZE`, the documented
                hard limit of 100 posts per request).
            extra: extra API parameters such as ``{"order": "score"}``.

        Raises:
            AuthenticationError: when Gelbooru answers 401 because no API
                credentials were configured.
            RateLimitError: on HTTP 429 (after retries are exhausted).
            MalformedResponseError: when the body is neither JSON nor XML.
        """

        per_page = max(1, min(limit or 100, MAX_PAGE_SIZE))
        first_page = max(1, page)
        params = self._api_params(
            {
                "tags": (tags or "").strip() or None,
                "limit": per_page,
                PAGE_PARAM: first_page - 1,
                **(dict(extra) if extra else {}),
            }
        )
        result = await self._request_page(params)
        # Gelbooru's reported ``offset`` is unreliable (it echoes the last pid
        # it saw), so the page/per_page we asked for is what we trust.
        return result.model_copy(update={"page": first_page, "per_page": per_page})

    async def iter_posts(
        self,
        tags: str | None = None,
        *,
        start_page: int = 1,
        page_size: int | None = None,
        max_posts: int | None = None,
        extra: Mapping[str, Any] | None = None,
        on_page: Callable[[PostPage], None] | None = None,
    ) -> AsyncIterator[Post]:
        """Yield posts across pages, stopping at ``max_posts``.

        The generator stops early on an empty page, when Gelbooru reports that
        there are no further results, or when the API stops returning new ids.
        ``on_page`` receives each raw page (useful for reading ``total``).
        """

        per_page = max(1, min(page_size or 100, MAX_PAGE_SIZE))
        page = max(1, start_page)
        produced = 0
        seen_ids: set[int] = set()

        while True:
            result = await self.search(tags, page=page, limit=per_page, extra=extra)
            if on_page is not None:
                on_page(result)
            if not result.posts:
                return

            new_in_page = 0
            for post in result.posts:
                if post.id in seen_ids:
                    continue
                seen_ids.add(post.id)
                yield post
                produced += 1
                new_in_page += 1
                if max_posts is not None and produced >= max_posts:
                    return

            if max_posts is not None and produced >= max_posts:
                return
            if not new_in_page:
                return
            if not result.has_more:
                return
            page += 1

    async def get_post(self, post_id: int) -> Post | None:
        """Fetch a single post by id, or ``None`` when it does not exist."""

        try:
            post_id = int(post_id)
        except (TypeError, ValueError) as exc:
            msg = f"invalid post id: {post_id!r}"
            raise ValueError(msg) from exc

        result = await self.search(f"id:{post_id}", page=1, limit=1)
        for post in result.posts:
            if post.id == post_id:
                return post
        return None

    async def get_posts(self, post_ids: Iterable[int]) -> list[Post]:
        """Fetch several posts by id in as few requests as possible."""

        ids: list[int] = []
        for post_id in post_ids:
            try:
                ids.append(int(post_id))
            except (TypeError, ValueError) as exc:
                msg = f"invalid post id: {post_id!r}"
                raise ValueError(msg) from exc
        if not ids:
            return []

        found: dict[int, Post] = {}
        # The API accepts several ids in one query ("id:1 id:2 id:3").
        batch_size = 100
        for start in range(0, len(ids), batch_size):
            chunk = ids[start : start + batch_size]
            expression = " ".join(f"id:{post_id}" for post_id in chunk)
            page = await self.search(expression, page=1, limit=len(chunk))
            for post in page.posts:
                found[post.id] = post
        return [found[post_id] for post_id in ids if post_id in found]

    async def close(self) -> None:  # pragma: no cover - convenience alias
        await self.aclose()


def _extract_error_message(body: str) -> str:
    """Pull a short human message out of an error body without trusting it."""

    if not body:
        return ""
    text = body.strip()
    if text.startswith("{") or text.startswith("["):
        try:
            payload = json.loads(text)
        except ValueError:
            pass
        else:
            if isinstance(payload, dict):
                for key in ("error", "message", "reason"):
                    value = payload.get(key)
                    if isinstance(value, str) and value.strip():
                        return value.strip()[:200]
    if text.startswith("<"):
        try:
            # Same rationale as _parse_xml: API response bodies only.
            root = ET.fromstring(text)  # noqa: S314
        except ET.ParseError:
            return ""
        message = "".join(root.itertext()).strip()
        return message[:200]
    return " ".join(text.split())[:200]


def rate_limit_hint(response: httpx.Response) -> str | None:
    """Return a human readable rate-limit hint from ``Retry-After``."""

    seconds = parse_retry_after(response.headers.get("Retry-After"))
    if seconds is None:
        return None
    resume_at = datetime.fromtimestamp(datetime.now(UTC).timestamp() + seconds, tz=UTC)
    return f"Retry-After: {seconds:.0f}s (until {resume_at:%H:%M:%S} UTC)"
