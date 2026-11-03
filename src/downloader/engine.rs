use crate::api::models::Post;
use crate::config::Config;
use crate::database::history::{DownloadRecord, HistoryDb};
use crate::downloader::validation;
use crate::utils::error::{AppError, Result};
use crate::utils::paths::sanitize_filename;
use blake3::Hasher;
use bytes::Bytes;
use futures::StreamExt;
use indicatif::ProgressBar;
use reqwest::Client;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex as AsyncMutex, Semaphore};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct DownloadOutcome {
    pub post_id: i64,
    pub path: PathBuf,
    pub size: u64,
    pub hash: String,
    pub skipped: bool,
    pub error: Option<String>,
}

pub struct ProgressTracker {
    pub total: AtomicUsize,
    pub completed: AtomicUsize,
    pub failed: AtomicUsize,
    pub skipped: AtomicUsize,
    pub bytes: AtomicU64,
    pub label: Mutex<String>,
}

impl ProgressTracker {
    pub fn new() -> Self {
        Self {
            total: AtomicUsize::new(0),
            completed: AtomicUsize::new(0),
            failed: AtomicUsize::new(0),
            skipped: AtomicUsize::new(0),
            bytes: AtomicU64::new(0),
            label: Mutex::new("download".to_string()),
        }
    }
    pub fn reset(&self, total: usize) {
        self.total.store(total, Ordering::Relaxed);
        self.completed.store(0, Ordering::Relaxed);
        self.failed.store(0, Ordering::Relaxed);
        self.skipped.store(0, Ordering::Relaxed);
        self.bytes.store(0, Ordering::Relaxed);
        // label set by caller before reset ideally; leave as is
    }
    pub fn write_file(&self) {
        let data = serde_json::json!({
            "total": self.total.load(Ordering::Relaxed),
            "completed": self.completed.load(Ordering::Relaxed),
            "failed": self.failed.load(Ordering::Relaxed),
            "skipped": self.skipped.load(Ordering::Relaxed),
            "bytes": self.bytes.load(Ordering::Relaxed),
            "label": self.label.lock().map(|g| g.clone()).unwrap_or_else(|_| "download".to_string()),
            "updated_at": chrono::Utc::now().timestamp(),
        });
        let path = crate::utils::paths::data_dir().join("download_progress.json");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, data.to_string());
    }
}

#[derive(Clone)]
pub struct DownloadEngine {
    pub client: Client,
    pub config: Arc<Config>,
    pub db: Option<Arc<HistoryDb>>,
    pub semaphore: Arc<Semaphore>,
    pub cancel: CancellationToken,
    pub stats: Arc<AsyncMutex<Stats>>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub label: Option<String>,
    pub progress: Arc<ProgressTracker>,
}

#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub queued: usize,
    pub completed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub bytes: u64,
}

impl DownloadEngine {
    pub fn new(config: Config, db: Option<Arc<HistoryDb>>) -> Self {
        let concurrency = config.downloads.concurrency.max(1);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::REFERER,
            reqwest::header::HeaderValue::from_static("https://gelbooru.com/"),
        );
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .pool_max_idle_per_host(concurrency * 2)
            .user_agent("gelbooru-downloader/0.1")
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("http client");
        Self {
            client: http,
            config: Arc::new(config),
            db,
            semaphore: Arc::new(Semaphore::new(concurrency)),
            cancel: CancellationToken::new(),
            stats: Arc::new(AsyncMutex::new(Stats::default())),
            sort: None,
            order: None,
            label: None,
            progress: Arc::new(ProgressTracker::new()),
        }
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub async fn download_post(&self, post: Post, output_dir: Option<PathBuf>) -> DownloadOutcome {
        let _permit = match self.semaphore.acquire().await {
            Ok(p) => p,
            Err(_) => {
                return DownloadOutcome {
                    post_id: post.id,
                    path: PathBuf::new(),
                    size: 0,
                    hash: String::new(),
                    skipped: false,
                    error: Some("semaphore closed".into()),
                };
            }
        };
        if self.cancel.is_cancelled() {
            return DownloadOutcome {
                post_id: post.id,
                path: PathBuf::new(),
                size: 0,
                hash: String::new(),
                skipped: false,
                error: Some("cancelled".into()),
            };
        }

        let dir = output_dir.unwrap_or_else(|| self.config.download_dir());
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            return DownloadOutcome {
                post_id: post.id,
                path: dir,
                size: 0,
                hash: String::new(),
                skipped: false,
                error: Some(e.to_string()),
            };
        }
        // filename
        let ext = if post.extension().is_empty() {
            "jpg".to_string()
        } else {
            post.extension()
        };
        let sort_prefix = match &self.sort {
            Some(key) if key.eq_ignore_ascii_case("score") => {
                let desc = self
                    .order
                    .as_deref()
                    .map(|o| o.eq_ignore_ascii_case("desc"))
                    .unwrap_or(true);
                if desc {
                    format!("{:012}_", 2_000_000_000i64.saturating_sub(post.score))
                } else {
                    format!("{:012}_", post.score.saturating_add(1_000_000_000))
                }
            }
            Some(key) if key.eq_ignore_ascii_case("id") => {
                let desc = self
                    .order
                    .as_deref()
                    .map(|o| o.eq_ignore_ascii_case("desc"))
                    .unwrap_or(true);
                if desc {
                    format!("{:020}_", u128::MAX - post.id as u128)
                } else {
                    format!("{:020}_", post.id as u64)
                }
            }
            _ => String::new(),
        };
        let filename = format!(
            "{}{}_{}.{}",
            sort_prefix,
            post.id,
            sanitize_filename(&post.md5),
            ext
        );
        let final_path = dir.join(&filename);

