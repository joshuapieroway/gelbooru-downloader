"""Downloader tests: atomic writes, resume, retries, concurrency and filtering.

Everything runs against ``httpx.MockTransport`` — no real images are fetched.
"""

from __future__ import annotations

import asyncio
import dataclasses
import json
from collections.abc import Callable
from pathlib import Path
from typing import Any

import httpx
import pytest

from gelbooru_downloader.database import HistoryDatabase
from gelbooru_downloader.downloader import Downloader, DownloadOptions, target_path
from gelbooru_downloader.models import DownloadResult, DownloadStatus, Post

from .conftest import Recorder, post_payload

PNG = b"\x89PNG\r\n\x1a\n" + b"payload" * 32
BIG_PNG = b"\x89PNG\r\n\x1a\n" + b"z" * 4096


def image_response(
    data: bytes = PNG,
    *,
    content_type: str = "image/png",
    status: int = 200,
    headers: dict[str, str] | None = None,
) -> httpx.Response:
    return httpx.Response(
        status,
        content=data,
        headers={"content-type": content_type, **(headers or {})},
    )


def post_for(
    post_id: int = 1,
    *,
    url: str = "https://cdn.test/image.png",
    size: int | None = None,
    name: str = "image.png",
    unknown_size: bool = False,
) -> Post:
    """A post whose metadata points at ``url``."""

    return Post.from_api(
        post_payload(
            id=str(post_id),
            file_url=url,
            file_name=f"{post_id}_{name}",
            type=name.rsplit(".", 1)[-1] if "." in name else "png",
            file_size="" if unknown_size else str(size if size is not None else len(PNG)),
        )
    )


@pytest.fixture
def options(tmp_path: Path) -> DownloadOptions:
    return DownloadOptions(output_dir=tmp_path / "out", concurrency=2, retries=1, timeout=5)


def replace_options(options: DownloadOptions, **overrides: object) -> DownloadOptions:
    """Copy ``options`` with ``overrides`` applied."""

    return dataclasses.replace(options, **overrides)


@pytest.fixture
def make_downloader(
    options: DownloadOptions, history: HistoryDatabase, recorder: Recorder
) -> Callable[..., Downloader]:
    def factory(handler: Callable[[httpx.Request], httpx.Response], **overrides: Any) -> Downloader:
        return Downloader(
            replace_options(options, **{k: v for k, v in overrides.items() if k != "reporter"}),
            history=history,
            transport=httpx.MockTransport(handler),
            sleep=recorder,
            reporter=overrides.get("reporter"),
        )

    return factory


