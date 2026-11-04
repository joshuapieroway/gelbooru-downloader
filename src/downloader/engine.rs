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
use std::sync::atomic::{AtomicUsize, Ordering};
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

#[derive(Debug, Clone)]
pub struct DownloadEntry {
    pub post_id: i64,
    pub status: DownloadStatus,
    pub progress: usize,
    pub total: u64,
    pub bytes: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadStatus {
    Queued,
    Downloading,
    Completed,
    Failed,
    Cancelled,
}

pub struct DownloadQueue {
    pub entries: Arc<AsyncMutex<Vec<DownloadEntry>>>,
}

impl DownloadQueue {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(AsyncMutex::new(Vec::new())),
        }
    }

    pub async fn add_queued(&self, post_id: i64) -> usize {
        let mut entries = self.entries.lock().await;
        entries.push(DownloadEntry {
            post_id,
            status: DownloadStatus::Queued,
            progress: 0,
            total: 0,
            bytes: 0,
            error: None,
        });
        entries.len() - 1
    }

    /// Whether `post_id` is queued or currently downloading.
    pub async fn is_active(&self, post_id: i64) -> bool {
        self.entries.lock().await.iter().any(|e| {
            e.post_id == post_id
                && matches!(
                    e.status,
                    DownloadStatus::Queued | DownloadStatus::Downloading
                )
        })
    }

    pub async fn set_status(&self, post_id: i64, status: DownloadStatus) {
        let mut entries = self.entries.lock().await;
        for e in entries.iter_mut() {
            if e.post_id == post_id {
                e.status = status;
                break;
            }
        }
    }

    pub async fn set_progress(&self, post_id: i64, progress: usize, bytes: u64) {
        let mut entries = self.entries.lock().await;
        for e in entries.iter_mut() {
            if e.post_id == post_id {
                e.progress = progress;
                e.bytes = bytes;
                break;
            }
        }
    }

    pub async fn set_error(&self, post_id: i64, error: Option<String>) {
        let mut entries = self.entries.lock().await;
        for e in entries.iter_mut() {
            if e.post_id == post_id {
                e.error = error.clone();
                if let Some(message) = &error {
                    e.status = if message == "cancelled" {
                        DownloadStatus::Cancelled
                    } else {
                        DownloadStatus::Failed
                    };
                }
                break;
            }
        }
    }

    pub async fn len(&self) -> usize {
        self.entries.lock().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.entries.lock().await.is_empty()
    }

    pub async fn entries(&self) -> Vec<DownloadEntry> {
        self.entries.lock().await.clone()
    }

    pub async fn clear_completed(&self) {
        let mut entries = self.entries.lock().await;
        entries.retain(|e| {
            e.status != DownloadStatus::Completed && e.status != DownloadStatus::Failed
        });
    }
}

impl Default for DownloadQueue {
    fn default() -> Self {
        Self::new()
    }
}