        // duplicate check by post_id (only skip if the file still exists on disk)
        if self.config.downloads.skip_duplicates {
            if let Some(db) = &self.db {
                if db.exists_post(post.id).await.unwrap_or(false) && final_path.exists() {
                    let mut st = self.stats.lock().await;
                    st.skipped += 1;
                    return DownloadOutcome {
                        post_id: post.id,
                        path: final_path,
                        size: 0,
                        hash: String::new(),
                        skipped: true,
                        error: None,
                    };
                }
            }
        }
        // if file already exists on disk, skip
        if final_path.exists() {
            if self.config.downloads.skip_duplicates {
                let mut st = self.stats.lock().await;
                st.skipped += 1;
                return DownloadOutcome {
                    post_id: post.id,
                    path: final_path,
                    size: 0,
                    hash: String::new(),
                    skipped: true,
                    error: None,
                };
            }
        }

        let url = post.file_url.clone();
        if url.is_empty() {
            return DownloadOutcome {
                post_id: post.id,
                path: final_path,
                size: 0,
                hash: String::new(),
                skipped: false,
                error: Some("no file_url".into()),
            };
        }

        let part_path = dir.join(format!("{}.part", filename));
        let mut result = self.download_file(&url, &final_path, &part_path).await;
        let mut retries = 0;
        while result.is_err() && retries < 5 {
            // A cancelled download should stop immediately instead of being
            // retried with backoff (important for the interactive viewer).
            if self.cancel.is_cancelled() {
                break;
            }
            let e = result.as_ref().unwrap_err();
            // don't retry validation errors
            if matches!(e, AppError::Validation(_)) {
                break;
            }
            retries += 1;
            tracing::warn!("retrying {} (attempt {}): {}", post.id, retries, e);
            tokio::time::sleep(std::time::Duration::from_secs(1 << retries.min(3))).await;
            result = self.download_file(&url, &final_path, &part_path).await;
        }
        match result {
            Ok((size, hash)) => {
                // duplicate by hash
                if self.config.downloads.skip_duplicates {
                    if let Some(db) = &self.db {
                        if db.exists_hash(&hash).await.unwrap_or(false) {
                            // remove file? keep but mark duplicate? We'll delete to avoid dup
                            let _ = tokio::fs::remove_file(&final_path).await;
                            let mut st = self.stats.lock().await;
                            st.skipped += 1;
                            return DownloadOutcome {
                                post_id: post.id,
                                path: final_path,
                                size,
                                hash,
                                skipped: true,
                                error: None,
                            };
                        }
                    }
                }
                // record history
                if let Some(db) = &self.db {
                    let rec = DownloadRecord {
                        id: None,
                        post_id: post.id,
                        url: url.clone(),
                        local_path: final_path.to_string_lossy().to_string(),
                        file_size: size as i64,
                        file_hash: hash.clone(),
                        downloaded_at: crate::database::history::now(),
                        tags: post.tags.clone(),
                        rating: post.rating.clone(),
                        status: "completed".to_string(),
                    };
                    let _ = db.insert(rec).await;
                }
                let mut st = self.stats.lock().await;
                st.completed += 1;
                st.bytes += size;
                DownloadOutcome {
                    post_id: post.id,
                    path: final_path,
                    size,
                    hash,
                    skipped: false,
                    error: None,
                }
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(&part_path).await;
                let msg = e.to_string();
                let mut st = self.stats.lock().await;
                st.failed += 1;
                DownloadOutcome {
                    post_id: post.id,
                    path: final_path,
                    size: 0,
                    hash: String::new(),
                    skipped: false,
                    error: Some(msg),
                }
            }
        }
    }

    async fn download_file(
        &self,
        url: &str,
        final_path: &Path,
        part_path: &Path,
    ) -> Result<(u64, String)> {
        use reqwest::header::{CONTENT_TYPE, RANGE};
        // Defensive: recreate directories if they were removed mid-download.
        if let Some(parent) = part_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        if let Some(parent) = final_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        // determine resume offset
        let existing_bytes = if self.config.downloads.resume && part_path.exists() {
            tokio::fs::read(part_path).await.ok()
        } else {
            None
        };
        let resume_from: u64 = existing_bytes.as_ref().map(|b| b.len() as u64).unwrap_or(0);
        // Manual redirect handling so the Referer header is preserved (Gelbooru hotlink protection).
        let mut current_url = url.to_string();
        let resp = loop {
            let mut req = self
                .client
                .get(&current_url)
                .header(reqwest::header::REFERER, "https://gelbooru.com/");
            if resume_from > 0 {
                req = req.header(RANGE, format!("bytes={}-", resume_from));
            }
            let r = req
                .send()
                .await
                .map_err(|e| AppError::Http(e.to_string()))?;
            if r.status().is_redirection() {
                if let Some(loc) = r.headers().get(reqwest::header::LOCATION) {
                    let loc = loc.to_str().unwrap_or("").to_string();
                    if loc.starts_with("http") {
                        current_url = loc;
                    } else {
                        // relative
                        if let Ok(base) = url::Url::parse(&current_url) {
                            if let Ok(joined) = base.join(&loc) {
                                current_url = joined.to_string();
                            }
                        }
                    }
                    continue;
                }
            }
            break r;
        };
        let status = resp.status();
        if !status.is_success() && status.as_u16() != 206 {
            return Err(AppError::Http(format!("HTTP {}", status)));
        }
        // content type check
        if let Some(ct) = resp.headers().get(CONTENT_TYPE) {
            let cts = ct.to_str().unwrap_or("");
            if self.config.downloads.validate_files {
                if !(cts.starts_with("image/")
                    || cts.starts_with("video/")
                    || cts == "application/octet-stream")
                {
                    return Err(AppError::Validation(format!("bad content-type: {}", cts)));
                }
            }
        }
        let mut hasher = Hasher::new();
        let mut total = resume_from;
        let mut file = if resume_from > 0 && status.as_u16() == 206 {
            if let Some(existing) = &existing_bytes {
                hasher.update(existing);
            }
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(part_path)
                .await
                .map_err(AppError::Io)?
        } else {
            total = 0;
            tokio::fs::File::create(part_path)
                .await
                .map_err(AppError::Io)?
        };

        let mut stream = resp.bytes_stream();
        let bandwidth = self.config.downloads.bandwidth_limit;
        while let Some(chunk) = stream.next().await {
            if self.cancel.is_cancelled() {
                return Err(AppError::Cancelled);
            }
            let chunk: Bytes = chunk.map_err(|e| AppError::Http(e.to_string()))?;
            file.write_all(&chunk).await.map_err(AppError::Io)?;
            hasher.update(&chunk);
            total += chunk.len() as u64;
            if let Some(limit) = bandwidth {
                if limit > 0 {
                    let delay = std::time::Duration::from_millis(
                        (chunk.len() as u64 * 1000 / limit).max(1),
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
        file.flush().await.map_err(AppError::Io)?;
        drop(file);

        // validation: read first bytes
        if self.config.downloads.validate_files {
            let mut f = tokio::fs::File::open(part_path)
                .await
                .map_err(AppError::Io)?;
            use tokio::io::AsyncReadExt;
            let mut buf = vec![0u8; 16];
            let n = f.read(&mut buf).await.map_err(AppError::Io)?;
            validation::validate_signature(&buf[..n])?;
            let ext = validation::extension_from_bytes(&buf[..n]);
            // if ext differs, rename? keep original ext; not critical
            let _ = ext;
        }

        let hash = hasher.finalize().to_hex().to_string();
        tokio::fs::rename(part_path, final_path)
            .await
            .map_err(AppError::Io)?;
        Ok((total, hash))
    }

    pub async fn download_many(
        &self,
        posts: Vec<Post>,
        output_dir: Option<PathBuf>,
        progress: Option<ProgressBar>,
    ) -> Vec<DownloadOutcome> {
        if let Some(pb) = &progress {
            pb.set_length(posts.len() as u64);
        }
        if let Ok(mut g) = self.progress.label.lock() {
            *g = self.label.clone().unwrap_or_else(|| "download".to_string());
        }
        self.progress.reset(posts.len());
        self.progress.write_file();
        let progress_tracker = self.progress.clone();
        let mut futs = Vec::new();
        for p in posts {
            let eng = self.clone();
            let od = output_dir.clone();
            let pb = progress.clone();
            let pt = progress_tracker.clone();
            futs.push(tokio::spawn(async move {
                let res = eng.download_post(p, od).await;
                if res.error.is_none() && !res.skipped {
                    pt.completed.fetch_add(1, Ordering::Relaxed);
                    pt.bytes.fetch_add(res.size, Ordering::Relaxed);
                } else if res.skipped {
                    pt.skipped.fetch_add(1, Ordering::Relaxed);
                } else {
                    pt.failed.fetch_add(1, Ordering::Relaxed);
                }
                pt.write_file();
                if let Some(b) = &pb {
                    b.inc(1);
                }
                res
            }));
        }
        let mut out = Vec::new();
        for f in futs {
            match f.await {
                Ok(o) => out.push(o),
                Err(e) => tracing::error!("task join error: {}", e),
            }
        }
        out
    }
}
