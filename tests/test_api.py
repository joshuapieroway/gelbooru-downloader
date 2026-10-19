"""Tests for the Gelbooru dapi client: parsing, pagination, auth and errors."""

from __future__ import annotations

import json
from collections.abc import Callable
from datetime import UTC, datetime, timedelta
from email.utils import format_datetime

import httpx
import pytest

from gelbooru_downloader.api import PAGE_PARAM, GelbooruClient, parse_post_payload
from gelbooru_downloader.config import Config
from gelbooru_downloader.exceptions import (
    ApiError,
    AuthenticationError,
    MalformedResponseError,
    NetworkError,
    NotFoundError,
    RateLimitError,
)
from gelbooru_downloader.models import Post, PostPage, Rating

from .conftest import Recorder, json_page, post_payload, xml_page


# --------------------------------------------------------------------------- #
# Response parsing
# --------------------------------------------------------------------------- #
class TestParsing:
    def test_parses_json_envelope(self) -> None:
        page = parse_post_payload(json_page([post_payload(id="7")]))
        assert isinstance(page, PostPage)
        assert page.count == 1
        assert page.posts[0].id == 7
        assert page.posts[0].rating == "safe"
        assert page.total == 1
        assert page.offset == 0

    def test_parses_bare_json_list(self) -> None:
        page = parse_post_payload(json.dumps([post_payload(id="8"), post_payload(id="9")]))
        assert [post.id for post in page.posts] == [8, 9]

    def test_parses_single_post_object(self) -> None:
        page = parse_post_payload(json.dumps({"post": post_payload(id="10")}))
        assert page.posts[0].id == 10

    def test_parses_xml_envelope(self) -> None:
        page = parse_post_payload(xml_page([post_payload(id="11")], count=42, offset=5), "xml")
        assert page.posts[0].id == 11
        assert page.total == 42
        assert page.offset == 5

    def test_auto_format_sniffs_xml(self) -> None:
        page = parse_post_payload(xml_page([post_payload(id="12")]))
        assert page.posts[0].id == 12

    def test_auto_format_sniffs_json(self) -> None:
        page = parse_post_payload(json_page([post_payload(id="13")]))
        assert page.posts[0].id == 13

    def test_empty_result_is_valid(self) -> None:
        page = parse_post_payload(
            json.dumps({"@attributes": {"count": 0, "offset": 0}, "post": []})
        )
        assert page.count == 0
        assert page.has_more is False

    def test_empty_xml_is_valid(self) -> None:
        page = parse_post_payload('<posts count="0"></posts>', "xml")
        assert page.count == 0

    def test_numeric_strings_are_coerced(self) -> None:
        post = parse_post_payload(json_page([post_payload(file_size="1234", width="800")])).posts[0]
        assert post.file_size == 1234
        assert post.width == 800

    def test_missing_fields_default_safely(self) -> None:
        raw = {"id": "555"}
        post = Post.from_api(raw)
        assert post.id == 555
        assert post.file_url is None
        assert post.tags == ""
        assert post.is_available is False
        assert post.unavailable_reason == "no file URL supplied by the API"

    def test_parses_gelbooru_date_strings(self) -> None:
        # The live JSON API returns created_at as "Fri Oct 16 18:15:34 -0500 2026".
        post = Post.from_api(post_payload(created_at="Fri Oct 16 18:15:34 -0500 2026"))
        assert post.created_at is not None
        assert post.created_at_dt is not None
        assert post.created_at_dt.year == 2026
        assert post.created_at_dt.month == 10

    def test_parses_iso_and_epoch_timestamps(self) -> None:
        assert Post.from_api(post_payload(created_at="1700000000")).created_at == 1700000000
        assert Post.from_api(post_payload(created_at=1700000000)).created_at == 1700000000
        iso = Post.from_api(post_payload(created_at="2026-01-02T03:04:05+00:00"))
        assert iso.created_at is not None
        assert iso.created_at_dt is not None

    def test_unparseable_timestamp_is_none(self) -> None:
        assert Post.from_api(post_payload(created_at="not a date")).created_at is None

    def test_derives_extension_from_url_without_type(self) -> None:
        # The live API omits both "type" and "file_name".
        post = Post.from_api(
            post_payload(type="", file_name="", file_url="https://img4.gelbooru.com/x/ab/abc.png")
        )
        assert post.extension == "png"
        assert post.media_kind == "image"

    def test_general_rating_is_recognised(self) -> None:
        post = Post.from_api(post_payload(rating="general"))
        assert post.rating == "general"
        assert post.rating_value is Rating.GENERAL

    def test_html_escaped_tags_are_unescaped(self) -> None:
        # The live JSON payload escapes tag names: "&gt;3&lt;" means ">3<".
        post = Post.from_api(post_payload(tags="&gt;3&lt; 2girls arknights"))
        assert post.tag_list[:2] == [">3<", "2girls"]

    def test_escaped_ampersand_in_tags(self) -> None:
        post = Post.from_api(post_payload(tags="a&amp;b solo"))
        assert post.tag_list == ["a&b", "solo"]

    def test_pid_is_zero_based(self) -> None:
        # Verified against the live API: offset = pid * limit, pid=0 is newest.
        assert PAGE_PARAM == "pid"

    def test_empty_envelope_without_post_key(self) -> None:
        # The live API answers a zero-result query with attributes only:
        # {"@attributes": {"limit": 100, "offset": 0, "count": 0}}
        page = parse_post_payload('{"@attributes":{"limit":100,"offset":0,"count":0}}')
        assert page.count == 0
        assert page.total == 0
        assert page.has_more is False

    def test_attributes_merged_into_a_single_post(self) -> None:
        raw = post_payload(id="4242")
        raw["@attributes"] = {"limit": 1, "count": 1}
        page = parse_post_payload(json.dumps({"post": raw}))
        assert page.count == 1
        assert page.posts[0].id == 4242
        assert page.posts[0].file_url is not None

    def test_invalid_json_raises(self) -> None:
        with pytest.raises(MalformedResponseError):
            parse_post_payload("this is not json", "json")

    def test_invalid_xml_raises(self) -> None:
        with pytest.raises(MalformedResponseError):
            parse_post_payload("<posts><post id='1'></posts>", "xml")

    def test_record_without_id_raises(self) -> None:
        with pytest.raises(MalformedResponseError, match="unusable post record"):
            parse_post_payload(
                json.dumps({"post": [{"file_url": "https://example.invalid/a.png"}]})
            )

    def test_api_error_field_raises(self) -> None:
        with pytest.raises(ApiError, match="search down"):
            parse_post_payload(json.dumps({"error": "search down"}))

    def test_xml_error_element_raises(self) -> None:
        with pytest.raises(ApiError):
            parse_post_payload("<error>search down</error>", "xml")

    def test_deleted_post_is_not_available(self) -> None:
        post = Post.from_api(post_payload(status="deleted", file_url=""))
        assert post.is_available is False
        assert post.unavailable_reason is not None

    def test_never_invents_a_file_url(self) -> None:
        post = Post.from_api(post_payload(file_url="", type="png", width="100"))
        assert post.file_url is None
        assert post.is_available is False


