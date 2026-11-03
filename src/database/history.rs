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
        // Migrate legacy schema (post_id primary key, path/timestamp/sha256 columns) to new schema.
        let has_id: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('downloads') WHERE name='id'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false);
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='downloads'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false);
        if table_exists && !has_id {
            conn.execute("ALTER TABLE downloads RENAME TO downloads_legacy", [])
                .map_err(AppError::from)?;
        }
        conn.execute_batch(SCHEMA).map_err(AppError::from)?;
        if table_exists && !has_id {
            conn.execute(
                "INSERT OR IGNORE INTO downloads (post_id,url,local_path,file_size,file_hash,downloaded_at,tags,rating,status)
                 SELECT post_id, url, COALESCE(path,''), file_size, COALESCE(sha256,''), timestamp, tags, '', status FROM downloads_legacy",
                []).map_err(AppError::from)?;
            conn.execute("DROP TABLE downloads_legacy", [])
                .map_err(AppError::from)?;
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
}
