use crate::api::client::GelbooruClient;
use crate::api::search;
use crate::config::Config;
use crate::database::history::HistoryDb;
use crate::downloader::engine::DownloadEngine;
use crate::filters::rules::FilterRules;
use std::sync::Arc;
use tokio::time::{Duration, sleep};

pub struct Watcher {
    pub tags: String,
    pub interval: u64,
    pub client: GelbooruClient,
    pub cfg: Config,
    pub db: Arc<HistoryDb>,
    pub engine: DownloadEngine,
    pub filters: FilterRules,
    pub output_dir: Option<std::path::PathBuf>,
}

impl Watcher {
    pub async fn run(&self, cancel: tokio_util::sync::CancellationToken) {
        loop {
            if cancel.is_cancelled() {
                break;
            }
            let last = self.db.get_watch_state(&self.tags).await.unwrap_or(0);
            let params = search::SearchParams {
                tags: self.tags.clone(),
                rating: None,
                limit: 100,
                page_size: self.cfg.search.page_size,
                sort: None,
                order: None,
            };
            if let Ok(posts) = search::search_all(&self.client, &self.cfg, &params).await {
                let new_posts: Vec<_> = posts.into_iter().filter(|p| p.id > last).collect();
                let mut max_id = last;
                for p in &new_posts {
                    if p.id > max_id {
                        max_id = p.id;
                    }
                }
                let filtered = self.filters.filter_posts(new_posts);
                let _ = self
                    .engine
                    .download_many(filtered, self.output_dir.clone(), None)
                    .await;
                if max_id > last {
                    let _ = self.db.set_watch_state(&self.tags, max_id).await;
                }
            }
            // wait interval
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = sleep(Duration::from_secs(self.interval)) => {}
            }
        }
    }
}