use super::progress::ProgressTracker;

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
    pub rating: Option<String>,
    pub progress: Arc<ProgressTracker>,
    pub queue: Option<Arc<DownloadQueue>>,
    /// Optional CLI progress bar advanced while response bytes stream in.
    pub progress_bar: Option<ProgressBar>,
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
        Self::with_queue(config, db, None)
    }

    pub fn with_queue(
        config: Config,
        db: Option<Arc<HistoryDb>>,
        queue: Option<Arc<DownloadQueue>>,
    ) -> Self {
        let concurrency = config.downloads.concurrency.max(1);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::REFERER,
            reqwest::header::HeaderValue::from_static("https://gelbooru.com/"),
        );
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .pool_max_idle_per_host(concurrency)
            .pool_idle_timeout(Some(std::time::Duration::from_secs(30)))
            .tcp_keepalive(Some(std::time::Duration::from_secs(10)))
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
            rating: None,
            progress: Arc::new(ProgressTracker::new()),
            queue,
            progress_bar: None,
        }
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub async fn download_post(&self, post: Post, output_dir: Option<PathBuf>) -> DownloadOutcome {
        let post_id = post.id;

        let _permit = match self.semaphore.acquire().await {
            Ok(p) => p,
            Err(_) => {
                if let Some(queue) = &self.queue {
                    queue
                        .set_error(post_id, Some("semaphore closed".into()))
                        .await;
                }
                return DownloadOutcome {
                    post_id,
                    path: PathBuf::new(),
                    size: 0,
                    hash: String::new(),
                    skipped: false,
                    error: Some("semaphore closed".into()),
                };
            }
        };
        if self.cancel.is_cancelled() {
            if let Some(queue) = &self.queue {
                queue.set_error(post_id, Some("cancelled".into())).await;
            }
            return DownloadOutcome {
                post_id: post.id,
                path: PathBuf::new(),
                size: 0,
                hash: String::new(),
                skipped: false,
                error: Some("cancelled".into()),
            };
        }

        // Only report "downloading" once a concurrency slot is held; entries
        // waiting for a slot stay queued instead of looking active.
        if let Some(queue) = &self.queue {
            queue.set_status(post_id, DownloadStatus::Downloading).await;
        }

        let dir = output_dir.unwrap_or_else(|| self.config.download_dir());
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            if let Some(queue) = &self.queue {
                queue.set_error(post_id, Some(e.to_string())).await;
            }
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
                    if let Some(queue) = &self.queue {
                        queue.set_status(post_id, DownloadStatus::Completed).await;
                    }
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
                if let Some(queue) = &self.queue {
                    queue.set_status(post_id, DownloadStatus::Completed).await;
                }
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
            if let Some(queue) = &self.queue {
                queue.set_error(post_id, Some("no file_url".into())).await;
            }
            return DownloadOutcome {
                post_id: post.id,
                path: final_path,
                size: 0,
                hash: String::new(),
                skipped: false,
                error: Some("no file_url".into()),
            };
        }

        // Try primary URL (file_url), with fallback to sample_url for large files
        let sample_url = if !post.sample_url.trim().is_empty() {
            Some(post.sample_url.clone())
        } else {
            None
        };

        // Determine if file size suggests we should try sample as fallback
        // Large files (> 15 MB) may timeout or expire on download
        let large_file = post.file_size > 15_000_000;

        let part_path = dir.join(format!("{}.part", filename));
        let expected_size = post.file_size.max(0) as u64;
        let mut result = self
            .download_file(&url, &final_path, &part_path, post_id, expected_size)
            .await;
        let mut retries = 0;
        let mut current_url = url.clone();

        while result.is_err() && retries < 5 {
            // A cancelled download should stop immediately instead of being
            // retried with backoff (important for the interactive viewer).
            if self.cancel.is_cancelled() {
                if let Some(queue) = &self.queue {
                    queue.set_error(post_id, Some("cancelled".into())).await;
                }
                let _ = tokio::fs::remove_file(&part_path).await;
                return DownloadOutcome {
                    post_id: post.id,
                    path: final_path,
                    size: 0,
                    hash: String::new(),
                    skipped: false,
                    error: Some("cancelled".into()),
                };
            }

            let e = result.as_ref().unwrap_err();

            // don't retry validation errors
            if matches!(e, AppError::Validation(_)) {
                break;
            }

            retries += 1;
            tracing::warn!("retrying {} (attempt {}): {}", post.id, retries, e);
            tokio::time::sleep(std::time::Duration::from_millis(
                100 * 10u64.pow(retries.min(2) as u32),
            ))
            .await;

            // Try sample URL as fallback for large files, or on 404/500 errors
            let next_url = if let Some(surl) = &sample_url {
                // Use sample for large files or on HTTP errors
                if large_file || matches!(e, AppError::Http(_)) {
                    Some(surl.clone())
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(next_url) = next_url {
                current_url = next_url.clone();
                result = self
                    .download_file(&next_url, &final_path, &part_path, post_id, expected_size)
                    .await;
            } else {
                result = self
                    .download_file(&url, &final_path, &part_path, post_id, expected_size)
                    .await;
            }
        }
        match result {
            Ok((size, hash)) => {
                // duplicate by hash
                if self.config.downloads.skip_duplicates {
                    if let Some(db) = &self.db {
                        if db.exists_hash(&hash).await.unwrap_or(false) {
                            // remove file? keep but mark duplicate? We'll delete to avoid dup
                            let _ = tokio::fs::remove_file(&final_path).await;
                            if let Some(queue) = &self.queue {
                                queue.set_status(post_id, DownloadStatus::Completed).await;
                            }
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
                        url: current_url.clone(),
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
                if let Some(queue) = &self.queue {
                    queue.set_status(post_id, DownloadStatus::Completed).await;
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
                let err_msg = match &e {
                    AppError::Http(s) if s.contains("404") => {
                        format!(
                            "{} (file may have been removed or hotlink expired - try again later or view preview with arrow keys)",
                            s
                        )
                    }
                    AppError::Http(s) if s.contains("500") || s.contains("503") => {
                        format!("{} (server temporarily unavailable - try again later)", s)
                    }
                    _ => e.to_string(),
                };
                if let Some(queue) = &self.queue {
                    queue.set_error(post_id, Some(err_msg.clone())).await;
                }
                let _ = tokio::fs::remove_file(&part_path).await;
                let mut st = self.stats.lock().await;
                st.failed += 1;
                DownloadOutcome {
                    post_id: post.id,
                    path: final_path,
                    size: 0,
                    hash: String::new(),
                    skipped: false,
                    error: Some(err_msg),
                }
            }
        }
    }

    async fn download_file(
        &self,
        url: &str,
        final_path: &Path,
        part_path: &Path,
        post_id: i64,
        expected_size: u64,
    ) -> Result<(u64, String)> {
        use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, RANGE};

        // Get the queue for progress updates
        let queue = self.queue.as_ref();

        // Defensive: recreate directories if they were removed mid-download.
        if let Some(parent) = part_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        if let Some(parent) = final_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }

        // Update queue with total size estimate when we start
        if let Some(q) = &queue {
            // Mark as downloading
            q.set_status(post_id, DownloadStatus::Downloading).await;
        }

        // determine resume offset - use metadata instead of reading entire file
        let resume_from: u64 = if self.config.downloads.resume && part_path.exists() {
            tokio::fs::metadata(part_path)
                .await
                .map(|m| m.len())
                .unwrap_or(0)
        } else {
            0
        };

        // Manual redirect handling so the Referer header is preserved (Gelbooru hotlink protection).
        let mut current_url = url.to_string();
        let mut total_size: Option<u64> = None;
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
            // Get content length for progress tracking
            if let Some(len) = r
                .headers()
                .get(CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
            {
                total_size = len.parse().ok();
            }
            break r;
        };
        let status = resp.status();
        if !status.is_success() && status.as_u16() != 206 {
            return Err(AppError::Http(format!("HTTP {}", status)));
        }

        // Content-type check - keep for reliability
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

        // Update total size in queue if we know it
        if let Some(q) = &queue {
            if let Some(size) = total_size {
                let mut entries = q.entries.lock().await;
                for e in entries.iter_mut() {
                    if e.post_id == post_id {
                        e.total = size;
                        break;
                    }
                }
            }
        }

        // If the API did not report a size beforehand, grow the CLI bar so
        // the percentage stays meaningful once the server tells us.
        if expected_size == 0 {
            if let (Some(pb), Some(size)) = (&self.progress_bar, total_size) {
                pb.inc_length(size);
            }
        }

        let mut hasher = Hasher::new();
        let mut total = resume_from;

        // For resume: hash existing partial file content for correct final hash
        if status.as_u16() == 206 && resume_from > 0 {
            if let Ok(mut partial) = tokio::fs::File::open(part_path).await {
                use tokio::io::AsyncReadExt;
                let mut buf = vec![0u8; 8192];
                let mut read_total: usize = 0;
                while read_total < resume_from as usize {
                    let to_read = buf.len().min(resume_from as usize - read_total);
                    buf.truncate(to_read);
                    let n = partial.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    hasher.update(&buf[..n]);
                    read_total += n;
                }
            }
        }

        let mut file = if status.as_u16() == 206 {
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
        let mut last_progress_update = std::time::Instant::now();
        while let Some(chunk) = stream.next().await {
            if self.cancel.is_cancelled() {
                return Err(AppError::Cancelled);
            }
            let chunk: Bytes = chunk.map_err(|e| AppError::Http(e.to_string()))?;
            file.write_all(&chunk).await.map_err(AppError::Io)?;
            hasher.update(&chunk);
            total += chunk.len() as u64;
            if let Some(pb) = &self.progress_bar {
                pb.inc(chunk.len() as u64);
            }
            if let Some(limit) = bandwidth {
                if limit > 0 {
                    let delay = std::time::Duration::from_millis(
                        (chunk.len() as u64 * 1000 / limit).max(1),
                    );
                    tokio::time::sleep(delay).await;
                }
            }

            // Update progress during download (throttled to ~100ms intervals)
            if let Some(q) = &queue {
                if last_progress_update.elapsed().as_millis() >= 100 {
                    let progress = if let Some(size) = total_size {
                        if size > 0 {
                            (total as f64 / size as f64 * 100.0) as usize
                        } else {
                            0
                        }
                    } else {
                        0
                    };
                    q.set_progress(post_id, progress, total).await;
                    last_progress_update = std::time::Instant::now();
                }
            }
        }
        // Final progress update at completion
        if let Some(q) = &queue {
            q.set_progress(post_id, 100, total).await;
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
        let total_files = posts.len();
        // The API reports each post's size, so the bar can track real bytes
        // instead of only completed files. If no sizes are known at all we
        // fall back to a file-count bar.
        let expected_total: u64 = posts
            .iter()
            .filter(|p| p.file_size > 0)
            .map(|p| p.file_size as u64)
            .sum();
        let byte_mode = expected_total > 0;

        if let Some(pb) = &progress {
            pb.set_length(if byte_mode {
                expected_total
            } else {
                total_files as u64
            });
            if byte_mode {
                pb.set_message(format!("0/{total_files} files"));
            }
        }
        self.progress.set_context(
            self.label.clone(),
            self.rating.clone(),
            output_dir.as_ref().map(|p| p.to_string_lossy().to_string()),
        );
        self.progress.reset(total_files);
        self.progress.write_file();

        // Workers share the bar so `download_file` can advance it while
        // chunks stream in, not just when a whole file is done.
        let mut engine = self.clone();
        engine.progress_bar = if byte_mode { progress.clone() } else { None };

        let progress_tracker = self.progress.clone();
        let files_done = Arc::new(AtomicUsize::new(0));
        let mut futs = Vec::new();
        for p in posts {
            let expected_size = p.file_size.max(0) as u64;
            let eng = engine.clone();
            let od = output_dir.clone();
            let pb = progress.clone();
            let pt = progress_tracker.clone();
            let done = files_done.clone();
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
                    let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if byte_mode {
                        // Files skipped before any transfer (size 0) are part
                        // of the planned total but moved no bytes, so advance
                        // past them. Hash-duplicates already counted their
                        // streamed bytes.
                        if res.skipped && res.size == 0 {
                            b.inc(expected_size);
                        }
                        b.set_message(format!("{finished}/{total_files} files"));
                    } else {
                        b.inc(1);
                    }
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
        if let Some(pb) = &progress {
            pb.finish_with_message(format!("{total_files}/{total_files} files"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn queue_tracks_multiple_active_downloads() {
        let queue = DownloadQueue::new();
        assert!(queue.is_empty().await);

        queue.add_queued(1).await;
        queue.add_queued(2).await;
        assert_eq!(queue.len().await, 2);
        assert!(queue.is_active(1).await);
        assert!(queue.is_active(2).await);

        queue.set_status(1, DownloadStatus::Downloading).await;
        queue.set_status(2, DownloadStatus::Completed).await;
        assert!(queue.is_active(1).await);
        assert!(!queue.is_active(2).await);
    }

    #[tokio::test]
    async fn queue_errors_become_terminal_states() {
        let queue = DownloadQueue::new();
        queue.add_queued(1).await;
        queue.add_queued(2).await;

        queue.set_error(1, Some("HTTP 500".into())).await;
        queue.set_error(2, Some("cancelled".into())).await;

        let entries = queue.entries().await;
        assert_eq!(entries[0].status, DownloadStatus::Failed);
        assert_eq!(entries[1].status, DownloadStatus::Cancelled);
        assert!(!queue.is_active(1).await);
        assert!(!queue.is_active(2).await);
    }

    #[test]
    fn cloned_engines_share_the_queue_and_concurrency_limit() {
        let queue = Arc::new(DownloadQueue::new());
        let engine = DownloadEngine::with_queue(Config::default(), None, Some(queue.clone()));
        let clone = engine.clone();

        assert!(Arc::ptr_eq(&engine.semaphore, &clone.semaphore));
        assert!(Arc::ptr_eq(engine.queue.as_ref().unwrap(), &queue));
        assert!(Arc::ptr_eq(clone.queue.as_ref().unwrap(), &queue));
    }
}