# --------------------------------------------------------------------------- #
# Happy path
# --------------------------------------------------------------------------- #
class TestSuccessfulDownload:
    async def test_writes_file_and_records_history(
        self,
        make_downloader: Callable[..., Downloader],
        options: DownloadOptions,
        history: HistoryDatabase,
    ) -> None:
        async with make_downloader(lambda r: image_response()) as downloader:
            result = await downloader.download_post(post_for(101))

        assert result.status is DownloadStatus.COMPLETED
        assert result.size == len(PNG)
        target = Path(result.path or "")
        assert target == options.output_dir / "101.png"
        assert target.read_bytes() == PNG

        entry = history.get(101)
        assert entry is not None
        assert entry.status == "completed"
        assert entry.file_size == len(PNG)
        assert entry.path == str(target)

    async def test_no_partial_file_is_left_behind(
        self, make_downloader: Callable[..., Downloader], options: DownloadOptions
    ) -> None:
        async with make_downloader(lambda r: image_response()) as downloader:
            await downloader.download_post(post_for(102))
        assert not list(options.output_dir.glob("*.part"))

    async def test_extension_comes_from_content_type_when_url_has_none(
        self, make_downloader: Callable[..., Downloader], options: DownloadOptions
    ) -> None:
        post = Post.from_api(
            post_payload(id="103", file_url="https://cdn.test/1234567", type="", file_name="")
        )
        async with make_downloader(
            lambda r: image_response(content_type="image/png")
        ) as downloader:
            result = await downloader.download_post(post)
        assert result.path is not None
        assert Path(result.path).name == "103.png"

    async def test_webm_video_is_accepted(self, make_downloader: Callable[..., Downloader]) -> None:
        post = Post.from_api(
            post_payload(id="104", file_url="https://cdn.test/104.webm", type="webm")
        )
        async with make_downloader(
            lambda r: image_response(b"\x1a\x45\xdf\xa3", content_type="video/webm")
        ) as d:
            result = await d.download_post(post)
        assert result.status is DownloadStatus.COMPLETED
        assert result.path is not None and result.path.endswith(".webm")

    async def test_filename_template_fields(self, tmp_path: Path, history: HistoryDatabase) -> None:
        options = DownloadOptions(
            output_dir=tmp_path / "out",
            filename_template="{id}-{rating}-{md5:.8}",
            retries=0,
        )
        downloader = Downloader(
            options,
            history=history,
            transport=httpx.MockTransport(lambda r: image_response()),
            sleep=_noop,
        )
        async with downloader:
            result = await downloader.download_post(post_for(105))
        assert result.path is not None and Path(result.path).name == "105-safe-01234567.png"

    async def test_archive_sidecar_is_written(
        self, tmp_path: Path, history: HistoryDatabase
    ) -> None:
        options = DownloadOptions(output_dir=tmp_path / "out", archive_metadata=True, retries=0)
        downloader = Downloader(
            options,
            history=history,
            transport=httpx.MockTransport(lambda r: image_response()),
            sleep=_noop,
        )
        async with downloader:
            result = await downloader.download_post(post_for(106))
        assert result.path is not None
        sidecar = Path(result.path).with_suffix(".png.json")
        assert sidecar.is_file()
        data = json.loads(sidecar.read_text())
        assert data["id"] == 106
        assert data["downloaded_at"]

    async def test_folder_by_tags_creates_subfolders(
        self, tmp_path: Path, history: HistoryDatabase
    ) -> None:
        options = DownloadOptions(output_dir=tmp_path / "out", folder_by_tags=True, retries=0)
        downloader = Downloader(
            options,
            history=history,
            transport=httpx.MockTransport(lambda r: image_response()),
            sleep=_noop,
        )
        async with downloader:
            result = await downloader.download_post(post_for(107))
        assert result.path is not None
        assert Path(result.path).parent.name == "blue_hair_solo"

    async def test_collection_option_creates_subfolder(
        self, tmp_path: Path, history: HistoryDatabase
    ) -> None:
        options = DownloadOptions(output_dir=tmp_path / "out", folder="My Collection", retries=0)
        downloader = Downloader(
            options,
            history=history,
            transport=httpx.MockTransport(lambda r: image_response()),
            sleep=_noop,
        )
        async with downloader:
            result = await downloader.download_post(post_for(108))
        assert result.path is not None
        assert Path(result.path).parent.name == "My Collection"


async def _noop(delay: float) -> None:
    return None


# --------------------------------------------------------------------------- #
# Skipping and overwrite
# --------------------------------------------------------------------------- #
class TestSkipping:
    async def test_existing_file_is_skipped_without_request(
        self, make_downloader: Callable[..., Downloader], options: DownloadOptions
    ) -> None:
        target = options.output_dir / "111.png"
        target.parent.mkdir(parents=True)
        target.write_bytes(PNG)
        calls: list[int] = []

        def handler(request: httpx.Request) -> httpx.Response:
            calls.append(1)
            return image_response()

        async with make_downloader(handler) as downloader:
            result = await downloader.download_post(post_for(111))

        assert result.status is DownloadStatus.SKIPPED
        assert calls == []
        assert target.read_bytes() == PNG

    async def test_empty_placeholder_is_replaced(
        self, make_downloader: Callable[..., Downloader], options: DownloadOptions
    ) -> None:
        target = options.output_dir / "112.png"
        target.parent.mkdir(parents=True)
        target.write_bytes(b"")
        async with make_downloader(lambda r: image_response()) as downloader:
            result = await downloader.download_post(post_for(112))
        assert result.status is DownloadStatus.COMPLETED
        assert target.read_bytes() == PNG

    async def test_overwrite_redownloads(
        self, make_downloader: Callable[..., Downloader], options: DownloadOptions
    ) -> None:
        target = options.output_dir / "113.png"
        target.parent.mkdir(parents=True)
        target.write_bytes(b"stale")
        async with make_downloader(lambda r: image_response(), overwrite=True) as downloader:
            result = await downloader.download_post(post_for(113))
        assert result.status is DownloadStatus.COMPLETED
        assert target.read_bytes() == PNG

    async def test_history_skip_when_the_same_file_was_recorded(
        self,
        make_downloader: Callable[..., Downloader],
        history: HistoryDatabase,
        options: DownloadOptions,
    ) -> None:
        target = options.output_dir / "114.png"
        target.parent.mkdir(parents=True)
        target.write_bytes(PNG)
        history.record(
            DownloadResult(
                post_id=114, status=DownloadStatus.COMPLETED, path=str(target), size=len(PNG)
            )
        )
        calls: list[int] = []
        async with make_downloader(lambda r: (calls.append(1), image_response())[1]) as downloader:
            result = await downloader.download_post(post_for(114))
        assert result.status is DownloadStatus.SKIPPED
        assert calls == []

    async def test_missing_history_file_triggers_download(
        self, make_downloader: Callable[..., Downloader], history: HistoryDatabase
    ) -> None:
        history.record(
            DownloadResult(
                post_id=115,
                status=DownloadStatus.COMPLETED,
                path="/nonexistent/115.png",
                size=len(PNG),
            )
        )
        async with make_downloader(lambda r: image_response()) as downloader:
            result = await downloader.download_post(post_for(115))
        assert result.status is DownloadStatus.COMPLETED


