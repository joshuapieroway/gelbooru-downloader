use crate::api::models::Post;
use crate::database::migrations::SCHEMA;
use crate::utils::error::{AppError, Result};
use chrono::Utc;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadRecord {
    pub id: Option<i64>,
    pub post_id: i64,
    pub url: String,
    pub local_path: String,
    pub file_size: i64,
    pub file_hash: String,
    pub downloaded_at: String,
    pub tags: String,
    pub rating: String,
    pub status: String,
}

/// A persisted download that has not completed yet.
///
/// The full post is stored as JSON so a resume does not need the API (or a
/// still-valid search result) to reconstruct the request. Successful rows are
/// deleted; failed rows stay behind for inspection and `queue retry`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedDownload {
    pub id: i64,
    pub post_id: i64,
    pub post: Post,
    pub output_dir: String,
    pub filename_template: String,
    pub folder_template: String,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub attempts: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone)]
pub struct HistoryDb {
    conn: Arc<Mutex<Connection>>,
}

impl HistoryDb {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(AppError::Io)?;
        }
        let conn = Connection::open(path).map_err(AppError::from)?;
        conn.busy_timeout(std::time::Duration::from_secs(5)).ok();
        // Migrate legacy schema (post_id primary key, path/timestamp/sha256
        // columns) to the current schema. The rename and copy live in one
        // transaction so a crash cannot strand data in `downloads_legacy`.
        let downloads_exists = table_exists(&conn, "downloads")?;
        let has_id = downloads_exists && has_column(&conn, "downloads", "id")?;
        if downloads_exists && !has_id {
            let tx = conn.unchecked_transaction().map_err(AppError::from)?;
            tx.execute("ALTER TABLE downloads RENAME TO downloads_legacy", [])
                .map_err(AppError::from)?;
            tx.execute_batch(SCHEMA).map_err(AppError::from)?;
            migrate_legacy(&tx)?;
            tx.commit().map_err(AppError::from)?;
        } else {
            conn.execute_batch(SCHEMA).map_err(AppError::from)?;
            // Recover from an older build that created the new table but
            // failed before copying and dropping the legacy table.
            if table_exists(&conn, "downloads_legacy")? {
                let tx = conn.unchecked_transaction().map_err(AppError::from)?;
                migrate_legacy(&tx)?;
                tx.commit().map_err(AppError::from)?;
            }
        }
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(AppError::from)?;
        conn.busy_timeout(std::time::Duration::from_secs(5)).ok();
        conn.execute_batch(SCHEMA).map_err(AppError::from)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub async fn insert(&self, rec: DownloadRecord) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            c.execute(
                "INSERT INTO downloads (post_id,url,local_path,file_size,file_hash,downloaded_at,tags,rating,status)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
                 ON CONFLICT(post_id) DO UPDATE SET url=excluded.url, local_path=excluded.local_path,
                    file_size=excluded.file_size, file_hash=excluded.file_hash,
                    downloaded_at=excluded.downloaded_at, tags=excluded.tags,
                    rating=excluded.rating, status=excluded.status",
                params![rec.post_id, rec.url, rec.local_path, rec.file_size, rec.file_hash,
                        rec.downloaded_at, rec.tags, rec.rating, rec.status],
            ).map_err(AppError::from)?;
            Ok(())
        }).await.map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    pub async fn exists_post(&self, post_id: i64) -> Result<bool> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            let count: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM downloads WHERE post_id=?1 AND status='completed'",
                    params![post_id],
                    |r| r.get(0),
                )
                .map_err(AppError::from)?;
            Ok(count > 0)
        })
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    pub async fn exists_hash(&self, hash: &str) -> Result<bool> {
        let conn = self.conn.clone();
        let hash = hash.to_string();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            let count: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM downloads WHERE file_hash=?1 AND file_hash<>''",
                    params![hash],
                    |r| r.get(0),
                )
                .map_err(AppError::from)?;
            Ok(count > 0)
        })
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    /// Path recorded for a completed post, if any. Used to skip the post even
    /// when a newer filename template points at a different name.
    pub async fn post_path(&self, post_id: i64) -> Result<Option<String>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            let res = c.query_row(
                "SELECT local_path FROM downloads
                 WHERE post_id=?1 AND status='completed'
                 ORDER BY downloaded_at DESC LIMIT 1",
                params![post_id],
                |r| r.get(0),
            );
            match res {
                Ok(path) => Ok(Some(path)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(AppError::from(e)),
            }
        })
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    /// Path of one completed download with this hash, if any. Used to skip a
    /// duplicate only when the existing copy is still on disk.
    pub async fn hash_path(&self, hash: &str) -> Result<Option<String>> {
        let conn = self.conn.clone();
        let hash = hash.to_string();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            let res = c.query_row(
                "SELECT local_path FROM downloads
                 WHERE file_hash=?1 AND file_hash<>'' AND status='completed'
                 ORDER BY downloaded_at DESC LIMIT 1",
                params![hash],
                |r| r.get(0),
            );
            match res {
                Ok(path) => Ok(Some(path)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(AppError::from(e)),
            }
        })
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    pub async fn list(&self, limit: i64, search: Option<String>) -> Result<Vec<DownloadRecord>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            let mut out = Vec::new();
            if let Some(q) = &search {
                let pattern = format!("%{}%", q);
                let mut stmt = c.prepare("SELECT id,post_id,url,local_path,file_size,file_hash,downloaded_at,tags,rating,status FROM downloads WHERE tags LIKE ?1 OR url LIKE ?1 ORDER BY downloaded_at DESC LIMIT ?2").map_err(AppError::from)?;
                let rows = stmt.query_map(params![pattern, limit], |r| Ok((
                    r.get::<_,i64>(0)?, r.get::<_,i64>(1)?, r.get::<_,String>(2)?, r.get::<_,String>(3)?,
                    r.get::<_,i64>(4)?, r.get::<_,String>(5)?, r.get::<_,String>(6)?, r.get::<_,String>(7)?,
                    r.get::<_,String>(8)?, r.get::<_,String>(9)?))) .map_err(AppError::from)?;
                for r in rows {
                    let (id,post_id,url,local_path,file_size,file_hash,downloaded_at,tags,rating,status) = r.map_err(AppError::from)?;
                    out.push(DownloadRecord { id: Some(id), post_id, url, local_path, file_size, file_hash, downloaded_at, tags, rating, status });
                }
            } else {
                let mut stmt = c.prepare("SELECT id,post_id,url,local_path,file_size,file_hash,downloaded_at,tags,rating,status FROM downloads ORDER BY downloaded_at DESC LIMIT ?1").map_err(AppError::from)?;
                let rows = stmt.query_map(params![limit], |r| Ok((
                    r.get::<_,i64>(0)?, r.get::<_,i64>(1)?, r.get::<_,String>(2)?, r.get::<_,String>(3)?,
                    r.get::<_,i64>(4)?, r.get::<_,String>(5)?, r.get::<_,String>(6)?, r.get::<_,String>(7)?,
                    r.get::<_,String>(8)?, r.get::<_,String>(9)?))) .map_err(AppError::from)?;
                for r in rows {
                    let (id,post_id,url,local_path,file_size,file_hash,downloaded_at,tags,rating,status) = r.map_err(AppError::from)?;
                    out.push(DownloadRecord { id: Some(id), post_id, url, local_path, file_size, file_hash, downloaded_at, tags, rating, status });
                }
            }
            Ok(out)
        }).await.map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    pub async fn count(&self) -> Result<i64> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            let n: i64 = c
                .query_row("SELECT COUNT(*) FROM downloads", [], |r| r.get(0))
                .map_err(AppError::from)?;
            Ok(n)
        })
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    pub async fn delete_by_post(&self, post_id: i64) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            c.execute("DELETE FROM downloads WHERE post_id=?1", params![post_id])
                .map_err(AppError::from)?;
            Ok(())
        })
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    pub async fn clear(&self) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            c.execute("DELETE FROM downloads", [])
                .map_err(AppError::from)?;
            Ok(())
        })
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    pub async fn get_watch_state(&self, tags: &str) -> Result<i64> {
        let conn = self.conn.clone();
        let tags = tags.to_string();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            let res = c.query_row(
                "SELECT last_seen_id FROM watch_state WHERE tags=?1",
                params![tags],
                |r| r.get(0),
            );
            match res {
                Ok(v) => Ok(v),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(0),
                Err(e) => Err(AppError::from(e)),
            }
        })
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    pub async fn set_watch_state(&self, tags: &str, last_seen: i64) -> Result<()> {
        let conn = self.conn.clone();
        let tags = tags.to_string();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            c.execute(
                "INSERT INTO watch_state (tags,last_seen_id) VALUES (?1,?2)
                 ON CONFLICT(tags) DO UPDATE SET last_seen_id=excluded.last_seen_id",
                params![tags, last_seen],
            )
            .map_err(AppError::from)?;
            Ok(())
        })
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    // Presets stored in presets table
    pub async fn list_presets(&self) -> Result<Vec<(String, String)>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            let mut stmt = c
                .prepare("SELECT name,data FROM presets")
                .map_err(AppError::from)?;
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(AppError::from)?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(AppError::from)?);
            }
            Ok(out)
        })
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    pub async fn save_preset(&self, name: &str, data: &str) -> Result<()> {
        let conn = self.conn.clone();
        let name = name.to_string();
        let data = data.to_string();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            c.execute(
                "INSERT INTO presets (name,data) VALUES (?1,?2) ON CONFLICT(name) DO UPDATE SET data=excluded.data",
                params![name, data],
            ).map_err(AppError::from)?;
            Ok(())
        }).await.map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    pub async fn delete_preset(&self, name: &str) -> Result<()> {
        let conn = self.conn.clone();
        let name = name.to_string();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            c.execute("DELETE FROM presets WHERE name=?1", params![name])
                .map_err(AppError::from)?;
            Ok(())
        })
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    pub async fn get_preset(&self, name: &str) -> Result<Option<String>> {
        let conn = self.conn.clone();
        let name = name.to_string();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            let res = c.query_row(
                "SELECT data FROM presets WHERE name=?1",
                params![name],
                |r| r.get(0),
            );
            match res {
                Ok(v) => Ok(Some(v)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(AppError::from(e)),
            }
        })
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    /// Insert a pending download, or reset an existing row for the same post
    /// and output directory so a retry is a fresh attempt.
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_queue(
        &self,
        post: Post,
        output_dir: String,
        filename_template: String,
        folder_template: String,
        sort: Option<String>,
        order: Option<String>,
    ) -> Result<i64> {
        let conn = self.conn.clone();
        let post_id = post.id;
        let post_json = serde_json::to_string(&post)?;
        let now = now();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            c.execute(
                "INSERT INTO download_queue
                    (post_id,post_json,output_dir,filename_template,folder_template,sort,\"order\",status,error,attempts,created_at,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,'queued',NULL,0,?8,?8)
                 ON CONFLICT(post_id,output_dir) DO UPDATE SET
                    post_json=excluded.post_json,
                    filename_template=excluded.filename_template,
                    folder_template=excluded.folder_template,
                    sort=excluded.sort,
                    \"order\"=excluded.\"order\",
                    status='queued',
                    error=NULL,
                    updated_at=excluded.updated_at",
                params![
                    post_id,
                    post_json,
                    output_dir,
                    filename_template,
                    folder_template,
                    sort,
                    order,
                    now
                ],
            )
            .map_err(AppError::from)?;
            let id: i64 = c
                .query_row(
                    "SELECT id FROM download_queue WHERE post_id=?1 AND output_dir=?2",
                    params![post_id, output_dir],
                    |r| r.get(0),
                )
                .map_err(AppError::from)?;
            Ok(id)
        })
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    pub async fn queue_set_downloading(&self, id: i64) -> Result<()> {
        let conn = self.conn.clone();
        let now = now();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            c.execute(
                "UPDATE download_queue
                 SET status='downloading', error=NULL, attempts=attempts+1, updated_at=?1
                 WHERE id=?2",
                params![now, id],
            )
            .map_err(AppError::from)?;
            Ok(())
        })
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    pub async fn queue_set_failed(&self, id: i64, error: &str) -> Result<()> {
        let conn = self.conn.clone();
        let error = error.to_string();
        let now = now();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            c.execute(
                "UPDATE download_queue SET status='failed', error=?1, updated_at=?2 WHERE id=?3",
                params![error, now, id],
            )
            .map_err(AppError::from)?;
            Ok(())
        })
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    pub async fn queue_remove(&self, id: i64) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            c.execute("DELETE FROM download_queue WHERE id=?1", params![id])
                .map_err(AppError::from)?;
            Ok(())
        })
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    /// Rows waiting to run. `downloading` rows are included because they may
    /// belong to a process that was killed before it could reset the status.
    pub async fn queue_pending(&self) -> Result<Vec<QueuedDownload>> {
        self.queue_where("status IN ('queued','downloading')", None)
            .await
    }

    pub async fn queue_failed(&self) -> Result<Vec<QueuedDownload>> {
        self.queue_where("status='failed'", None).await
    }

    pub async fn queue_list(&self, limit: i64) -> Result<Vec<QueuedDownload>> {
        self.queue_where("1=1", Some(limit)).await
    }

    async fn queue_where(
        &self,
        predicate: &'static str,
        limit: Option<i64>,
    ) -> Result<Vec<QueuedDownload>> {
        let conn = self.conn.clone();
        let limit = limit.unwrap_or(-1);
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            let mut stmt = c
                .prepare(&format!(
                    "SELECT id,post_id,post_json,output_dir,filename_template,folder_template,
                            sort,\"order\",status,error,attempts,created_at,updated_at
                     FROM download_queue
                     WHERE {predicate}
                     ORDER BY CASE status WHEN 'downloading' THEN 0 WHEN 'queued' THEN 1 ELSE 2 END, id
                     LIMIT ?1"
                ))
                .map_err(AppError::from)?;
            let rows = stmt
                .query_map(params![limit], queue_row)
                .map_err(AppError::from)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(AppError::from)?);
            }
            Ok(out)
        })
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    /// Move failed rows back to `queued`. Returns how many were updated.
    pub async fn queue_retry_failed(&self) -> Result<usize> {
        let conn = self.conn.clone();
        let now = now();
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            let changed = c
                .execute(
                    "UPDATE download_queue SET status='queued', error=NULL, updated_at=?1
                     WHERE status='failed'",
                    params![now],
                )
                .map_err(AppError::from)?;
            Ok(changed)
        })
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }

    /// Delete queue rows. `status: None` clears everything.
    pub async fn queue_clear(&self, status: Option<&str>) -> Result<usize> {
        let conn = self.conn.clone();
        let status = status.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            let c = conn.lock().unwrap();
            let changed = match status {
                Some(status) => c.execute(
                    "DELETE FROM download_queue WHERE status=?1",
                    params![status],
                ),
                None => c.execute("DELETE FROM download_queue", []),
            }
            .map_err(AppError::from)?;
            Ok(changed)
        })
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!(e)))?
    }
}