# --------------------------------------------------------------------------- #
# Requests
# --------------------------------------------------------------------------- #
class TestRequests:
    async def test_search_uses_dapi_route_and_pid(
        self, make_api_client: Callable[..., GelbooruClient]
    ) -> None:
        seen: list[httpx.URL] = []

        def handler(request: httpx.Request) -> httpx.Response:
            seen.append(request.url)
            return httpx.Response(200, text=json_page([post_payload()]))

        client = make_api_client(handler)
        try:
            await client.search("blue_hair solo", page=3, limit=25)
        finally:
            await client.aclose()

        url = seen[0]
        params = dict(url.params)
        assert params["page"] == "dapi"
        assert params["s"] == "post"
        assert params["q"] == "index"
        assert params["tags"] == "blue_hair solo"
        assert params["limit"] == "25"
        # CLI page 3 is the server's zero-based pid 2.
        assert params[PAGE_PARAM] == "2"

    async def test_credentials_are_sent_as_parameters(
        self, make_api_client: Callable[..., GelbooruClient]
    ) -> None:
        seen: dict[str, str] = {}

        def handler(request: httpx.Request) -> httpx.Response:
            seen.update(dict(request.url.params))
            return httpx.Response(200, text=json_page([]))

        client = make_api_client(handler)
        try:
            await client.search("solo")
        finally:
            await client.aclose()

        assert seen["user_id"] == "42"
        assert seen["api_key"] == "key-123"

    async def test_basic_auth_header(self) -> None:
        import base64

        config = Config()
        config.network.auth_mode = "basic"
        config.network.api_key = "secret"
        config.network.user_id = "77"

        async with GelbooruClient(
            config, transport=httpx.MockTransport(lambda r: httpx.Response(200, text="{}"))
        ) as client:
            assert client._client.headers["Authorization"].startswith("Basic ")
            decoded = base64.b64decode(client._client.headers["Authorization"].split()[1]).decode()
            assert decoded == "77:secret"

    async def test_page_size_is_capped_at_100(
        self, make_api_client: Callable[..., GelbooruClient]
    ) -> None:
        seen: dict[str, str] = {}

        def handler(request: httpx.Request) -> httpx.Response:
            seen.update(dict(request.url.params))
            return httpx.Response(200, text=json_page([]))

        client = make_api_client(handler)
        try:
            await client.search("solo", limit=5000)
        finally:
            await client.aclose()
        assert seen["limit"] == "100"

    async def test_user_agent_is_sent(self, make_api_client: Callable[..., GelbooruClient]) -> None:
        async with make_api_client(lambda r: httpx.Response(200, text="{}")) as client:
            assert client._client.headers["User-Agent"]


