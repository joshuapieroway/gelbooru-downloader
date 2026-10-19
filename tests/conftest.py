"""Shared fixtures.

No test in this suite touches the real Gelbooru: HTTP is either mocked with
``httpx.MockTransport`` or served by the local ``mock_server`` module.
"""

from __future__ import annotations

import json
from collections.abc import Callable
from pathlib import Path
from typing import Any

import httpx
import pytest

from gelbooru_downloader.api import GelbooruClient
from gelbooru_downloader.app import AppContext, create_context
from gelbooru_downloader.config import Config
from gelbooru_downloader.database import HistoryDatabase
from gelbooru_downloader.models import Post


class Recorder:
    """Records the delays a component would have slept for."""

    def __init__(self) -> None:
        self.delays: list[float] = []

    async def __call__(self, delay: float) -> None:
        self.delays.append(delay)

    @property
    def total(self) -> float:
        return sum(self.delays)


def post_payload(**overrides: Any) -> dict[str, str]:
    """A complete API post record (all values as strings, like Gelbooru)."""

    post_id = overrides.pop("id", "1234567")
    payload = {
        "id": str(post_id),
        "md5": "0123456789abcdef0123456789abcdef",
        "creator_id": "42",
        "file_name": f"{post_id}.png",
        "file_url": f"https://cdn.example.invalid/images/{post_id}.png",
        "file_size": "204800",
        "width": "1920",
        "height": "1080",
        "source": "https://example.invalid/source",
        "preview_url": f"https://cdn.example.invalid/thumb/{post_id}.jpg",
        "rating": "safe",
        "score": "42",
        "up_score": "50",
        "down_score": "2",
        "comment_count": "3",
        "tags": "blue_hair solo rating:safe long_hair",
        "created_at": "1700000000",
        "status": "active",
        "type": "png",
    }
    payload.update({key: str(value) for key, value in overrides.items()})
    return payload


def json_page(posts: list[dict[str, str]], *, count: int | None = None, offset: int = 0) -> str:
    """Wrap post records in Gelbooru's JSON envelope."""

    return json.dumps(
        {
            "@attributes": {"count": count if count is not None else len(posts), "offset": offset},
            "post": posts,
        }
    )


def xml_page(posts: list[dict[str, str]], *, count: int | None = None, offset: int = 0) -> str:
    """Render the same page in Gelbooru's classic XML format."""

    body = "".join(
        "<post " + " ".join(f'{key}="{value}"' for key, value in record.items()) + " />"
        for record in posts
    )
    return f'<posts count="{count if count is not None else len(posts)}" offset="{offset}">{body}</posts>'


def make_post(**overrides: Any) -> Post:
    """Build a :class:`Post` directly (no HTTP)."""

    return Post.from_api(post_payload(**overrides))


@pytest.fixture
def config() -> Config:
    cfg = Config()
    cfg.network.base_url = "https://gelbooru.test"
    cfg.network.api_key = "key-123"
    cfg.network.user_id = "42"
    cfg.general.page_size = 10
    return cfg


@pytest.fixture
def recorder() -> Recorder:
    return Recorder()


@pytest.fixture
def make_api_client(config: Config, recorder: Recorder) -> Callable[..., GelbooruClient]:
    """Factory building clients backed by ``httpx.MockTransport``."""

    def factory(
        handler: Callable[[httpx.Request], httpx.Response], **kwargs: Any
    ) -> GelbooruClient:
        kwargs.setdefault("retries", 2)
        return GelbooruClient(
            config,
            transport=httpx.MockTransport(handler),
            sleep=recorder,
            **kwargs,
        )

    return factory


@pytest.fixture
def history(tmp_path: Path) -> HistoryDatabase:
    db = HistoryDatabase(tmp_path / "history.db")
    yield db
    db.close()


@pytest.fixture
def context(tmp_path: Path) -> AppContext:
    """An :class:`AppContext` rooted in a throwaway directory."""

    app = create_context(
        config_path=tmp_path / "config.toml",
        color=False,
        use_env=False,
        overrides={"general.output_dir": str(tmp_path / "downloads")},
    )
    yield app
    app.close()


def write_image(path: Path, data: bytes = b"\x89PNG\r\n\x1a\n" + b"0" * 100) -> Path:
    """Create a fake image file on disk."""

    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    return path
