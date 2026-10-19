"""SQLite download-history tests, including concurrent access."""

from __future__ import annotations

import threading
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import pytest

from gelbooru_downloader.database import HistoryDatabase
from gelbooru_downloader.exceptions import HistoryError
from gelbooru_downloader.models import DownloadResult, DownloadStatus, HistoryStats


def completed(
    post_id: int, *, path: str | None = None, size: int = 1024, sha: str | None = None
) -> DownloadResult:
    return DownloadResult(
        post_id=post_id,
        status=DownloadStatus.COMPLETED,
        path=path or f"/tmp/downloads/{post_id}.png",
        size=size,
        sha256=sha,
        tags="blue_hair solo",
    )


class TestBasics:
    def test_schema_is_created(self, tmp_path: Path) -> None:
        db = HistoryDatabase(tmp_path / "nested" / "history.db")
        try:
            tables = {
                row[0]
                for row in db._execute(
                    "SELECT name FROM sqlite_master WHERE type='table'"
                ).fetchall()
            }
            assert {"downloads", "meta"} <= tables
        finally:
            db.close()

    def test_record_and_get(self, history: HistoryDatabase) -> None:
        history.record(completed(1), tags="blue_hair")
        entry = history.get(1)
        assert entry is not None
        assert entry.status == "completed"
        assert entry.file_size == 1024
        assert entry.tags == "blue_hair"
        assert entry.timestamp

    def test_record_is_an_upsert(self, history: HistoryDatabase) -> None:
        history.record(completed(2, size=100))
        history.record(
            DownloadResult(post_id=2, status=DownloadStatus.SKIPPED, path="/tmp/downloads/2.png"),
        )
        entry = history.get(2)
        assert entry is not None
        assert entry.status == "skipped"
        assert entry.file_size == 0

    def test_missing_entry_is_none(self, history: HistoryDatabase) -> None:
        assert history.get(999) is None
        assert history.status_of(999) is None

    def test_status_of(self, history: HistoryDatabase) -> None:
        history.record(DownloadResult(post_id=3, status=DownloadStatus.FAILED, error="boom"))
        assert history.status_of(3) is DownloadStatus.FAILED

    def test_history_is_newest_first(self, history: HistoryDatabase) -> None:
        history.record_many([completed(i) for i in range(5)])
        entries = history.history(limit=3)
        assert len(entries) == 3
        assert {entry.post_id for entry in entries} <= set(range(5))

    def test_history_filter_by_status(self, history: HistoryDatabase) -> None:
        history.record(completed(1))
        history.record(DownloadResult(post_id=2, status=DownloadStatus.FAILED, error="x"))
        assert len(history.history(status="failed")) == 1

    def test_iter_history(self, history: HistoryDatabase) -> None:
        history.record_many([completed(i) for i in range(7)])
        assert len(list(history.iter_history(batch_size=2))) == 7


class TestDownloadDetection:
    def test_is_downloaded_requires_existing_file(
        self, history: HistoryDatabase, tmp_path: Path
    ) -> None:
        target = tmp_path / "10.png"
        target.write_bytes(b"x")
        history.record(completed(10, path=str(target)))
        assert history.is_downloaded(10) is True
        target.unlink()
        assert history.is_downloaded(10) is False

    def test_is_downloaded_false_for_failures(self, history: HistoryDatabase) -> None:
        history.record(DownloadResult(post_id=11, status=DownloadStatus.FAILED, error="nope"))
        assert history.is_downloaded(11) is False

    def test_is_downloaded_with_explicit_path(
        self, history: HistoryDatabase, tmp_path: Path
    ) -> None:
        target = tmp_path / "12.png"
        target.write_bytes(b"x")
        history.record(completed(12, path=str(tmp_path / "elsewhere.png")))
        assert history.is_downloaded(12, path=target) is True
        assert history.is_downloaded(12, path=tmp_path / "elsewhere.png") is False

    def test_find_by_hash(self, history: HistoryDatabase) -> None:
        history.record(completed(20, sha="a" * 64))
        history.record(completed(21, sha="b" * 64))
        found = history.find_by_hash("a" * 64)
        assert found is not None
        assert found.post_id == 20
        assert history.find_by_hash("z" * 64) is None

    def test_forget_missing(self, history: HistoryDatabase, tmp_path: Path) -> None:
        present = tmp_path / "30.png"
        present.write_bytes(b"x")
        history.record(completed(30, path=str(present)))
        history.record(completed(31, path=str(tmp_path / "gone.png")))
        assert history.forget_missing() == 1
        assert history.get(31) is None
        assert history.get(30) is not None


