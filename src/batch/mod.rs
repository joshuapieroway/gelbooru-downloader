use crate::api::client::GelbooruClient;
use crate::api::models::Post;
use crate::api::search;
use crate::config::Config;
use crate::downloader::engine::{DownloadEngine, DownloadOutcome};
use crate::filters::rules::FilterRules;
use crate::utils::error::{AppError, Result};
use serde::Serialize;
use std::collections::HashSet;
use std::path::PathBuf;
use tokio::fs;

/// Aggregated result of a batch run. Search failures are reported per tag so
/// one bad query cannot silently turn into an empty, successful run.
#[derive(Debug, Default, Serialize)]
pub struct BatchReport {
    pub tags: Vec<String>,
    pub outcomes: Vec<DownloadOutcome>,
    pub errors: Vec<(String, String)>,
}

impl BatchReport {
    pub fn completed(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| o.error.is_none() && !o.skipped)
            .count()
    }

    pub fn skipped(&self) -> usize {
        self.outcomes.iter().filter(|o| o.skipped).count()
    }

    pub fn failed(&self) -> usize {
        self.outcomes.iter().filter(|o| o.error.is_some()).count()
    }
}

/// Per-run settings for a batch, kept together so the function signature stays
/// readable as more filters and limits are added.
pub struct BatchOptions {
    pub filters: FilterRules,
    pub limit_per_tag: u32,
    pub output_dir: Option<PathBuf>,
    pub concurrent_jobs: usize,
}

pub async fn run_batch(
    path: &str,
    client: &GelbooruClient,
    cfg: &Config,
    engine: &DownloadEngine,
    options: BatchOptions,
) -> Result<BatchReport> {
    let BatchOptions {
        filters,
        limit_per_tag,
        output_dir,
        concurrent_jobs,
    } = options;
    let content = fs::read_to_string(path).await.map_err(AppError::Io)?;
    let mut seen = HashSet::new();
    let tags_list: Vec<String> = content
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter(|l| seen.insert(l.to_lowercase()))
        .collect();

    // Search tags with bounded concurrency. Downloads happen afterwards, so
    // two tags that match the same post cannot race on one `.part` file.
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrent_jobs.max(1)));
    let mut handles = Vec::new();
    for tag in tags_list {
        let sem = sem.clone();
        let client = client.clone();
        let cfg = cfg.clone();
        let filters = filters.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem
                .acquire()
                .await
                .map_err(|e| (tag.clone(), e.to_string()))?;
            let params = search::SearchParams {
                tags: tag.clone(),
                rating: None,
                limit: limit_per_tag,
                page_size: cfg.search.page_size,
                sort: None,
                order: None,
                filters,
            };
            let posts = search::search_all(&client, &cfg, &params)
                .await
                .map_err(|e| (tag.clone(), e.to_string()))?;
            Ok::<_, (String, String)>((tag, posts))
        }));
    }

    let mut report = BatchReport::default();
    let mut groups: Vec<Vec<Post>> = Vec::new();
    for h in handles {
        match h.await {
            Ok(Ok((tag, posts))) => {
                report.tags.push(tag);
                groups.push(posts);
            }
            Ok(Err((tag, message))) => report.errors.push((tag, message)),
            Err(e) => report
                .errors
                .push((String::new(), format!("task join error: {e}"))),
        }
    }

    let mut seen = HashSet::new();
    let mut posts = Vec::new();
    for group in groups {
        for post in group {
            if post.id == 0 || seen.insert(post.id) {
                posts.push(post);
            }
        }
    }
    report.outcomes = engine.download_many(posts, output_dir, None).await;
    Ok(report)
}