# --------------------------------------------------------------------------- #
# Failures
# --------------------------------------------------------------------------- #
class TestFailures:
    async def test_missing_file_reports_unavailable(
        self, make_downloader: Callable[..., Downloader], options: DownloadOptions
    ) -> None:
        async with make_downloader(lambda r: httpx.Response(404)) as downloader:
            result = await downloader.download_post(post_for(120))
        assert result.status is DownloadStatus.UNAVAILABLE
        assert not options.output_dir.exists() or not list(options.output_dir.glob("*.png"))

    async def test_html_response_is_not_saved(
        self, make_downloader: Callable[..., Downloader], options: DownloadOptions
    ) -> None:
        html = b"<!DOCTYPE html><html>removed</html>"
        async with make_downloader(
            lambda r: httpx.Response(200, content=html, headers={"content-type": "text/html"})
        ) as downloader:
            result = await downloader.download_post(post_for(121))
        assert result.status is DownloadStatus.FAILED
        assert "text/html" in (result.error or "")
        assert not list(options.output_dir.glob("121*"))

    async def test_html_body_with_image_content_type_is_rejected(
        self, make_downloader: Callable[..., Downloader], options: DownloadOptions
    ) -> None:
        html = b"<!DOCTYPE html><html>error page</html>"
        async with make_downloader(
            lambda r: httpx.Response(200, content=html, headers={"content-type": "image/png"})
        ) as downloader:
            result = await downloader.download_post(post_for(122))
        assert result.status is DownloadStatus.FAILED
        assert not list(options.output_dir.glob("122*"))
        assert not list(options.output_dir.glob("*.part"))

    async def test_json_error_is_not_saved(
        self, make_downloader: Callable[..., Downloader], options: DownloadOptions
    ) -> None:
        async with make_downloader(
            lambda r: httpx.Response(
                200, content=b'{"error":"gone"}', headers={"content-type": "application/json"}
            )
        ) as downloader:
            result = await downloader.download_post(post_for(123))
        assert result.status is DownloadStatus.FAILED
        assert not list(options.output_dir.glob("123*"))

    async def test_server_error_is_retried_then_succeeds(
        self,
        make_downloader: Callable[..., Downloader],
        recorder: Recorder,
    ) -> None:
        calls = {"n": 0}

        def handler(request: httpx.Request) -> httpx.Response:
            calls["n"] += 1
            return httpx.Response(503) if calls["n"] == 1 else image_response()

        async with make_downloader(handler) as downloader:
            result = await downloader.download_post(post_for(124))

        assert result.status is DownloadStatus.COMPLETED
        assert result.attempts == 2
        assert len(recorder.delays) == 1

    async def test_rate_limit_honours_retry_after(
        self,
        make_downloader: Callable[..., Downloader],
        recorder: Recorder,
    ) -> None:
        calls = {"n": 0}

        def handler(request: httpx.Request) -> httpx.Response:
            calls["n"] += 1
            if calls["n"] == 1:
                return httpx.Response(429, headers={"Retry-After": "3"})
            return image_response()

        async with make_downloader(handler) as downloader:
            result = await downloader.download_post(post_for(125))

        assert result.status is DownloadStatus.COMPLETED
        assert recorder.delays == [3.0]

    async def test_retries_are_bounded(
        self, make_downloader: Callable[..., Downloader], recorder: Recorder
    ) -> None:
        async with make_downloader(lambda r: httpx.Response(500)) as downloader:
            result = await downloader.download_post(post_for(126))
        assert result.status is DownloadStatus.FAILED
        assert result.attempts == 2  # retries=1 → 2 attempts
        assert len(recorder.delays) == 1

    async def test_network_error_is_reported(
        self, make_downloader: Callable[..., Downloader], recorder: Recorder
    ) -> None:
        def handler(request: httpx.Request) -> httpx.Response:
            raise httpx.ConnectError("boom", request=request)

        async with make_downloader(handler) as downloader:
            result = await downloader.download_post(post_for(127))
        assert result.status is DownloadStatus.FAILED
        assert "network error" in (result.error or "").lower()

    async def test_empty_body_is_failure(self, make_downloader: Callable[..., Downloader]) -> None:
        async with make_downloader(lambda r: image_response(b"")) as downloader:
            result = await downloader.download_post(post_for(128))
        assert result.status is DownloadStatus.FAILED
        assert "empty" in (result.error or "")

    async def test_unavailable_post_is_not_requested(
        self, make_downloader: Callable[..., Downloader]
    ) -> None:
        calls: list[int] = []
        post = Post.from_api(post_payload(id="129", file_url="", status="deleted"))

        def handler(request: httpx.Request) -> httpx.Response:
            calls.append(1)
            return image_response()

        async with make_downloader(handler) as downloader:
            result = await downloader.download_post(post)

        assert result.status is DownloadStatus.UNAVAILABLE
        assert calls == []
        assert result.error


