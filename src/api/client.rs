use crate::config::Config;
use crate::utils::error::{AppError, Result};
use reqwest::Client;
use std::time::Duration;

#[derive(Clone)]
pub struct GelbooruClient {
    http: Client,
    base_url: String,
    user_id: String,
    api_key: String,
    max_retries: u32,
}

impl GelbooruClient {
    pub fn new(config: &Config) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(config.api.timeout))
            .pool_max_idle_per_host(16)
            .user_agent(concat!("gelbooru-downloader/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| AppError::Http(e.to_string()))?;
        Ok(Self {
            http,
            base_url: config.api.base_url.trim_end_matches('/').to_string(),
            user_id: config.api.user_id.clone(),
            api_key: config.api.api_key.clone(),
            max_retries: config.downloads.max_retries,
        })
    }

    pub fn from_parts(
        base_url: String,
        user_id: String,
        api_key: String,
        timeout: u64,
        max_retries: u32,
    ) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(timeout))
            .pool_max_idle_per_host(16)
            .user_agent(concat!("gelbooru-downloader/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| AppError::Http(e.to_string()))?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            user_id,
            api_key,
            max_retries,
        })
    }

    pub async fn get_post(&self, id: i64) -> Result<crate::api::models::Post> {
        let url = format!("{}/index.php", self.base_url);
        let mut req = self.http.get(&url).query(&[
            ("page", "dapi"),
            ("s", "post"),
            ("q", "index"),
            ("id", &id.to_string()),
            ("json", "1"),
        ]);
        req = self.apply_auth(req);
        let resp = self.send_with_retry(req).await?;
        let text = resp
            .text()
            .await
            .map_err(|e| AppError::Http(e.to_string()))?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            let snippet: String = text.chars().take(200).collect();
            AppError::Api(format!("json parse: {} body={}", e, snippet))
        })?;
        let posts = crate::api::models::parse_posts(&v);
        posts
            .into_iter()
            .next()
            .ok_or_else(|| AppError::NotFound(format!("post {}", id)))
    }

    pub async fn search_page(
        &self,
        tags: &str,
        limit: u32,
        page: u32,
        sort: Option<&str>,
        order: Option<&str>,
    ) -> Result<(Vec<crate::api::models::Post>, i64)> {
        let url = format!("{}/index.php", self.base_url);
        let pid = page.saturating_sub(1);
        // Gelbooru caps responses at 100 posts regardless of the requested
        // limit. Clamping here keeps paging offsets correct for every caller.
        let limit = limit.clamp(1, crate::api::search::MAX_PAGE_SIZE);
        let limit_s = limit.to_string();
        let pid_s = pid.to_string();
        let mut params: Vec<(&str, String)> = vec![
            ("page", "dapi".to_string()),
            ("s", "post".to_string()),
            ("q", "index".to_string()),
            ("tags", tags.to_string()),
            ("limit", limit_s),
            ("pid", pid_s),
            ("json", "1".to_string()),
        ];
        if let Some(srt) = sort {
            if !srt.is_empty() {
                params.push(("sort", srt.to_string()));
            }
        }
        if let Some(ord) = order {
            if !ord.is_empty() {
                params.push(("order", ord.to_string()));
            }
        }
        let mut req = self.http.get(&url).query(&params);
        req = self.apply_auth(req);
        let resp = self.send_with_retry(req).await?;
        let text = resp
            .text()
            .await
            .map_err(|e| AppError::Http(e.to_string()))?;
        let v: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| AppError::Api(format!("json parse: {}", e)))?;
        let total = v
            .get("@attributes")
            .and_then(|a| a.get("count"))
            .and_then(|c| c.as_i64())
            .or_else(|| {
                v.get("attributes")
                    .and_then(|a| a.get("count"))
                    .and_then(|c| c.as_i64())
            })
            .unwrap_or(0);
        let posts = crate::api::models::parse_posts(&v);
        Ok((posts, total))
    }

    pub async fn search(
        &self,
        tags: &str,
        limit: u32,
        page: u32,
        sort: Option<&str>,
        order: Option<&str>,
    ) -> Result<Vec<crate::api::models::Post>> {
        let (posts, _) = self.search_page(tags, limit, page, sort, order).await?;
        Ok(posts)
    }

    fn apply_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if !self.user_id.is_empty() && !self.api_key.is_empty() {
            req.query(&[("user_id", &self.user_id), ("api_key", &self.api_key)])
        } else {
            req
        }
    }

    async fn send_with_retry(&self, req: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let mut attempt = 0;
        loop {
            let r = req
                .try_clone()
                .ok_or_else(|| AppError::Http("cannot clone request".into()))?
                .send()
                .await;
            match r {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        return Ok(resp);
                    }
                    if status.as_u16() == 429 {
                        attempt += 1;
                        if attempt > self.max_retries {
                            return Err(AppError::RateLimited);
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(
                            2u64.pow(attempt.min(6)),
                        ))
                        .await;
                        continue;
                    }
                    if status.is_server_error() {
                        attempt += 1;
                        if attempt > self.max_retries {
                            return Err(AppError::Http(format!("HTTP {}", status)));
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(
                            300 * (1 << attempt.min(6)),
                        ))
                        .await;
                        continue;
                    }
                    return Err(AppError::Http(format!("HTTP {}", status)));
                }
                Err(e) => {
                    attempt += 1;
                    if attempt > self.max_retries {
                        return Err(AppError::Http(e.to_string()));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(
                        300 * (1 << attempt.min(6)),
                    ))
                    .await;
                    continue;
                }
            }
        }
    }
}
