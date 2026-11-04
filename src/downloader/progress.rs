use crate::utils::paths;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Progress records that have not been updated for this long are considered
/// abandoned (for example after a process was killed) and are removed.
pub const PROGRESS_TTL_SECS: i64 = 10;

const JOB_EXTENSION: &str = "json";
static NEXT_JOB: AtomicU64 = AtomicU64::new(0);

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Directory containing one JSON file per active download job.
pub fn progress_dir() -> PathBuf {
    paths::data_dir().join("progress")
}

/// A single download job as shown in the TUI queue.
///
/// Records are written by independent process jobs, so two downloads of the
/// same tag never overwrite each other.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressSnapshot {
    #[serde(default)]
    pub job_id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub rating: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub total: usize,
    #[serde(default)]
    pub completed: usize,
    #[serde(default)]
    pub failed: usize,
    #[serde(default)]
    pub skipped: usize,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub started_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

impl ProgressSnapshot {
    /// Human-readable job name. Includes the rating filter so two searches
    /// for the same tag remain distinguishable.
    pub fn base_label(&self) -> String {
        let mut label = if self.label.trim().is_empty() {
            "download".to_string()
        } else {
            self.label.trim().to_string()
        };
        if let Some(rating) = self
            .rating
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
        {
            label.push_str(" [");
            label.push_str(rating);
            label.push(']');
        }
        label
    }

    /// Final path component of the output directory, if one was supplied.
    pub fn output_name(&self) -> Option<String> {
        let raw = self.output.as_deref()?.trim();
        if raw.is_empty() {
            return None;
        }
        Path::new(raw)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .or_else(|| Some(raw.to_string()))
    }

    pub fn is_stale(&self, now: i64) -> bool {
        now.saturating_sub(self.updated_at) > PROGRESS_TTL_SECS
    }
}

struct Meta {
    job_id: String,
    label: String,
    rating: Option<String>,
    output: Option<String>,
    started_at: i64,
    updated_at: i64,
}

/// Shared counters for one `download_many` invocation.
///
/// Each tracker owns a unique JSON file, so concurrent processes and
/// concurrent jobs inside one process never race on the same record.
pub struct ProgressTracker {
    pub total: AtomicUsize,
    pub completed: AtomicUsize,
    pub failed: AtomicUsize,
    pub skipped: AtomicUsize,
    pub bytes: AtomicU64,
    meta: Mutex<Meta>,
    path: PathBuf,
    write_lock: Mutex<()>,
}

impl ProgressTracker {
    pub fn new() -> Self {
        Self::with_dir(progress_dir())
    }

    pub fn with_dir(dir: PathBuf) -> Self {
        let sequence = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
        let started_at = now_ts();
        let job_id = format!("{}-{}-{}", std::process::id(), started_at, sequence);
        let path = dir.join(format!("{job_id}.{JOB_EXTENSION}"));
        Self {
            total: AtomicUsize::new(0),
            completed: AtomicUsize::new(0),
            failed: AtomicUsize::new(0),
            skipped: AtomicUsize::new(0),
            bytes: AtomicU64::new(0),
            meta: Mutex::new(Meta {
                job_id,
                label: "download".to_string(),
                rating: None,
                output: None,
                started_at,
                updated_at: started_at,
            }),
            path,
            write_lock: Mutex::new(()),
        }
    }

    /// Attach the search/tag context used to tell simultaneous jobs apart.
    pub fn set_context(
        &self,
        label: Option<String>,
        rating: Option<String>,
        output: Option<String>,
    ) {
        let mut meta = self.meta.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(label) = label.filter(|l| !l.trim().is_empty()) {
            meta.label = label;
        }
        meta.rating = rating.filter(|r| !r.trim().is_empty());
        meta.output = output.filter(|o| !o.trim().is_empty());
    }