# --------------------------------------------------------------------------- #
# Resume
# --------------------------------------------------------------------------- #
class TestResume:
    async def test_resumes_partial_file_with_range(
        self, make_downloader: Callable[..., Downloader], options: DownloadOptions
    ) -> None:
        part = options.output_dir / "130.png.part"
        part.parent.mkdir(parents=True, exist_ok=True)
        part.write_bytes(PNG[:10])
        seen: dict[str, str] = {}

        def handler(request: httpx.Request) -> httpx.Response:
            seen["range"] = request.headers.get("Range", "")
            start = 10
            return httpx.Response(
                206,
                content=PNG[start:],
                headers={
                    "content-type": "image/png",
                    "content-range": f"bytes {start}-{len(PNG) - 1}/{len(PNG)}",
                },
            )

        async with make_downloader(handler) as downloader:
            result = await downloader.download_post(post_for(130))

        assert seen["range"] == "bytes=10-"
        assert result.status is DownloadStatus.COMPLETED
        assert Path(result.path or "").read_bytes() == PNG
        assert not part.exists()

    async def test_restarts_when_server_ignores_range(
        self, make_downloader: Callable[..., Downloader], options: DownloadOptions
    ) -> None:
        part = options.output_dir / "131.png.part"
        part.parent.mkdir(parents=True, exist_ok=True)
        part.write_bytes(PNG[:10])

        async with make_downloader(lambda r: image_response(PNG)) as downloader:
            result = await downloader.download_post(post_for(131))

        assert result.status is DownloadStatus.COMPLETED
        assert Path(result.path or "").read_bytes() == PNG

    async def test_resume_can_be_disabled(
        self, options: DownloadOptions, history: HistoryDatabase
    ) -> None:
        part = options.output_dir / "132.png.part"
        part.parent.mkdir(parents=True, exist_ok=True)
        part.write_bytes(PNG[:10])
        seen: dict[str, str] = {}

        def handler(request: httpx.Request) -> httpx.Response:
            seen["range"] = request.headers.get("Range", "")
            return image_response(PNG)

        downloader = Downloader(
            replace_options(options, resume=False),
            history=history,
            transport=httpx.MockTransport(handler),
            sleep=_noop,
        )
        async with downloader:
            result = await downloader.download_post(post_for(132))
        assert seen["range"] == ""  # no Range header sent
        assert Path(result.path or "").read_bytes() == PNG


