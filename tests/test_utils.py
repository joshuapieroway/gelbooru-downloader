"""Utility helpers: filenames, sizes, tag expressions, retries, timestamps."""

from __future__ import annotations

import random
from datetime import UTC, datetime, timedelta
from email.utils import format_datetime
from pathlib import Path

import pytest

from gelbooru_downloader.utils import (
    RetryPolicy,
    atomic_write_bytes,
    compute_backoff,
    content_type_to_extension,
    dedupe_preserve_order,
    extension_from_url,
    format_timestamp,
    human_bytes,
    human_duration,
    human_number,
    is_http_url,
    parse_retry_after,
    parse_size,
    parse_tag_expression,
    sanitize_filename,
    sanitize_path_component,
    sha256_file,
    shuffled,
    truncate_tags,
)


class TestSanitizeFilename:
    @pytest.mark.parametrize(
        ("raw", "expected"),
        [
            ("normal.png", "normal.png"),
            ("../../etc/passwd", "_.._etc_passwd"),
            ("/absolute/path.png", "_absolute_path.png"),
            ("back\\slash.png", "back_slash.png"),
            ("null\x00byte.png", "nullbyte.png"),
            ("with:colon?.png", "with_colon_.png"),
            ("  spaces  .png", "spaces .png"),
            ("..", "file"),
            ("", "file"),
            ("con.png", "_con.png"),
            ("nul", "_nul"),
            ("trailing.", "trailing"),
            ("日本語ファイル.png", "日本語ファイル.png"),
        ],
    )
    def test_names_are_made_safe(self, raw: str, expected: str) -> None:
        assert sanitize_filename(raw) == expected

    def test_control_characters_are_removed(self) -> None:
        assert sanitize_filename("a\nb\tc.png") == "abc.png"

    def test_long_names_are_truncated_keeping_extension(self) -> None:
        result = sanitize_filename("x" * 300 + ".png")
        assert len(result) <= 120
        assert result.endswith(".png")

    def test_path_component_has_shorter_limit(self) -> None:
        assert len(sanitize_path_component("y" * 200)) <= 64

    def test_windows_drive_and_altsep(self) -> None:
        assert "/" not in sanitize_filename("a/b/c")
        assert "\\" not in sanitize_filename("a/b/c")


class TestSizes:
    @pytest.mark.parametrize(
        ("raw", "expected"),
        [
            ("1", 1),
            ("512", 512),
            ("1KB", 1000),
            ("1KiB", 1024),
            ("5 MB", 5_000_000),
            ("1.5GiB", int(1.5 * 1024**3)),
            ("2gb", 2_000_000_000),
            ("1T", 10**12),
        ],
    )
    def test_parse_size(self, raw: str, expected: int) -> None:
        assert parse_size(raw) == expected

    @pytest.mark.parametrize("raw", ["", "abc", "5XB", "-3MB", "MB"])
    def test_invalid_sizes_raise(self, raw: str) -> None:
        with pytest.raises(ValueError):
            parse_size(raw)

    def test_negative_numbers_raise(self) -> None:
        with pytest.raises(ValueError):
            parse_size(-1)

    def test_human_bytes(self) -> None:
        assert human_bytes(0) == "0 B"
        assert human_bytes(1023) == "1023 B"
        assert human_bytes(1024) == "1.0 KiB"
        assert human_bytes(1024**3) == "1.0 GiB"
        assert human_bytes(None) == "unknown"

    def test_human_number(self) -> None:
        assert human_number(1234567) == "1,234,567"
        assert human_number(None) == "?"

    def test_human_duration(self) -> None:
        assert human_duration(0.25) == "250ms"
        assert human_duration(5).endswith("s")
        assert "m " in human_duration(75)
        assert human_duration(3700).startswith("1h")


class TestTagExpressions:
    def test_simple_tags(self) -> None:
        query = parse_tag_expression("blue_hair solo")
        assert query.include == ("blue_hair", "solo")
        assert query.exclude == ()
        assert query.search_tags == "blue_hair solo"

    def test_exclusions_with_dash_and_bang(self) -> None:
        query = parse_tag_expression("solo -nude !blurry")
        assert query.include == ("solo",)
        assert query.exclude == ("nude", "blurry")
        assert query.search_tags == "solo -nude -blurry"

    def test_metatags_are_kept(self) -> None:
        query = parse_tag_expression("solo rating:safe order:score")
        assert "rating:safe" in query.include
        assert "order:score" in query.include

    def test_negated_metatag_stays_negated(self) -> None:
        # "-rating:safe" must exclude safe posts, not select them.
        query = parse_tag_expression("solo -rating:safe")
        assert query.include == ("solo",)
        assert query.exclude == ("rating:safe",)
        assert query.search_tags == "solo -rating:safe"

    def test_bang_negated_metatag(self) -> None:
        query = parse_tag_expression("solo !rating:explicit")
        assert query.exclude == ("rating:explicit",)
        assert query.search_tags == "solo -rating:explicit"

    def test_duplicates_removed(self) -> None:
        assert parse_tag_expression("a a b").include == ("a", "b")

    def test_empty_expression(self) -> None:
        assert not parse_tag_expression("")
        assert not parse_tag_expression(None)
        assert parse_tag_expression(None).tags == ""

    def test_truncate_tags(self) -> None:
        assert truncate_tags("a b c d e", 3) == "a b c"
        assert truncate_tags("a b", 5) == "a b"