    pub fn reset(&self, total: usize) {
        self.total.store(total, Ordering::Relaxed);
        self.completed.store(0, Ordering::Relaxed);
        self.failed.store(0, Ordering::Relaxed);
        self.skipped.store(0, Ordering::Relaxed);
        self.bytes.store(0, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> ProgressSnapshot {
        let meta = self.meta.lock().unwrap_or_else(|e| e.into_inner());
        self.snapshot_locked(&meta)
    }

    fn snapshot_locked(&self, meta: &Meta) -> ProgressSnapshot {
        ProgressSnapshot {
            job_id: meta.job_id.clone(),
            label: meta.label.clone(),
            rating: meta.rating.clone(),
            output: meta.output.clone(),
            total: self.total.load(Ordering::Relaxed),
            completed: self.completed.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            skipped: self.skipped.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
            started_at: meta.started_at,
            updated_at: meta.updated_at,
        }
    }

    /// Publish the current counters. The file is replaced atomically so the
    /// TUI never reads a half-written record.
    pub fn write_file(&self) {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let snapshot = {
            let mut meta = self.meta.lock().unwrap_or_else(|e| e.into_inner());
            meta.updated_at = now_ts();
            self.snapshot_locked(&meta)
        };
        let Ok(data) = serde_json::to_vec(&snapshot) else {
            return;
        };
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let tmp = self.path.with_extension("tmp");
        if fs::write(&tmp, data).is_ok() && fs::rename(&tmp, &self.path).is_err() {
            let _ = fs::remove_file(&tmp);
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Default for ProgressTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ProgressTracker {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(self.path.with_extension("tmp"));
    }
}

/// Read every live job record. Stale and malformed records are cleaned up as
/// a side effect, so abandoned processes cannot leave rows behind forever.
pub fn read_snapshots() -> Vec<ProgressSnapshot> {
    read_snapshots_from(&progress_dir(), now_ts())
}

pub fn read_snapshots_from(dir: &Path, now: i64) -> Vec<ProgressSnapshot> {
    let mut snapshots = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return snapshots;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some(JOB_EXTENSION) {
            continue;
        }
        let parsed = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<ProgressSnapshot>(&text).ok());
        match parsed {
            Some(snapshot) if snapshot.is_stale(now) => {
                let _ = fs::remove_file(&path);
            }
            Some(snapshot) if snapshot.total > 0 => snapshots.push(snapshot),
            Some(_) => {}
            None => {
                if file_is_stale(&path, now) {
                    let _ = fs::remove_file(&path);
                }
            }
        }
    }
    snapshots.sort_by(|a, b| {
        a.started_at
            .cmp(&b.started_at)
            .then_with(|| a.job_id.cmp(&b.job_id))
    });
    snapshots
}

fn file_is_stale(path: &Path, now: i64) -> bool {
    path.metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| now.saturating_sub(d.as_secs() as i64) > PROGRESS_TTL_SECS)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn tracker(dir: &Path, label: &str, rating: &str) -> ProgressTracker {
        let tracker = ProgressTracker::with_dir(dir.to_path_buf());
        tracker.set_context(
            Some(label.to_string()),
            Some(rating.to_string()),
            Some("/tmp/out".to_string()),
        );
        tracker.reset(10);
        tracker.completed.store(3, Ordering::Relaxed);
        tracker.write_file();
        tracker
    }

    #[test]
    fn concurrent_jobs_are_written_as_separate_records() {
        let dir = tempdir().unwrap();
        let first = tracker(dir.path(), "gilberta_(arknights)", "general,sensitive");
        let second = tracker(dir.path(), "gilberta_(arknights)", "explicit,questionable");

        let snapshots = read_snapshots_from(dir.path(), now_ts());
        assert_eq!(snapshots.len(), 2);
        assert_ne!(snapshots[0].job_id, snapshots[1].job_id);
        assert!(snapshots.iter().any(|s| s.base_label().contains("general")));
        assert!(
            snapshots
                .iter()
                .any(|s| s.base_label().contains("explicit"))
        );

        drop(first);
        drop(second);
        assert!(read_snapshots_from(dir.path(), now_ts()).is_empty());
    }

    #[test]
    fn same_tag_and_rating_keeps_output_directories_distinct() {
        let dir = tempdir().unwrap();
        let first = tracker(dir.path(), "tag", "general");
        let second = tracker(dir.path(), "tag", "general");
        {
            let mut meta = first.meta.lock().unwrap();
            meta.output = Some("/tmp/safe".to_string());
            meta.updated_at = now_ts();
        }
        {
            let mut meta = second.meta.lock().unwrap();
            meta.output = Some("/tmp/explicit".to_string());
            meta.updated_at = now_ts();
        }
        first.write_file();
        second.write_file();

        let snapshots = read_snapshots_from(dir.path(), now_ts());
        let mut snapshots: Vec<_> = snapshots.iter().collect();
        // Sort by output path lexically; "/tmp/explicit" < "/tmp/safe"
        snapshots.sort_by_key(|s| s.output.clone());
        assert_eq!(snapshots.len(), 2);
        // First alphabetically: "/tmp/explicit"
        assert_eq!(snapshots[0].output_name().as_deref(), Some("explicit"));
        // Second alphabetically: "/tmp/safe"
        assert_eq!(snapshots[1].output_name().as_deref(), Some("safe"));
    }

    #[test]
    fn stale_records_are_removed_and_never_returned() {
        let dir = tempdir().unwrap();
        let tracker = tracker(dir.path(), "stale", "general");
        let path = tracker.path().to_path_buf();
        drop(tracker);
        assert!(!path.exists());

        let snapshot = ProgressSnapshot {
            job_id: "old".into(),
            label: "old".into(),
            total: 5,
            updated_at: 100,
            ..Default::default()
        };
        let path = dir.path().join("old.json");
        fs::write(&path, serde_json::to_vec(&snapshot).unwrap()).unwrap();

        let snapshots = read_snapshots_from(dir.path(), 100 + PROGRESS_TTL_SECS + 1);
        assert!(snapshots.is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn malformed_stale_records_are_removed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("broken.json");
        fs::write(&path, b"{not json").unwrap();

        // Use a future timestamp to make the file appear stale by modification time
        let snapshots = read_snapshots_from(dir.path(), now_ts() + PROGRESS_TTL_SECS + 1);
        assert!(snapshots.is_empty());
        // Stale malformed files should be removed
        assert!(!path.exists());
    }

    #[test]
    fn concurrent_writers_never_corrupt_the_record() {
        let dir = tempdir().unwrap();
        let tracker = Arc::new(tracker(dir.path(), "busy", "general"));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let tracker = tracker.clone();
            handles.push(std::thread::spawn(move || {
                for _ in 0..50 {
                    tracker.completed.fetch_add(1, Ordering::Relaxed);
                    tracker.write_file();
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let snapshots = read_snapshots_from(dir.path(), now_ts());
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].completed, 3 + 8 * 50);
    }

    #[test]
    fn empty_jobs_are_not_shown() {
        let dir = tempdir().unwrap();
        let tracker = ProgressTracker::with_dir(dir.path().to_path_buf());
        tracker.write_file();

        assert!(read_snapshots_from(dir.path(), now_ts()).is_empty());
    }
}