# --------------------------------------------------------------------------- #
# Batches
# --------------------------------------------------------------------------- #
class TestBatches:
    async def test_one_failure_does_not_stop_the_batch(
        self, make_downloader: Callable[..., Downloader], options: DownloadOptions
    ) -> None:
        def handler(request: httpx.Request) -> httpx.Response:
            if request.url.path.endswith("missing.png"):
                return httpx.Response(404)
            return image_response()

        posts = [
            post_for(140, url="https://cdn.test/missing.png"),
            post_for(141, url="https://cdn.test/ok.png"),
            post_for(142, url="https://cdn.test/ok2.png"),
        ]
        async with make_downloader(handler) as downloader:
            summary = await downloader.download_many(posts)

        assert summary.total == 3
        assert summary.completed == 2
        assert summary.unavailable == 1
        assert summary.failed == 0
        assert summary.bytes_downloaded == 2 * len(PNG)

    async def test_concurrency_is_bounded(
        self, options: DownloadOptions, history: HistoryDatabase
    ) -> None:
        state = {"active": 0, "max": 0}

        async def handler(request: httpx.Request) -> httpx.Response:
            state["active"] += 1
            state["max"] = max(state["max"], state["active"])
            await asyncio.sleep(0.01)
            state["active"] -= 1
            return image_response()

        downloader = Downloader(
            replace_options(options, concurrency=3),
            history=history,
            transport=httpx.MockTransport(handler),
            sleep=_noop,
        )
        async with downloader:
            summary = await downloader.download_many([post_for(200 + i) for i in range(12)])

        assert summary.completed == 12
        assert state["max"] <= 3
        assert state["max"] > 1  # genuinely concurrent

    async def test_duplicate_posts_are_downloaded_once(
        self, make_downloader: Callable[..., Downloader]
    ) -> None:
        calls: list[int] = []

        def handler(request: httpx.Request) -> httpx.Response:
            calls.append(1)
            return image_response()

        async with make_downloader(handler) as downloader:
            summary = await downloader.download_many([post_for(150), post_for(150)])

        assert summary.total == 1
        assert len(calls) == 1

    async def test_dry_run_downloads_nothing(
        self, make_downloader: Callable[..., Downloader], options: DownloadOptions
    ) -> None:
        calls: list[int] = []

        def handler(request: httpx.Request) -> httpx.Response:
            calls.append(1)
            return image_response()

        async with make_downloader(handler, dry_run=True) as downloader:
            summary = await downloader.download_many([post_for(160), post_for(161)])

        assert summary.dry_run == 2
        assert calls == []
        assert not options.output_dir.exists()

    async def test_duplicate_detection_uses_sha256(
        self, make_downloader: Callable[..., Downloader], options: DownloadOptions
    ) -> None:
        async with make_downloader(
            lambda r: image_response(BIG_PNG), duplicate_check=True
        ) as downloader:
            summary = await downloader.download_many([post_for(170), post_for(171)])

        assert summary.completed == 1
        assert summary.duplicates == 1
        remaining = sorted(p.name for p in options.output_dir.glob("*.png"))
        assert remaining == ["170.png"]

    async def test_size_limits_skip_files(
        self, make_downloader: Callable[..., Downloader], options: DownloadOptions
    ) -> None:
        async with make_downloader(
            lambda r: image_response(BIG_PNG), min_size=10_000
        ) as downloader:
            summary = await downloader.download_many([post_for(180)])
        assert summary.skipped == 1
        assert "below" in (summary.results[0].error or "")

        async with make_downloader(lambda r: image_response(BIG_PNG), max_size=10) as downloader:
            summary = await downloader.download_many([post_for(181)])
        assert summary.skipped == 1
        assert "above" in (summary.results[0].error or "")

    async def test_max_size_aborts_mid_stream(
        self, make_downloader: Callable[..., Downloader], options: DownloadOptions
    ) -> None:
        async with make_downloader(
            lambda r: image_response(BIG_PNG), max_size=64, retries=0
        ) as downloader:
            summary = await downloader.download_many([post_for(182, unknown_size=True)])
        assert summary.failed == 1
        assert not list(options.output_dir.glob("182*"))

    async def test_progress_reporter_receives_events(
        self, make_downloader: Callable[..., Downloader]
    ) -> None:
        events: list[str] = []

        class Reporter:
            def begin(self, total: int, *, title: str) -> None:
                events.append(f"begin:{total}")

            def event(self, event: object) -> None:
                events.append(f"event:{event.post_id}:{event.status.value}")  # type: ignore[attr-defined]

            def finish(self) -> None:
                events.append("finish")

        async with make_downloader(lambda r: image_response(), reporter=Reporter()) as downloader:
            await downloader.download_many([post_for(190)])

        assert events == ["begin:1", "event:190:completed", "finish"]

    async def test_empty_batch_is_fine(self, make_downloader: Callable[..., Downloader]) -> None:
        async with make_downloader(lambda r: image_response()) as downloader:
            summary = await downloader.download_many([])
        assert summary.total == 0
        assert summary.bytes_downloaded == 0


