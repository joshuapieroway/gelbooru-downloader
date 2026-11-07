//! Persistent download queue.
//!
//! Every post enqueued through [`crate::downloader::engine::DownloadEngine`] is
//! written to SQLite before its transfer starts. Successful rows are deleted;
//! failures stay behind so they can be inspected with `queue list` and retried
//! with `queue retry`. On TUI startup, [`resume`] picks up rows that were left
//! queued or downloading by a previous process.

use crate::api::models::Post;
use crate::config::Config;
use crate::database::history::{HistoryDb, QueuedDownload};
use crate::downloader::engine::DownloadEngine;
use crate::utils::error::Result;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Default, Clone, Serialize)]
pub struct ResumeReport {
    pub resumed: usize,
    pub completed: usize,
    pub skipped: usize,
    pub failed: usize,
}

/// Downloads are grouped by the settings that affect their destination, so one
/// engine (and one progress record) can handle each group.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct GroupKey {
    output_dir: String,
    filename_template: String,
    folder_template: String,
    sort: Option<String>,
    order: Option<String>,
}

/// Resume every row still marked `queued` or `downloading`.
pub async fn resume(db: &Arc<HistoryDb>, cfg: &Config) -> Result<ResumeReport> {
    let rows = db.queue_pending().await?;
    let mut report = ResumeReport {
        resumed: rows.len(),
        ..ResumeReport::default()
    };
    if rows.is_empty() {
        return Ok(report);
    }

    let mut groups: BTreeMap<GroupKey, Vec<QueuedDownload>> = BTreeMap::new();
    for row in rows {
        groups
            .entry(GroupKey {
                output_dir: row.output_dir.clone(),
                filename_template: row.filename_template.clone(),
                folder_template: row.folder_template.clone(),
                sort: row.sort.clone(),
                order: row.order.clone(),
            })
            .or_default()
            .push(row);
    }

    for (key, rows) in groups {
        let mut engine = DownloadEngine::new(cfg.clone(), Some(db.clone()));
        engine.filename_template = Some(key.filename_template);
        engine.folder_template = Some(key.folder_template);
        engine.sort = key.sort;
        engine.order = key.order;
        engine.label = Some(format!("resumed queue ({} files)", rows.len()));

        let posts: Vec<Post> = rows.into_iter().map(|row| row.post).collect();
        let outcomes = engine
            .download_many(posts, Some(PathBuf::from(key.output_dir)), None)
            .await;
        for outcome in outcomes {
            if outcome.error.is_some() {
                report.failed += 1;
            } else if outcome.skipped {
                report.skipped += 1;
            } else {
                report.completed += 1;
            }
        }
    }

    Ok(report)
}

/// Requeue failed rows, then resume them along with anything already pending.
pub async fn retry_failed(db: &Arc<HistoryDb>, cfg: &Config) -> Result<ResumeReport> {
    db.queue_retry_failed().await?;
    resume(db, cfg).await
}