class TestStats:
    def test_aggregates(self, history: HistoryDatabase) -> None:
        history.record(completed(1, size=100))
        history.record(completed(2, size=250))
        history.record(DownloadResult(post_id=3, status=DownloadStatus.SKIPPED, path="x"))
        history.record(DownloadResult(post_id=4, status=DownloadStatus.FAILED, error="x"))
        history.record(DownloadResult(post_id=5, status=DownloadStatus.UNAVAILABLE, error="x"))
        stats = history.stats()
        assert stats.total == 5
        assert stats.completed == 2
        assert stats.skipped == 1
        assert stats.failed == 1
        assert stats.unavailable == 1
        assert stats.bytes_downloaded == 350
        assert stats.first_download is not None
        assert stats.last_download is not None

    def test_stats_merge(self) -> None:
        left = HistoryStats(total=2, completed=1, bytes_downloaded=10)
        right = HistoryStats(total=1, failed=1)
        merged = left.merge(right)
        assert merged.total == 3
        assert merged.completed == 1
        assert merged.failed == 1

    def test_empty_stats(self, history: HistoryDatabase) -> None:
        stats = history.stats()
        assert stats.total == 0
        assert stats.bytes_downloaded == 0


class TestMutation:
    def test_delete_and_clear(self, history: HistoryDatabase) -> None:
        history.record_many([completed(i) for i in range(3)])
        assert history.delete(2) is True
        assert history.delete(2) is False
        assert history.clear() == 2
        assert history.history() == []

    def test_paths(self, history: HistoryDatabase) -> None:
        history.record(completed(1, path="/tmp/a.png"))
        history.record(DownloadResult(post_id=2, status=DownloadStatus.FAILED))
        assert history.paths() == [Path("/tmp/a.png")]


class TestConcurrency:
    def test_threads_can_write_safely(self, tmp_path: Path) -> None:
        db = HistoryDatabase(tmp_path / "history.db")
        try:

            def write(post_id: int) -> None:
                db.record(completed(post_id))

            with ThreadPoolExecutor(max_workers=8) as pool:
                list(pool.map(write, range(200)))

            stats = db.stats()
            assert stats.total == 200
            assert stats.completed == 200
        finally:
            db.close()

    def test_mixed_readers_and_writers(self, tmp_path: Path) -> None:
        db = HistoryDatabase(tmp_path / "history.db")
        stop = threading.Event()
        errors: list[Exception] = []

        def reader() -> None:
            try:
                while not stop.is_set():
                    db.stats()
                    db.history(limit=5)
                    db.get(1)
            except Exception as exc:
                errors.append(exc)

        threads = [threading.Thread(target=reader) for _ in range(3)]
        for thread in threads:
            thread.start()
        try:
            with ThreadPoolExecutor(max_workers=4) as pool:
                list(pool.map(db.record, [completed(i) for i in range(50)]))
        finally:
            stop.set()
            for thread in threads:
                thread.join(timeout=5)
            db.close()

        assert not errors, errors

    def test_heavy_read_write_churn(self, tmp_path: Path) -> None:
        """Regression: sharing one connection across threads yielded ``None`` rows."""
        db = HistoryDatabase(tmp_path / "history.db")
        errors: list[Exception] = []
        iterations = 40

        def worker(offset: int) -> None:
            try:
                for index in range(iterations):
                    db.record(completed(offset * iterations + index))
                    db.stats()
                    db.history(limit=3)
                    db.find_by_hash("0" * 64)
            except Exception as exc:
                errors.append(exc)

        try:
            with ThreadPoolExecutor(max_workers=8) as pool:
                list(pool.map(worker, range(8)))
        finally:
            db.close()

        assert not errors, errors

        check = HistoryDatabase(tmp_path / "history.db")
        try:
            assert check.stats().total == 8 * iterations
        finally:
            check.close()


class TestFailureModes:
    def test_operations_after_close_raise(self, tmp_path: Path) -> None:
        db = HistoryDatabase(tmp_path / "history.db")
        db.close()
        with pytest.raises(HistoryError, match="closed"):
            db.get(1)

    def test_close_is_idempotent(self, tmp_path: Path) -> None:
        db = HistoryDatabase(tmp_path / "history.db")
        db.close()
        db.close()

    def test_unwritable_location_raises(self, tmp_path: Path) -> None:
        blocker = tmp_path / "blocked"
        blocker.write_text("not a directory")
        with pytest.raises(HistoryError):
            HistoryDatabase(blocker / "sub" / "history.db")

    def test_context_manager_closes(self, tmp_path: Path) -> None:
        with HistoryDatabase(tmp_path / "history.db") as db:
            db.record(completed(1))
        with pytest.raises(HistoryError):
            db.get(1)