fn queue_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueuedDownload> {
    let post_json: String = row.get(2)?;
    Ok(QueuedDownload {
        id: row.get(0)?,
        post_id: row.get(1)?,
        post: serde_json::from_str(&post_json).unwrap_or_default(),
        output_dir: row.get(3)?,
        filename_template: row.get(4)?,
        folder_template: row.get(5)?,
        sort: row.get(6)?,
        order: row.get(7)?,
        status: row.get(8)?,
        error: row.get(9)?,
        attempts: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            params![name],
            |r| r.get(0),
        )
        .map_err(AppError::from)?;
    Ok(count > 0)
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({})", table))
        .map_err(AppError::from)?;
    let names = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(AppError::from)?;
    for name in names {
        if name.map_err(AppError::from)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn migrate_legacy(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    let path = if has_column(tx, "downloads_legacy", "path")? {
        "COALESCE(path,'')"
    } else {
        "''"
    };
    let sha = if has_column(tx, "downloads_legacy", "sha256")? {
        "COALESCE(sha256,'')"
    } else {
        "''"
    };
    let when = if has_column(tx, "downloads_legacy", "timestamp")? {
        "COALESCE(timestamp, datetime('now'))"
    } else {
        "datetime('now')"
    };
    tx.execute(
        &format!(
            "INSERT OR IGNORE INTO downloads (post_id,url,local_path,file_size,file_hash,downloaded_at,tags,rating,status)
             SELECT post_id, url, {path}, file_size, {sha}, {when}, tags, '', status FROM downloads_legacy"
        ),
        [],
    )
    .map_err(AppError::from)?;
    tx.execute("DROP TABLE downloads_legacy", [])
        .map_err(AppError::from)?;
    Ok(())
}

// helper for now timestamp
pub fn now() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn insert_and_exists() {
        let db = HistoryDb::open_in_memory().unwrap();
        let rec = DownloadRecord {
            id: None,
            post_id: 42,
            url: "http://x".into(),
            local_path: "/tmp/a.jpg".into(),
            file_size: 10,
            file_hash: "abc".into(),
            downloaded_at: now(),
            tags: "t".into(),
            rating: "safe".into(),
            status: "completed".into(),
        };
        db.insert(rec).await.unwrap();
        assert!(db.exists_post(42).await.unwrap());
        assert!(db.exists_hash("abc").await.unwrap());
        assert_eq!(db.count().await.unwrap(), 1);
        let recs = db.list(10, None).await.unwrap();
        assert_eq!(recs.len(), 1);
    }
    fn record(post_id: i64, hash: &str, path: &str, at: &str) -> DownloadRecord {
        DownloadRecord {
            id: None,
            post_id,
            url: "http://x".into(),
            local_path: path.into(),
            file_size: 10,
            file_hash: hash.into(),
            downloaded_at: at.into(),
            tags: "t".into(),
            rating: "safe".into(),
            status: "completed".into(),
        }
    }

    #[tokio::test]
    async fn hash_path_finds_the_newest_copy() {
        let db = HistoryDb::open_in_memory().unwrap();
        db.insert(record(1, "same", "/tmp/old.png", "2020-01-01T00:00:00Z"))
            .await
            .unwrap();
        db.insert(record(2, "same", "/tmp/new.png", "2024-01-01T00:00:00Z"))
            .await
            .unwrap();
        assert_eq!(
            db.hash_path("same").await.unwrap().as_deref(),
            Some("/tmp/new.png")
        );
        assert_eq!(db.hash_path("missing").await.unwrap(), None);
    }

    #[tokio::test]
    async fn post_path_tracks_the_completed_file() {
        let db = HistoryDb::open_in_memory().unwrap();
        db.insert(record(7, "hash", "/tmp/7.png", "2020-01-01T00:00:00Z"))
            .await
            .unwrap();
        assert_eq!(
            db.post_path(7).await.unwrap().as_deref(),
            Some("/tmp/7.png")
        );
        assert_eq!(db.post_path(8).await.unwrap(), None);
    }

    #[tokio::test]
    async fn migrates_a_legacy_downloads_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE downloads (
                     post_id INTEGER PRIMARY KEY,
                     url TEXT NOT NULL,
                     path TEXT,
                     file_size INTEGER NOT NULL DEFAULT 0,
                     sha256 TEXT,
                     timestamp TEXT,
                     tags TEXT,
                     status TEXT NOT NULL DEFAULT 'completed'
                 );
                 INSERT INTO downloads VALUES
                     (9, 'http://x/9', '/tmp/9.png', 12, 'abc', '2020-02-02T00:00:00Z', 'tag', 'completed');",
            )
            .unwrap();
        }

        let db = HistoryDb::open(&path).unwrap();
        let recs = db.list(10, None).await.unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].post_id, 9);
        assert_eq!(recs[0].local_path, "/tmp/9.png");
        assert_eq!(recs[0].file_hash, "abc");
        assert_eq!(recs[0].downloaded_at, "2020-02-02T00:00:00Z");
        drop(db);

        // Reopening must not repeat or lose the migration.
        let db = HistoryDb::open(&path).unwrap();
        assert_eq!(db.count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn recovers_a_stranded_legacy_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute(
                "INSERT INTO downloads (post_id,url,local_path,downloaded_at)
                 VALUES (1,'http://x/1','/tmp/1.png','2023-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
            conn.execute_batch(
                "CREATE TABLE downloads_legacy (
                     post_id INTEGER PRIMARY KEY,
                     url TEXT NOT NULL,
                     path TEXT,
                     file_size INTEGER NOT NULL DEFAULT 0,
                     sha256 TEXT,
                     timestamp TEXT,
                     tags TEXT,
                     status TEXT NOT NULL DEFAULT 'completed'
                 );
                 INSERT INTO downloads_legacy VALUES
                     (2, 'http://x/2', '/tmp/2.png', 4, 'def', '2021-01-01T00:00:00Z', 'tag', 'completed');",
            )
            .unwrap();
        }

        let db = HistoryDb::open(&path).unwrap();
        assert_eq!(db.count().await.unwrap(), 2);
        let legacy_exists: bool = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='downloads_legacy'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap();
        assert!(!legacy_exists);
    }

    #[tokio::test]
    async fn watch_state() {
        let db = HistoryDb::open_in_memory().unwrap();
        assert_eq!(db.get_watch_state("tags").await.unwrap(), 0);
        db.set_watch_state("tags", 5).await.unwrap();
        assert_eq!(db.get_watch_state("tags").await.unwrap(), 5);
    }
    #[tokio::test]
    async fn presets() {
        let db = HistoryDb::open_in_memory().unwrap();
        db.save_preset("p", "{\"name\":\"p\",\"tags\":\"x\",\"limit\":10}")
            .await
            .unwrap();
        let v = db.get_preset("p").await.unwrap().unwrap();
        assert!(v.contains("\"p\""));
        let list = db.list_presets().await.unwrap();
        assert_eq!(list.len(), 1);
        db.delete_preset("p").await.unwrap();
        assert!(db.get_preset("p").await.unwrap().is_none());
    }

    fn queued_post(id: i64) -> Post {
        Post {
            id,
            md5: format!("hash{id}"),
            file_url: format!("http://x/{id}.png"),
            tags: "tag".to_string(),
            ..Post::default()
        }
    }

    #[tokio::test]
    async fn queue_round_trip_and_failure_lifecycle() {
        let db = HistoryDb::open_in_memory().unwrap();
        let id = db
            .enqueue_queue(
                queued_post(7),
                "/tmp/out".to_string(),
                "{id}.{ext}".to_string(),
                "{rating}".to_string(),
                None,
                None,
            )
            .await
            .unwrap();

        let pending = db.queue_pending().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].post_id, 7);
        assert_eq!(pending[0].post.md5, "hash7");
        assert_eq!(pending[0].output_dir, "/tmp/out");
        assert_eq!(pending[0].filename_template, "{id}.{ext}");

        db.queue_set_downloading(id).await.unwrap();
        assert_eq!(db.queue_pending().await.unwrap()[0].attempts, 1);
        assert_eq!(db.queue_pending().await.unwrap()[0].status, "downloading");

        db.queue_set_failed(id, "boom").await.unwrap();
        assert!(db.queue_pending().await.unwrap().is_empty());
        let failed = db.queue_failed().await.unwrap();
        assert_eq!(failed[0].error.as_deref(), Some("boom"));

        // Enqueuing the same post and directory reuses the row and clears the
        // error instead of creating a duplicate.
        let same_id = db
            .enqueue_queue(
                queued_post(7),
                "/tmp/out".to_string(),
                "{id}.{ext}".to_string(),
                "{rating}".to_string(),
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(same_id, id);
        assert!(db.queue_failed().await.unwrap().is_empty());
        assert_eq!(db.queue_pending().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn queue_retry_and_clear() {
        let db = HistoryDb::open_in_memory().unwrap();
        for id in [1, 2] {
            let row = db
                .enqueue_queue(
                    queued_post(id),
                    "/tmp/out".to_string(),
                    String::new(),
                    String::new(),
                    None,
                    None,
                )
                .await
                .unwrap();
            db.queue_set_failed(row, "nope").await.unwrap();
        }
        assert_eq!(db.queue_retry_failed().await.unwrap(), 2);
        assert_eq!(db.queue_pending().await.unwrap().len(), 2);
        assert_eq!(db.queue_list(10).await.unwrap().len(), 2);
        assert_eq!(db.queue_clear(Some("failed")).await.unwrap(), 0);
        assert_eq!(db.queue_clear(None).await.unwrap(), 2);
        assert!(db.queue_list(10).await.unwrap().is_empty());
    }
}
