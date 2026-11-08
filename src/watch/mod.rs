use crate::api::client::GelbooruClient;
use crate::api::models::Post;
use crate::api::search;
use crate::config::Config;
use crate::database::history::HistoryDb;
use crate::downloader::engine::DownloadEngine;
use crate::filters::rules::FilterRules;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::time::{Duration, sleep};

/// Failed downloads are retried on later polls so a transient error cannot
/// permanently skip a post just because the watch cursor moved past its id.
const MAX_PENDING_RETRIES: usize = 500;

pub struct Watcher {
    pub tags: String,
    pub interval: u64,
    pub client: GelbooruClient,
    pub cfg: Config,
    pub db: Arc<HistoryDb>,
    pub engine: DownloadEngine,
    pub filters: FilterRules,
    pub rating: Option<String>,
    pub limit: u32,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub output_dir: Option<std::path::PathBuf>,
}

impl Watcher {
    pub async fn run(&self, cancel: tokio_util::sync::CancellationToken) {
        let mut pending: Vec<Post> = Vec::new();
        loop {
            if cancel.is_cancelled() {
                break;
            }
            let last = self.db.get_watch_state(&self.tags).await.unwrap_or(0);
            let params = search::SearchParams {
                tags: self.tags.clone(),
                rating: self.rating.clone(),
                limit: self.limit,
                page_size: self.cfg.search.page_size,
                sort: self.sort.clone(),
                order: self.order.clone(),
                filters: self.filters.clone(),
            };

            let mut max_id = last;
            let mut new_posts = Vec::new();
            match search::search_all(&self.client, &self.cfg, &params).await {
                Ok(posts) => {
                    for post in posts {
                        if post.id > last {
                            max_id = max_id.max(post.id);
                            new_posts.push(post);
                        }
                    }
                }
                Err(e) => {
                    // Keep retrying pending posts even when the search fails.
                    tracing::warn!("watch search for '{}' failed: {}", self.tags, e);
                }
            }

            let mut seen = HashSet::new();
            let batch: Vec<Post> = pending
                .drain(..)
                .chain(new_posts)
                .filter(|p| seen.insert(p.id))
                .collect();
            if !batch.is_empty() {
                let by_id: HashMap<i64, Post> = batch.iter().map(|p| (p.id, p.clone())).collect();
                let outcomes = self
                    .engine
                    .download_many(batch, self.output_dir.clone(), None)
                    .await;
                for outcome in outcomes {
                    if let Some(error) = &outcome.error {
                        tracing::warn!("watch download {} failed: {}", outcome.post_id, error);
                        if let Some(post) = by_id.get(&outcome.post_id) {
                            pending.push(post.clone());
                        }
                    }
                }
                if pending.len() > MAX_PENDING_RETRIES {
                    let dropped = pending.len() - MAX_PENDING_RETRIES;
                    pending.drain(..dropped);
                    tracing::warn!("watch retry queue full; dropped {dropped} oldest posts");
                }
            }

            // Advance past every id we saw, even failures: failed posts stay in
            // the in-memory retry queue instead of being fetched again forever.
            if max_id > last {
                if let Err(e) = self.db.set_watch_state(&self.tags, max_id).await {
                    tracing::warn!("could not save watch state: {e}");
                }
            }

            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = sleep(Duration::from_secs(self.interval.max(1))) => {}
            }
        }
    }
}