# --------------------------------------------------------------------------- #
# Pagination
# --------------------------------------------------------------------------- #
class TestPagination:
    async def test_iter_posts_walks_pages(
        self, make_api_client: Callable[..., GelbooruClient]
    ) -> None:
        pages = {
            0: json_page([post_payload(id="1"), post_payload(id="2")], count=5, offset=0),
            1: json_page([post_payload(id="3"), post_payload(id="4")], count=5, offset=2),
            2: json_page([post_payload(id="5")], count=5, offset=4),
        }
        requested: list[int] = []

        def handler(request: httpx.Request) -> httpx.Response:
            pid = int(dict(request.url.params)[PAGE_PARAM])
            requested.append(pid)
            return httpx.Response(200, text=pages.get(pid, json_page([], count=5)))

        client = make_api_client(handler)
        try:
            ids = [post.id async for post in client.iter_posts("solo", page_size=2)]
        finally:
            await client.aclose()

        assert ids == [1, 2, 3, 4, 5]
        assert requested == [0, 1, 2]

    async def test_iter_posts_stops_at_max(
        self, make_api_client: Callable[..., GelbooruClient]
    ) -> None:
        def handler(request: httpx.Request) -> httpx.Response:
            return httpx.Response(
                200, text=json_page([post_payload(id=str(i)) for i in range(10)], count=1000)
            )

        client = make_api_client(handler)
        try:
            ids = [post.id async for post in client.iter_posts("solo", page_size=10, max_posts=4)]
        finally:
            await client.aclose()
        assert len(ids) == 4

    async def test_iter_posts_deduplicates_repeated_ids(
        self, make_api_client: Callable[..., GelbooruClient]
    ) -> None:
        pages = {
            0: json_page([post_payload(id="1")], count=2, offset=0),
            1: json_page([post_payload(id="1")], count=2, offset=1),
        }

        def handler(request: httpx.Request) -> httpx.Response:
            pid = int(dict(request.url.params)[PAGE_PARAM])
            return httpx.Response(200, text=pages.get(pid, json_page([], count=2)))

        client = make_api_client(handler)
        try:
            ids = [post.id async for post in client.iter_posts("solo", page_size=1)]
        finally:
            await client.aclose()
        assert ids == [1]

    async def test_has_more_uses_total(self) -> None:
        page = parse_post_payload(json_page([post_payload()], count=10, offset=0)).model_copy(
            update={"per_page": 5}
        )
        assert page.has_more is True
        assert page.total_pages == 2

    async def test_first_page_uses_pid_zero(
        self, make_api_client: Callable[..., GelbooruClient]
    ) -> None:
        seen: dict[str, str] = {}

        def handler(request: httpx.Request) -> httpx.Response:
            seen.update(dict(request.url.params))
            return httpx.Response(200, text=json_page([]))

        async with make_api_client(handler) as client:
            await client.search("arknights", page=1, limit=50)

        assert seen[PAGE_PARAM] == "0"

    async def test_start_page_is_honoured(
        self, make_api_client: Callable[..., GelbooruClient]
    ) -> None:
        seen: list[int] = []

        def handler(request: httpx.Request) -> httpx.Response:
            seen.append(int(dict(request.url.params)[PAGE_PARAM]))
            return httpx.Response(200, text=json_page([], count=0))

        client = make_api_client(handler)
        try:
            _ = [post async for post in client.iter_posts("solo", start_page=4, page_size=5)]
        finally:
            await client.aclose()
        assert seen == [3]  # start_page=4 -> pid 3


