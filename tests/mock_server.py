"""A tiny stand-in for Gelbooru's dapi + image CDN.

Used by the end-to-end smoke test (and handy for demos): it speaks the same
``index.php?page=dapi&s=post&q=index&tags=...&pid=N&limit=N`` protocol as
Gelbooru and serves real (tiny) PNG bytes for each post.

Run with::

    python tests/mock_server.py --port 8765
"""

from __future__ import annotations

import argparse
import json
import struct
from collections.abc import Iterator
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

# A 2x2 orange PNG — small, valid, and obviously not an error page.
PNG_BYTES = bytes.fromhex(
    "89504e470d0a1a0a0000000d49484452000000020000000208020000004d"
    "9a73000000194944415428cf63fccfc0f09f8114c0a80100dd70050165ea"
    "5ba10000000049454e44ae426082"
)


def _post(index: int, *, rating: str = "safe", extension: str = "png") -> dict[str, str]:
    post_id = 100000 + index
    return {
        "id": str(post_id),
        "md5": f"{post_id:032x}",
        "creator_id": "42",
        "file_name": f"{post_id}mock.{extension}",
        "file_url": f"__BASE__/image/{post_id}mock.{extension}",
        "file_size": str(len(PNG_BYTES)),
        "width": "2",
        "height": "2",
        "source": "https://example.invalid/source",
        "preview_url": f"__BASE__/thumb/{post_id}.jpg",
        "rating": rating,
        "score": str(50 - index),
        "up_score": "10",
        "down_score": "0",
        "comment_count": "0",
        "tags": f"mock_tag gelbooru_cli_test rating:{rating} source:https://example.invalid",
        "created_at": "1700000000",
        "status": "active",
        "type": extension,
    }


def _expand(base: str, records: list[dict[str, str]]) -> list[dict[str, str]]:
    return [
        {key: value.replace("__BASE__", base) for key, value in record.items()}
        for record in records
    ]


class Handler(BaseHTTPRequestHandler):
    """Serves dapi JSON responses and image bytes."""

    server_version = "MockGelbooru/1.0"
    total_posts = 25

    def log_message(self, format: str, *args: object) -> None:
        """Silence the default stderr logging."""

    # -- helpers ----------------------------------------------------------- #
    @property
    def base_url(self) -> str:
        host, _, port = self.server.server_address[0], None, self.server.server_address[1]  # type: ignore[misc]
        return f"http://{host}:{port}"

    def _send(
        self, status: int, body: bytes, content_type: str, extra: dict[str, str] | None = None
    ) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        for key, value in (extra or {}).items():
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(body)

    def _rate_limit_once(self) -> bool:
        """Fail the first API call with 429 to exercise the retry logic."""

        if getattr(self.server, "rate_limit_first", False):
            self.server.rate_limit_first = False  # type: ignore[attr-defined]
            self._send(429, b"slow down", "text/plain", {"Retry-After": "1"})
            return True
        return False

    # -- routing ---------------------------------------------------------- #
    def do_GET(self) -> None:
        parts = urlsplit(self.path)
        path = parts.path

        if path == "/image/" or path.startswith("/image/"):
            self._send_image(path)
            return
        if path == "/broken":
            self._send(404, b"not found", "text/plain")
            return
        if path == "/html-error":
            self._send(
                200, b"<!DOCTYPE html><html><body>nope</body></html>", "text/html; charset=UTF-8"
            )
            return
        if path == "/index.php":
            self._send_dapi(parse_qs(parts.query))
            return
        self._send(404, b"unknown endpoint", "text/plain")

    def _send_image(self, path: str) -> None:
        name = path.rsplit("/", 1)[-1]
        if "slow" in name:
            if self._rate_limit_once():
                return
        if "missing" in name:
            self._send(404, b"", "text/plain")
            return
        if "html" in name:
            self._send(200, b"<!DOCTYPE html><html>error</html>", "image/png")
            return
        self._send(200, PNG_BYTES, "image/png")

    def _send_dapi(self, query: dict[str, list[str]]) -> None:
        if query.get("page", [""])[0] != "dapi":
            self._send(200, b"<!DOCTYPE html><html>site</html>", "text/html; charset=UTF-8")
            return
        if self._rate_limit_once():
            return
        if query.get("require_auth", ["0"])[0] == "1" and "api_key" not in query:
            self._send(401, b"", "text/html; charset=UTF-8")
            return

        tags = query.get("tags", [""])[0]
        limit = max(1, min(int(query.get("limit", ["100"])[0]), 100))
        # Gelbooru's pid is a zero-based page index: offset = pid * limit.
        pid = max(0, int(query.get("pid", ["0"])[0]))

        records: list[dict[str, str]] = []
        if tags.startswith("id:"):
            for raw in tags.split():
                wanted = raw.split(":", 1)[1]
                index = int(wanted) - 100000
                if 0 <= index < self.total_posts:
                    records.append(
                        _post(index, rating="questionable" if index % 3 == 0 else "safe")
                    )
        else:
            start = pid * limit
            for index in range(start, min(start + limit, self.total_posts)):
                records.append(_post(index))
            if "gif" in tags:
                for record in records[:1]:
                    record["type"] = "gif"
                    record["file_url"] = record["file_url"].replace(".png", ".gif")

        records = _expand(self.base_url, records)
        payload = {
            "@attributes": {"limit": limit, "offset": pid * limit, "count": self.total_posts},
            "post": records,
        }
        self._send(200, json.dumps(payload).encode(), "application/json")


def _png_size(data: bytes) -> tuple[int, int]:  # pragma: no cover - debug helper
    width, height = struct.unpack(">II", data[16:24])
    return width, height


def serve(
    port: int = 8765, host: str = "127.0.0.1", *, rate_limit_first: bool = False
) -> ThreadingHTTPServer:
    """Start the mock server and return it (already serving in a thread)."""

    import threading

    server = ThreadingHTTPServer((host, port), Handler)
    server.rate_limit_first = rate_limit_first  # type: ignore[attr-defined]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server


def iter_paths() -> Iterator[str]:  # pragma: no cover - documentation helper
    yield "/index.php?page=dapi&s=post&q=index&tags=solo&limit=25&pid=1"
    yield "/image/100000mock.png"


def main() -> None:  # pragma: no cover - manual use
    parser = argparse.ArgumentParser(description="Mock Gelbooru API server")
    parser.add_argument("--port", type=int, default=8765)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--rate-limit-first", action="store_true")
    args = parser.parse_args()

    server = serve(args.port, args.host, rate_limit_first=args.rate_limit_first)
    host, port = server.server_address[0], server.server_address[1]  # type: ignore[misc]
    print(f"mock gelbooru listening on http://{host}:{port}")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        server.shutdown()


if __name__ == "__main__":  # pragma: no cover
    main()