class TestUrls:
    @pytest.mark.parametrize(
        ("url", "expected"),
        [
            ("https://cdn.example.invalid/image/123.png", "png"),
            ("https://cdn.example.invalid/image/123.PNG?x=1", "png"),
            ("https://cdn.example.invalid/image/1234567", None),
            ("https://cdn.example.invalid/path.tar.gz", "gz"),
            ("", None),
        ],
    )
    def test_extension_from_url(self, url: str, expected: str | None) -> None:
        assert extension_from_url(url) == expected

    @pytest.mark.parametrize(
        ("content_type", "expected"),
        [
            ("image/jpeg", "jpg"),
            ("image/jpeg; charset=binary", "jpg"),
            ("image/png", "png"),
            ("video/webm", "webm"),
            ("application/octet-stream", "bin"),
            (None, "jpg"),
            ("weird/unknown-type", "jpg"),
        ],
    )
    def test_content_type_to_extension(self, content_type: str | None, expected: str) -> None:
        assert content_type_to_extension(content_type) == expected

    @pytest.mark.parametrize(
        ("url", "expected"),
        [
            ("https://a.invalid/x.png", True),
            ("http://a.invalid/x.png", True),
            ("ftp://a.invalid/x.png", False),
            ("file:///etc/passwd", False),
            ("/relative.png", False),
            ("", False),
        ],
    )
    def test_is_http_url(self, url: str, expected: bool) -> None:
        assert is_http_url(url) is expected


class TestRetryAfter:
    def test_seconds(self) -> None:
        assert parse_retry_after("5") == 5.0
        assert parse_retry_after(7) == 7.0
        assert parse_retry_after(2.5) == 2.5

    def test_http_date(self) -> None:
        target = datetime.now(UTC) + timedelta(seconds=30)
        value = parse_retry_after(format_datetime(target, usegmt=True))
        assert value is not None
        assert 25 <= value <= 35

    def test_past_date_is_zero(self) -> None:
        target = datetime.now(UTC) - timedelta(seconds=30)
        assert parse_retry_after(format_datetime(target, usegmt=True)) == 0.0

    def test_missing_or_garbage(self) -> None:
        assert parse_retry_after(None) is None
        assert parse_retry_after("") is None
        assert parse_retry_after("soon please") is None


class TestBackoff:
    def test_exponential_growth(self) -> None:
        rng = random.Random(0)
        delays = [compute_backoff(attempt, jitter=0, rng=rng) for attempt in (1, 2, 3, 4)]
        assert delays == [0.5, 1.0, 2.0, 4.0]

    def test_capped_at_max(self) -> None:
        assert compute_backoff(10, base_delay=1, multiplier=10, max_delay=5, jitter=0) == 5

    def test_jitter_stays_in_bounds(self) -> None:
        rng = random.Random(42)
        for _ in range(50):
            delay = compute_backoff(2, base_delay=1, jitter=0.2, rng=rng)
            assert 1.6 <= delay <= 2.4

    def test_policy_helper(self) -> None:
        policy = RetryPolicy(attempts=4, base_delay=0.25, multiplier=2, jitter=0)
        assert policy.delay_for(1) == 0.25
        assert policy.delay_for(3) == 1.0


class TestFilesystem:
    def test_atomic_write_replaces_content(self, tmp_path: Path) -> None:
        target = tmp_path / "file.json"
        atomic_write_bytes(target, b"first")
        atomic_write_bytes(target, b"second")
        assert target.read_bytes() == b"second"
        assert list(tmp_path.glob(".*tmp*")) == []

    def test_atomic_write_creates_parents_and_mode(self, tmp_path: Path) -> None:
        target = tmp_path / "deep" / "nested" / "file.txt"
        atomic_write_bytes(target, b"data", mode=0o600)
        assert target.read_bytes() == b"data"
        assert oct(target.stat().st_mode)[-3:] == "600"

    def test_sha256_file(self, tmp_path: Path) -> None:
        target = tmp_path / "blob.bin"
        target.write_bytes(b"abc")
        digest = sha256_file(target)
        assert digest == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"


class TestMisc:
    def test_dedupe_preserves_order(self) -> None:
        assert dedupe_preserve_order(["b", "a", "b", "c"]) == ["b", "a", "c"]

    def test_shuffled_keeps_all_items(self) -> None:
        items = list(range(20))
        result = shuffled(items, seed=7)
        assert sorted(result) == items
        assert result != items or len(items) <= 1

    def test_shuffled_is_deterministic_with_seed(self) -> None:
        assert shuffled(list(range(10)), seed=1) == shuffled(list(range(10)), seed=1)

    def test_format_timestamp_accepts_iso_and_epoch(self) -> None:
        assert format_timestamp("2026-01-02T03:04:05+00:00") != "unknown"
        assert format_timestamp(1700000000) != "unknown"
        assert format_timestamp(None) == "unknown"
        assert format_timestamp("not a timestamp") == "not a timestamp"