# --------------------------------------------------------------------------- #
# Single posts
# --------------------------------------------------------------------------- #
class TestSinglePost:
    async def test_get_post_returns_post(
        self, make_api_client: Callable[..., GelbooruClient]
    ) -> None:
        async with make_api_client(
            lambda r: httpx.Response(200, text=json_page([post_payload(id="999")]))
        ) as client:
            post = await client.get_post(999)
        assert post is not None
        assert post.id == 999

    async def test_get_post_missing_returns_none(
        self, make_api_client: Callable[..., GelbooruClient]
    ) -> None:
        async with make_api_client(
            lambda r: httpx.Response(200, text=json_page([], count=0))
        ) as client:
            assert await client.get_post(4242) is None

    async def test_get_posts_batches_ids(
        self, make_api_client: Callable[..., GelbooruClient]
    ) -> None:
        def handler(request: httpx.Request) -> httpx.Response:
            tags = dict(request.url.params)["tags"]
            wanted = {int(part.split(":")[1]) for part in tags.split()}
            found = [post_payload(id=str(post_id)) for post_id in sorted(wanted)]
            return httpx.Response(200, text=json_page(found))

        async with make_api_client(handler) as client:
            posts = await client.get_posts([5, 6, 7])
        assert [post.id for post in posts] == [5, 6, 7]

    async def test_get_posts_ignores_missing(
        self, make_api_client: Callable[..., GelbooruClient]
    ) -> None:
        def handler(request: httpx.Request) -> httpx.Response:
            return httpx.Response(200, text=json_page([post_payload(id="5")]))

        async with make_api_client(handler) as client:
            posts = await client.get_posts([5, 6])
        assert [post.id for post in posts] == [5]

    async def test_invalid_post_id_raises_value_error(
        self, make_api_client: Callable[..., GelbooruClient]
    ) -> None:
        async with make_api_client(lambda r: httpx.Response(200, text="{}")) as client:
            with pytest.raises(ValueError):
                await client.get_post("not-a-number")


