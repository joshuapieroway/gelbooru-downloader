"""SQLite-backed download history.

The database is the authoritative record of what has already been downloaded, so
re-running the same command does not re-fetch files. All access goes through a
single connection guarded by a re-entrant lock, with WAL enabled, which keeps
concurrent asyncio download tasks safe (SQLite is called synchronously from the
event loop, but the lock also makes the class safe to share across threads).
"""

from __future__ import annotations

import contextlib
import logging
import sqlite3
import threading
from collections.abc import Iterable, Iterator, Sequence
from pathlib import Path
from typing import Any

from .exceptions import HistoryError
from .models import (
    DownloadResult,
    DownloadStatus,
    HistoryEntry,
    HistoryStats,
    utc_now_iso,
)

__all__ = ["HistoryDatabase"]

log = logging.getLogger("gelbooru.history")

SCHEMA_VERSION = 1

_SCHEMA = """
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS downloads (
    post_id     INTEGER PRIMARY KEY,
    url         TEXT,
    path        TEXT,
    timestamp   TEXT NOT NULL,
    file_size   INTEGER NOT NULL DEFAULT 0,
    status      TEXT NOT NULL,
    tags        TEXT NOT NULL DEFAULT '',
    sha256      TEXT,
    attempts    INTEGER NOT NULL DEFAULT 0,
    error       TEXT
);

CREATE INDEX IF NOT EXISTS idx_downloads_status ON downloads(status);
CREATE INDEX IF NOT EXISTS idx_downloads_sha ON downloads(sha256);
CREATE INDEX IF NOT EXISTS idx_downloads_timestamp ON downloads(timestamp);
"""

_COLUMNS = (
    "post_id",
    "url",
    "path",
    "timestamp",
    "file_size",
    "status",
    "tags",
    "sha256",
    "attempts",
    "error",
)

#: The insert/upsert statement is assembled from module constants only — no user
#: input ever reaches the SQL text; every value travels as a bound parameter.
_COLUMNS_SQL = ", ".join(_COLUMNS)
_UPSERT_SQL = (
    f"INSERT INTO downloads ({_COLUMNS_SQL}) "  # noqa: S608 - static column list
    "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) "
    "ON CONFLICT(post_id) DO UPDATE SET "
    "url = excluded.url, path = excluded.path, timestamp = excluded.timestamp, "
    "file_size = excluded.file_size, status = excluded.status, tags = excluded.tags, "
    "sha256 = COALESCE(excluded.sha256, downloads.sha256), "
    "attempts = excluded.attempts, error = excluded.error"
)