# --------------------------------------------------------------------------- #
# Path safety
# --------------------------------------------------------------------------- #
class TestReferer:
    """Gelbooru's CDN redirects requests without a Referer to an HTML page."""

    async def test_referer_sent_for_gelbooru_hosts(
        self, make_downloader: Callable[..., Downloader]
    ) -> None:
        seen: dict[str, str] = {}

        def handler(request: httpx.Request) -> httpx.Response:
            seen.update(request.headers)
            return image_response()

        async with make_downloader(handler, referer="https://gelbooru.com/") as downloader:
            result = await downloader.download_post(
                post_for(500, url="https://img4.gelbooru.com/images/a/b/c.png")
            )

        assert result.status is DownloadStatus.COMPLETED
        assert seen.get("referer") == "https://gelbooru.com/"

    async def test_referer_sent_for_the_site_host(
        self, make_downloader: Callable[..., Downloader]
    ) -> None:
        seen: dict[str, str] = {}

        def handler(request: httpx.Request) -> httpx.Response:
            seen.update(request.headers)
            return image_response()

        async with make_downloader(handler, referer="https://gelbooru.com/") as downloader:
            await downloader.download_post(post_for(501, url="https://gelbooru.com/images/x.png"))

        assert seen.get("referer") == "https://gelbooru.com/"

    async def test_no_referer_for_third_party_hosts(
        self, make_downloader: Callable[..., Downloader]
    ) -> None:
        seen: dict[str, str] = {}

        def handler(request: httpx.Request) -> httpx.Response:
            seen.update(request.headers)
            return image_response()

        async with make_downloader(handler, referer="https://gelbooru.com/") as downloader:
            await downloader.download_post(post_for(502, url="https://cdn.example.invalid/x.png"))

        assert "referer" not in seen

    async def test_referer_can_be_disabled(
        self, make_downloader: Callable[..., Downloader]
    ) -> None:
        seen: dict[str, str] = {}

        def handler(request: httpx.Request) -> httpx.Response:
            seen.update(request.headers)
            return image_response()

        async with make_downloader(handler, referer=None) as downloader:
            await downloader.download_post(post_for(503, url="https://img.gelbooru.com/x.png"))

        assert "referer" not in seen

    async def test_resume_requests_keep_the_referer(
        self, make_downloader: Callable[..., Downloader]
    ) -> None:
        seen: dict[str, str] = {}
        post = post_for(600, url="https://img4.gelbooru.com/images/a/b/c.png")

        def handler(request: httpx.Request) -> httpx.Response:
            seen.update(request.headers)
            return image_response(PNG)

        async with make_downloader(handler, referer="https://gelbooru.com/") as downloader:
            part = downloader.target_for(post).with_name("600.png.part")
            part.parent.mkdir(parents=True, exist_ok=True)
            part.write_bytes(PNG[:5])
            result = await downloader.download_post(post)

        assert result.status is DownloadStatus.COMPLETED
        assert seen.get("referer") == "https://gelbooru.com/"


