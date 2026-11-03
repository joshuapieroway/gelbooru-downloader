use crate::api::client::GelbooruClient;
use crate::api::search;
use crate::config::Config;
use crate::downloader::engine::DownloadEngine;
use crate::filters::rules::FilterRules;
use crate::utils::error::Result;
use std::path::PathBuf;
use tokio::fs;

pub async fn run_batch(
    path: &str,
    client: &GelbooruClient,
    cfg: &Config,
    engine: &DownloadEngine,
    filters: FilterRules,
    limit_per_tag: u32,
    output_dir: Option<PathBuf>,
    concurrent_jobs: usize,
) -> Result<Vec<String>> {
    let content = fs::read_to_string(path)
        .await
        .map_err(crate::utils::error::AppError::Io)?;
    let tags_list: Vec<String> = content
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();

    // process sequentially or with bounded concurrency
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrent_jobs.max(1)));
    let mut handles = Vec::new();
    for tag in tags_list {
        let sem = sem.clone();
        let client = client.clone();
        let cfg = cfg.clone();
        let engine = engine.clone();
        let filters = filters.clone();
        let od = output_dir.clone();
        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            let params = search::SearchParams {
                tags: tag.clone(),
                rating: None,
                limit: limit_per_tag,
                page_size: cfg.search.page_size,
                sort: None,
                order: None,
            };
            let posts = search::search_all(&client, &cfg, &params)
                .await
                .unwrap_or_default();
            let posts = filters.filter_posts(posts);
            let _ = engine.download_many(posts, od, None).await;
            tag
        }));
    }
    let mut done = Vec::new();
    for h in handles {
        if let Ok(t) = h.await {
            done.push(t);
        }
    }
    Ok(done)
}