# --------------------------------------------------------------------------- #
# Errors and retries
# --------------------------------------------------------------------------- #
class TestErrors:
    async def test_401_raises_authentication_error(
        self, make_api_client: Callable[..., GelbooruClient]
    ) -> None:
        async with make_api_client(lambda r: httpx.Response(401)) as client:
            with pytest.raises(AuthenticationError) as excinfo:
                await client.search("solo")
        assert "API key" in (excinfo.value.hint or "")

    async def test_401_is_not_retried(
        self,
        make_api_client: Callable[..., GelbooruClient],
        recorder: Recorder,
    ) -> None:
        calls = {"n": 0}

        def handler(request: httpx.Request) -> httpx.Response:
            calls["n"] += 1
            return httpx.Response(401)

        async with make_api_client(handler) as client:
            with pytest.raises(AuthenticationError):
                await client.search("solo")
        assert calls["n"] == 1
        assert recorder.delays == []

    async def test_404_raises_not_found(
        self, make_api_client: Callable[..., GelbooruClient]
    ) -> None:
        async with make_api_client(lambda r: httpx.Response(404)) as client:
            with pytest.raises(NotFoundError):
                await client.search("solo")

    async def test_rate_limit_is_retried_and_honours_retry_after(
        self,
        make_api_client: Callable[..., GelbooruClient],
        recorder: Recorder,
    ) -> None:
        calls = {"n": 0}

        def handler(request: httpx.Request) -> httpx.Response:
            calls["n"] += 1
            if calls["n"] == 1:
                return httpx.Response(429, headers={"Retry-After": "7"}, text="slow down")
            return httpx.Response(200, text=json_page([post_payload()]))

        async with make_api_client(handler) as client:
            page = await client.search("solo")

        assert page.count == 1
        assert calls["n"] == 2
        assert recorder.delays == [7.0]

    async def test_rate_limit_gives_up_after_retries(
        self,
        make_api_client: Callable[..., GelbooruClient],
        recorder: Recorder,
    ) -> None:
        async with make_api_client(lambda r: httpx.Response(429)) as client:
            with pytest.raises(RateLimitError) as excinfo:
                await client.search("solo")
        assert len(recorder.delays) == 2  # retries=2 → attempts=3
        assert excinfo.value.status_code == 429

    async def test_server_error_is_retried_with_backoff(
        self,
        make_api_client: Callable[..., GelbooruClient],
        recorder: Recorder,
    ) -> None:
        calls = {"n": 0}

        def handler(request: httpx.Request) -> httpx.Response:
            calls["n"] += 1
            if calls["n"] < 3:
                return httpx.Response(503)
            return httpx.Response(200, text=json_page([]))

        async with make_api_client(handler) as client:
            await client.search("solo")

        assert calls["n"] == 3
        assert len(recorder.delays) == 2
        assert recorder.delays[1] > recorder.delays[0]  # exponential

    async def test_client_error_is_not_retried(
        self,
        make_api_client: Callable[..., GelbooruClient],
        recorder: Recorder,
    ) -> None:
        calls = {"n": 0}

        def handler(request: httpx.Request) -> httpx.Response:
            calls["n"] += 1
            return httpx.Response(400)

        async with make_api_client(handler) as client:
            with pytest.raises(ApiError):
                await client.search("solo")
        assert calls["n"] == 1
        assert recorder.delays == []

    async def test_network_failure_is_retried_then_raises(
        self,
        make_api_client: Callable[..., GelbooruClient],
        recorder: Recorder,
    ) -> None:
        def handler(request: httpx.Request) -> httpx.Response:
            raise httpx.ConnectError("no route to host", request=request)

        async with make_api_client(handler) as client:
            with pytest.raises(NetworkError):
                await client.search("solo")
        assert len(recorder.delays) == 2

    async def test_timeout_is_reported_clearly(
        self,
        make_api_client: Callable[..., GelbooruClient],
        recorder: Recorder,
    ) -> None:
        def handler(request: httpx.Request) -> httpx.Response:
            raise httpx.ReadTimeout("too slow", request=request)

        async with make_api_client(handler, retries=0) as client:
            with pytest.raises(NetworkError, match="timed out"):
                await client.search("solo")

    async def test_empty_body_raises_malformed(
        self, make_api_client: Callable[..., GelbooruClient]
    ) -> None:
        async with make_api_client(lambda r: httpx.Response(200, content=b"")) as client:
            with pytest.raises(MalformedResponseError, match="empty response"):
                await client.search("solo")

    async def test_html_response_raises_malformed(
        self, make_api_client: Callable[..., GelbooruClient]
    ) -> None:
        html = b"<!DOCTYPE html><html><body>please log in</body></html>"
        async with make_api_client(
            lambda r: httpx.Response(200, content=html, headers={"content-type": "text/html"})
        ) as client:
            with pytest.raises(ApiError) as excinfo:
                await client.search("solo")
        assert "refused the request" in str(excinfo.value)
        assert "log in" in str(excinfo.value)

    async def test_plain_text_json_is_still_parsed(
        self, make_api_client: Callable[..., GelbooruClient]
    ) -> None:
        # httpx labels body-less test responses text/plain; real JSON must parse.
        async with make_api_client(
            lambda r: httpx.Response(200, text=json_page([post_payload()]))
        ) as client:
            page = await client.search("solo")
        assert page.count == 1

    async def test_too_deep_page_is_explained(
        self, make_api_client: Callable[..., GelbooruClient]
    ) -> None:
        # The live API answers pages beyond pid 20000 with HTTP 200 and a
        # plain-text notice instead of JSON.
        notice = b"Too deep! Pull it back some. Holy fuck."
        async with make_api_client(
            lambda r: httpx.Response(200, content=notice, headers={"content-type": "text/html"})
        ) as client:
            with pytest.raises(ApiError) as excinfo:
                await client.search("", page=50000, limit=1)
        assert "Too deep" in str(excinfo.value)
        assert excinfo.value.hint is not None

    async def test_xml_response_is_parsed(
        self, make_api_client: Callable[..., GelbooruClient]
    ) -> None:
        xml = xml_page([post_payload(id="77")])
        async with make_api_client(
            lambda r: httpx.Response(
                200, content=xml.encode(), headers={"content-type": "text/xml"}
            )
        ) as client:
            page = await client.search("solo")
        assert page.posts[0].id == 77

    async def test_retry_after_http_date_is_understood(
        self, make_api_client: Callable[..., GelbooruClient], recorder: Recorder
    ) -> None:
        from gelbooru_downloader.utils import parse_retry_after

        target = datetime.now(UTC) + timedelta(seconds=42)
        header = format_datetime(target, usegmt=True)
        assert 30 <= (parse_retry_after(header) or 0) <= 45

    async def test_error_message_is_extracted_from_body(self) -> None:
        from gelbooru_downloader.api import _extract_error_message

        assert _extract_error_message('{"error":"search down"}') == "search down"
        assert _extract_error_message("<error>boom</error>") == "boom"
        assert _extract_error_message("plain text") == "plain text"