class TestPathSafety:
    def test_traversal_in_template_is_contained(self, tmp_path: Path) -> None:
        from gelbooru_downloader.exceptions import DownloadError

        options = DownloadOptions(output_dir=tmp_path / "out", filename_template="{id}", retries=0)
        post = Post.from_api(
            post_payload(id="305", file_url="https://cdn.test/x.png", file_name="../../evil")
        )
        target = target_path(options, post)
        assert Path(target).resolve().is_relative_to((tmp_path / "out").resolve())
        assert Path(target).name == "305.png"

        options_bad_field = DownloadOptions(
            output_dir=tmp_path / "out", filename_template="{unknown}", retries=0
        )
        with pytest.raises(DownloadError, match="filename_template"):
            target_path(options_bad_field, post_for(306))

    def test_traversal_in_tag_folder_is_contained(self, tmp_path: Path) -> None:
        post = Post.from_api(
            post_payload(
                id="300",
                tags="../../../evil solo",
                file_url="https://cdn.test/300.png",
            )
        )
        options = DownloadOptions(output_dir=tmp_path / "out", folder_by_tags=True, retries=0)
        target = target_path(options, post)
        assert str(target.resolve()).startswith(str((tmp_path / "out").resolve()))

    def test_hostile_tag_cannot_escape(self, tmp_path: Path) -> None:
        post = Post.from_api(
            post_payload(id="301", tags=".. .. /etc/passwd", file_url="https://cdn.test/a.png")
        )
        options = DownloadOptions(output_dir=tmp_path / "out", folder_by_tags=True, retries=0)
        target = target_path(options, post)
        root = (tmp_path / "out").resolve()
        assert target.resolve().is_relative_to(root)

    def test_collection_name_is_sanitised(self, tmp_path: Path) -> None:
        options = DownloadOptions(output_dir=tmp_path / "out", folder="../../etc", retries=0)
        target = target_path(options, post_for(302))
        root = (tmp_path / "out").resolve()
        assert Path(target).resolve().is_relative_to(root)
        assert target.parent == root / "_.._etc"
        assert target.parent.parent == root

    def test_directory_creation_is_lazy(self, tmp_path: Path) -> None:
        options = DownloadOptions(output_dir=tmp_path / "out", retries=0)
        target_path(options, post_for(303))
        assert not (tmp_path / "out").exists()


class TestSummary:
    def test_counts_and_bytes(self) -> None:
        from gelbooru_downloader.models import DownloadResult, DownloadSummary

        summary = DownloadSummary(
            results=(
                DownloadResult(post_id=1, status=DownloadStatus.COMPLETED, size=100),
                DownloadResult(post_id=2, status=DownloadStatus.SKIPPED, size=50),
                DownloadResult(post_id=3, status=DownloadStatus.FAILED, error="boom"),
                DownloadResult(post_id=4, status=DownloadStatus.UNAVAILABLE, error="gone"),
            )
        )
        assert summary.completed == 1
        assert summary.skipped == 1
        assert summary.failed == 1
        assert summary.unavailable == 1
        assert summary.bytes_downloaded == 100  # skipped bytes are not counted
        assert summary.remaining == 2
        assert len(summary.failures) == 2


class TestIntegrity:
    async def test_part_file_is_removed_when_download_fails_midstream(
        self, make_downloader: Callable[..., Downloader], options: DownloadOptions
    ) -> None:
        def handler(request: httpx.Request) -> httpx.Response:
            return image_response(BIG_PNG, headers={"content-length": str(len(BIG_PNG))})

        async with make_downloader(handler, max_size=100, retries=0) as downloader:
            result = await downloader.download_post(post_for(400, unknown_size=True))

        assert result.status is DownloadStatus.FAILED
        assert not list(options.output_dir.glob("*.part"))

    async def test_failed_download_is_recorded_as_failed(
        self, make_downloader: Callable[..., Downloader], history: HistoryDatabase
    ) -> None:
        async with make_downloader(lambda r: httpx.Response(404)) as downloader:
            await downloader.download_post(post_for(401))
        entry = history.get(401)
        assert entry is not None
        assert entry.status == "unavailable"
        assert entry.error


class TestIteratorHelper:
    def test_planned_targets_is_pure(self, tmp_path: Path) -> None:
        options = DownloadOptions(output_dir=tmp_path / "out", retries=0)
        posts = [post_for(1), post_for(2)]
        targets = [target_path(options, post, index=index) for index, post in enumerate(posts, 1)]
        assert [path.name for path in targets] == ["1.png", "2.png"]
        assert not (tmp_path / "out").exists()