class HistoryDatabase:
    """Small, synchronous wrapper around the ``downloads`` table.

    Thread safety: a SQLite connection may not be shared between threads while a
    statement is running, so every thread gets its own connection (created
    lazily). WAL mode lets readers run while a writer commits, a ``busy_timeout``
    absorbs lock contention, and writes are additionally serialised with a
    process-wide lock so concurrent download tasks never raise ``SQLITE_BUSY``.
    """

    def __init__(self, path: Path | str, *, busy_timeout: float = 5.0) -> None:
        self.path = Path(path)
        self._busy_timeout = busy_timeout
        self._lock = threading.RLock()
        self._local = threading.local()
        self._connections: list[sqlite3.Connection] = []
        self._closed = False
        if str(self.path) != ":memory:":
            try:
                self.path.parent.mkdir(parents=True, exist_ok=True)
            except OSError as exc:
                msg = f"cannot create download history directory {self.path.parent}: {exc}"
                raise HistoryError(msg) from exc
        self._initialise(self._conn)

    # -- connections ------------------------------------------------------- #
    def _open(self) -> sqlite3.Connection:
        """Create a configured connection for the calling thread."""

        try:
            connection = sqlite3.connect(
                str(self.path), check_same_thread=False, timeout=self._busy_timeout
            )
            connection.row_factory = sqlite3.Row
            connection.execute(f"PRAGMA busy_timeout = {int(self._busy_timeout * 1000)}")
            connection.execute("PRAGMA synchronous = NORMAL")
            if str(self.path) != ":memory:":
                connection.execute("PRAGMA journal_mode = WAL")
        except sqlite3.Error as exc:  # pragma: no cover - unwritable location
            msg = f"cannot open download history database at {self.path}: {exc}"
            raise HistoryError(msg) from exc
        with self._lock:
            self._connections.append(connection)
        return connection

    @property
    def _conn(self) -> sqlite3.Connection:
        """This thread's connection, opened on first use."""

        if self._closed:  # pragma: no cover - defensive
            msg = "history database is closed"
            raise HistoryError(msg)
        connection: sqlite3.Connection | None = getattr(self._local, "connection", None)
        if connection is None:
            connection = self._open()
            self._local.connection = connection
        return connection

    # -- lifecycle --------------------------------------------------------- #
    def _initialise(self, connection: sqlite3.Connection) -> None:
        with self._lock:
            try:
                connection.executescript(_SCHEMA)
                connection.execute(
                    "INSERT OR IGNORE INTO meta(key, value) VALUES('schema_version', ?)",
                    (str(SCHEMA_VERSION),),
                )
                connection.commit()
            except sqlite3.Error as exc:
                msg = f"cannot initialise download history database: {exc}"
                raise HistoryError(msg) from exc

    def close(self) -> None:
        with self._lock:
            if self._closed:
                return
            self._closed = True
            connections, self._connections = self._connections, []
            for connection in connections:
                with contextlib.suppress(sqlite3.Error):
                    connection.commit()
                with contextlib.suppress(sqlite3.Error):
                    connection.close()
            self._local = threading.local()

    def __enter__(self) -> HistoryDatabase:
        return self

    def __exit__(self, *exc_info: object) -> None:
        self.close()

    # -- internals --------------------------------------------------------- #
    def _execute(self, sql: str, params: Sequence[Any] = ()) -> sqlite3.Cursor:
        if self._closed:  # pragma: no cover - defensive
            msg = "history database is closed"
            raise HistoryError(msg)
        with self._lock:
            try:
                return self._conn.execute(sql, tuple(params))
            except sqlite3.Error as exc:
                msg = f"history database error: {exc}"
                raise HistoryError(msg) from exc

    def _write(self, sql: str, params: Sequence[Any] = ()) -> None:
        """Run a statement and commit it while holding the write lock."""

        with self._lock:
            connection = self._conn
            try:
                cursor = connection.execute(sql, tuple(params))
                connection.commit()
            except sqlite3.Error as exc:  # pragma: no cover - disk full etc.
                msg = f"cannot write to download history: {exc}"
                raise HistoryError(msg) from exc
            cursor.close()

    @staticmethod
    def _to_entry(row: sqlite3.Row | None) -> HistoryEntry | None:
        if row is None:
            return None
        return HistoryEntry(
            post_id=int(row["post_id"]),
            url=row["url"],
            path=row["path"],
            timestamp=row["timestamp"],
            file_size=int(row["file_size"] or 0),
            status=str(row["status"]),
            tags=row["tags"] or "",
            sha256=row["sha256"],
            attempts=int(row["attempts"] or 0),
            error=row["error"],
        )

    # -- queries ----------------------------------------------------------- #
    def get(self, post_id: int) -> HistoryEntry | None:
        """Return the history entry for ``post_id`` if there is one."""

        row = self._execute("SELECT * FROM downloads WHERE post_id = ?", (int(post_id),)).fetchone()
        return self._to_entry(row)

    def status_of(self, post_id: int) -> DownloadStatus | None:
        entry = self.get(post_id)
        if entry is None:
            return None
        try:
            return DownloadStatus(entry.status)
        except ValueError:
            return None

    def is_downloaded(self, post_id: int, path: Path | str | None = None) -> bool:
        """Whether a post was already downloaded *and* its file still exists."""

        entry = self.get(post_id)
        if entry is None or entry.status != DownloadStatus.COMPLETED.value:
            return False
        if path is not None:
            return Path(path).exists()
        if entry.path:
            return Path(entry.path).exists()
        # Recorded without a path: trust the row.
        return True

    def find_by_hash(self, sha256: str) -> HistoryEntry | None:
        """Find a previously completed download with the same content hash."""

        row = self._execute(
            "SELECT * FROM downloads WHERE sha256 = ? AND status = ? ORDER BY timestamp LIMIT 1",
            (sha256, DownloadStatus.COMPLETED.value),
        ).fetchone()
        return self._to_entry(row)

    def history(
        self,
        *,
        limit: int | None = None,
        offset: int = 0,
        status: str | None = None,
    ) -> list[HistoryEntry]:
        """Return recorded downloads, newest first."""

        sql = "SELECT * FROM downloads"
        params: list[Any] = []
        if status:
            sql += " WHERE status = ?"
            params.append(status)
        sql += " ORDER BY timestamp DESC, post_id DESC"
        if limit is not None:
            sql += " LIMIT ? OFFSET ?"
            params.extend([int(limit), int(offset)])
        rows = self._execute(sql, params).fetchall()
        return [entry for row in rows if (entry := self._to_entry(row)) is not None]

    def iter_history(self, batch_size: int = 500) -> Iterator[HistoryEntry]:
        """Stream every row without loading the whole table into memory."""

        offset = 0
        while True:
            rows = self.history(limit=batch_size, offset=offset)
            if not rows:
                return
            yield from rows
            offset += len(rows)

    def stats(self) -> HistoryStats:
        """Aggregate counters and byte totals for the whole history."""

        rows = self._execute("SELECT status, file_size, timestamp FROM downloads").fetchall()
        return HistoryStats.from_rows(
            [(row["status"], row["file_size"], row["timestamp"]) for row in rows]
        )

    def paths(self) -> list[Path]:
        """Every recorded file path that is currently a completed download."""

        rows = self._execute(
            "SELECT path FROM downloads WHERE status = ? AND path IS NOT NULL",
            (DownloadStatus.COMPLETED.value,),
        ).fetchall()
        return [Path(row["path"]) for row in rows]

    # -- mutations --------------------------------------------------------- #
    def record(self, result: DownloadResult, *, tags: str | None = None) -> None:
        """Insert or update the row for a post's latest outcome."""

        self._write(
            _UPSERT_SQL,
            (
                int(result.post_id),
                result.url,
                result.path,
                utc_now_iso(),
                int(result.size),
                result.status.value,
                (tags if tags is not None else result.tags) or "",
                result.sha256,
                int(result.attempts),
                result.error,
            ),
        )

    def record_many(
        self, results: Iterable[DownloadResult], *, tags_by_post: dict[int, str] | None = None
    ) -> int:
        """Record many results in one transaction. Returns the number of rows written."""

        rows: list[tuple[Any, ...]] = []
        now = utc_now_iso()
        tags_by_post = tags_by_post or {}
        for result in results:
            rows.append(
                (
                    int(result.post_id),
                    result.url,
                    result.path,
                    now,
                    int(result.size),
                    result.status.value,
                    tags_by_post.get(result.post_id, result.tags) or "",
                    result.sha256,
                    int(result.attempts),
                    result.error,
                )
            )
        if not rows:
            return 0
        with self._lock:
            try:
                self._conn.executemany(_UPSERT_SQL, rows)
                self._conn.commit()
            except sqlite3.Error as exc:
                msg = f"cannot write to download history: {exc}"
                raise HistoryError(msg) from exc
        return len(rows)

    def delete(self, post_id: int) -> bool:
        """Remove a row; returns ``True`` when something was deleted."""

        cursor = self._execute("DELETE FROM downloads WHERE post_id = ?", (int(post_id),))
        deleted = cursor.rowcount > 0
        with self._lock:
            self._conn.commit()
        return deleted

    def clear(self) -> int:
        """Delete every row; returns the number of removed rows."""

        cursor = self._execute("DELETE FROM downloads")
        removed = cursor.rowcount
        with self._lock:
            self._conn.commit()
        return max(removed, 0)

    def forget_missing(self) -> int:
        """Drop rows whose files have disappeared from disk."""

        missing: list[tuple[int]] = []
        for entry in self.history(status=DownloadStatus.COMPLETED.value):
            if not entry.path or not Path(entry.path).exists():
                missing.append((entry.post_id,))
        if not missing:
            return 0
        with self._lock:
            try:
                self._conn.executemany("DELETE FROM downloads WHERE post_id = ?", missing)
                self._conn.commit()
            except sqlite3.Error as exc:  # pragma: no cover
                msg = f"cannot prune download history: {exc}"
                raise HistoryError(msg) from exc
        return len(missing)
