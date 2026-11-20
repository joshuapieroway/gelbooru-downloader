use crate::config::Config;
use crate::utils::error::{AppError, Result};
use reqwest::Client;
use std::time::Duration;

/// Image board a client talks to. Only the search tab switches between these
/// for now; the rest of the application keeps using Gelbooru.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Gelbooru,
    Rule34,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Gelbooru => "Gelbooru",
            Source::Rule34 => "Rule34",
        }
    }

    /// Stable lowercase name used in configuration values.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Gelbooru => "gelbooru",
            Source::Rule34 => "rule34",
        }
    }

    /// Parse a configured site name, defaulting to Gelbooru for unknown values.
    pub fn from_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "rule34" | "r34" => Source::Rule34,
            _ => Source::Gelbooru,
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            Source::Gelbooru => Source::Rule34,
            Source::Rule34 => Source::Gelbooru,
        }
    }

    /// Public website used for "open post in browser".
    pub fn website(self) -> &'static str {
        match self {
            Source::Gelbooru => "https://gelbooru.com",
            Source::Rule34 => "https://rule34.xxx",
        }
    }
}

/// Referer expected by an image CDN. Derived from the media URL so downloads
/// work for every supported board without storing the source on each post.
pub fn referer_for_url(url: &str) -> &'static str {
    if url.contains("rule34") {
        "https://rule34.xxx/"
    } else {
        "https://gelbooru.com/"
    }
}

/// Extract the trailing `(1234)` count from a Rule34 autocomplete label.
fn parse_label_count(label: Option<&str>) -> Option<i64> {
    let label = label?;
    let start = label.rfind('(')?;
    let end = label.rfind(')')?;
    if end <= start {
        return None;
    }
    label[start + 1..end].trim().parse().ok()
}

/// Rule34 user IDs are numeric; API keys are long hex strings. Detect the
/// common copy/paste mistake where the two fields are reversed.
fn credentials_look_swapped(user_id: &str, api_key: &str) -> bool {
    let key_is_numeric = !api_key.is_empty() && api_key.chars().all(|c| c.is_ascii_digit());
    let user_id_is_long_hex = user_id.len() > 32
        && user_id.chars().all(|c| c.is_ascii_hexdigit())
        && !user_id.chars().all(|c| c.is_ascii_digit());
    key_is_numeric && user_id_is_long_hex
}

#[derive(Clone)]
pub struct GelbooruClient {
    http: Client,
    source: Source,
    base_url: String,
    user_id: String,
    api_key: String,
    max_retries: u32,
}

impl GelbooruClient {
    pub fn new(config: &Config) -> Result<Self> {
        Self::new_for(config, Source::Gelbooru)
    }

    /// Build a client for the given image board, reading that board's
    /// credentials from the config.
    pub fn new_for(config: &Config, source: Source) -> Result<Self> {
        let (base_url, mut user_id, mut api_key) = match source {
            Source::Gelbooru => (
                config.api.base_url.clone(),
                config.api.user_id.clone(),
                config.api.api_key.clone(),
            ),
            Source::Rule34 => (
                config.rule34.base_url.clone(),
                config.rule34.user_id.clone(),
                config.rule34.api_key.clone(),
            ),
        };
        user_id = user_id.trim().to_string();
        api_key = api_key.trim().to_string();
        // Rule34's settings page shows `&api_key=...&user_id=...`, which makes
        // it easy to paste the values into the wrong fields. The user ID is
        // numeric and the key is a long hex string, so recover automatically.
        if source == Source::Rule34 && credentials_look_swapped(&user_id, &api_key) {
            std::mem::swap(&mut user_id, &mut api_key);
        }
        Self::from_parts_for(
            source,
            base_url,
            user_id,
            api_key,
            config.api.timeout,
            config.downloads.max_retries,
        )
    }

    pub fn from_parts(
        base_url: String,
        user_id: String,
        api_key: String,
        timeout: u64,
        max_retries: u32,
    ) -> Result<Self> {
        Self::from_parts_for(
            Source::Gelbooru,
            base_url,
            user_id,
            api_key,
            timeout,
            max_retries,
        )
    }

    pub fn from_parts_for(
        source: Source,
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
            source,
            base_url: base_url.trim_end_matches('/').to_string(),
            user_id,
            api_key,
            max_retries,
        })
    }

    pub fn source(&self) -> Source {
        self.source
    }

    /// Rule34 requires API credentials for the post/tag endpoints. Failing
    /// early gives the TUI a useful message instead of a misleading empty
    /// result set.
    fn require_credentials(&self) -> Result<()> {
        if self.source == Source::Rule34 && (self.user_id.is_empty() || self.api_key.is_empty()) {
            return Err(AppError::Auth(
                "Rule34 needs an API key and user ID. Add them in Settings (Rule34 user ID / \
                 Rule34 API key) or under [rule34] in config.toml."
                    .to_string(),
            ));
        }
        Ok(())
    }

    pub async fn get_post(&self, id: i64) -> Result<crate::api::models::Post> {
        self.require_credentials()?;
        let url = format!("{}/index.php", self.base_url);
        let mut params: Vec<(&str, String)> = vec![
            ("page", "dapi".to_string()),
            ("s", "post".to_string()),
            ("q", "index".to_string()),
            ("id", id.to_string()),
            ("json", "1".to_string()),
        ];
        if self.source == Source::Rule34 {
            params.push(("fields", "tag_info".to_string()));
        }
        let req = self.http.get(&url).query(&params);
        let req = self.apply_auth(req);
        let resp = self.send_with_retry(req).await?;
        let text = resp
            .text()
            .await
            .map_err(|e| AppError::Http(e.to_string()))?;
        if text.trim().is_empty() {
            return Err(AppError::NotFound(format!("post {}", id)));
        }
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            let snippet: String = text.chars().take(200).collect();
            AppError::Api(format!("json parse: {} body={}", e, snippet))
        })?;
        Self::check_api_message(&v)?;
        let posts = crate::api::models::parse_posts(&v);
        posts
            .into_iter()
            .next()
            .ok_or_else(|| AppError::NotFound(format!("post {}", id)))
    }

    /// Rule34 reports some failures as a bare JSON string or a
    /// `{"success": false, "message": ...}` object. Turn those into readable
    /// errors (notably the authentication message).
    fn check_api_message(v: &serde_json::Value) -> Result<()> {
        let msg = if let Some(msg) = v.as_str() {
            msg.trim().to_string()
        } else if let Some(obj) = v.as_object() {
            if obj.get("success").and_then(|s| s.as_bool()) == Some(false) {
                obj.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("search failed")
                    .trim()
                    .to_string()
            } else {
                return Ok(());
            }
        } else {
            return Ok(());
        };
        if msg.is_empty() {
            return Ok(());
        }
        if msg.to_ascii_lowercase().contains("authentication") {
            return Err(AppError::Auth(
                "Rule34 rejected the API credentials. Check the Rule34 user ID (numeric) and API \
                 key (long hex string) in Settings or config.toml; they are easy to swap."
                    .to_string(),
            ));
        }
        Err(AppError::Api(msg))
    }

    pub async fn search_page(
        &self,
        tags: &str,
        limit: u32,
        page: u32,
        sort: Option<&str>,
        order: Option<&str>,
    ) -> Result<(Vec<crate::api::models::Post>, i64)> {
        self.require_credentials()?;
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
        if self.source == Source::Rule34 {
            // Rule34 can return per-post tag types, which the search columns
            // and viewer use to find character/series tags.
            params.push(("fields", "tag_info".to_string()));
        }
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
        // Retry the whole request on a non-JSON/empty body (often a transient
        // rate-limit or gateway response) with exponential backoff.
        let mut attempt = 0u32;
        let v = loop {
            let req = self
                .http
                .get(&url)
                .query(&params)
                .try_clone()
                .ok_or_else(|| AppError::Http("cannot clone request".into()))?;
            let req = self.apply_auth(req);
            let resp = self.send_with_retry(req).await?;
            let text = resp
                .text()
                .await
                .map_err(|e| AppError::Http(e.to_string()))?;
            // Gelbooru caps deep pagination with a non-JSON message.
            if text.contains("Too deep") {
                return Err(AppError::TooDeep);
            }
            // Rule34 returns an empty body when the query matches nothing.
            if text.trim().is_empty() {
                break serde_json::Value::Array(Vec::new());
            }
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(v) => {
                    Self::check_api_message(&v)?;
                    break v;
                }
                Err(e) => {
                    attempt += 1;
                    if attempt > self.max_retries {
                        return Err(AppError::Api(format!(
                            "json parse: {} body={}",
                            e,
                            text.chars().take(120).collect::<String>()
                        )));
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(2u64.pow(attempt.min(6))))
                        .await;
                }
            }
        };
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

    /// Autocomplete tags matching `prefix`. The three lookups run concurrently.
    pub async fn tag_autocomplete(&self, prefix: &str, _limit: u32) -> Result<Vec<(String, i64)>> {
        if self.source == Source::Rule34 {
            return self.rule34_autocomplete(prefix).await;
        }
        let self1 = self.clone();
        let self2 = self.clone();
        let self3 = self.clone();
        let prefix1 = prefix.to_string();
        let prefix2 = prefix.to_string();
        let prefix3 = prefix.to_string();

        let t1 = tokio::spawn(async move {
            let url = format!("{}/index.php", self1.base_url);
            let mut req = self1.http.get(&url).query(&[
                ("page", "autocomplete2"),
                ("term", &prefix1),
                ("type", "tag_query"),
                ("limit", "10"),
            ]);
            req = self1.apply_auth(req);
            let mut merged: std::collections::BTreeMap<String, i64> =
                std::collections::BTreeMap::new();
            if let Ok(resp) = self1.send_with_retry(req).await {
                if let Ok(text) = resp.text().await {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                        if let Some(arr) = v.as_array() {
                            for item in arr {
                                let value = item
                                    .get("value")
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let count = item
                                    .get("post_count")
                                    .and_then(|c| c.as_str())
                                    .and_then(|s| s.parse::<i64>().ok())
                                    .or_else(|| item.get("post_count").and_then(|c| c.as_i64()))
                                    .unwrap_or(0);
                                if !value.is_empty() {
                                    merged
                                        .entry(value)
                                        .and_modify(|c| *c = (*c).max(count))
                                        .or_insert(count);
                                }
                            }
                        }
                    }
                }
            }
            merged
        });

        let t2 = tokio::spawn(async move {
            let url = format!("{}/index.php", self2.base_url);
            let pattern = format!("{}%", prefix2);
            let mut req = self2.http.get(&url).query(&[
                ("page", "dapi"),
                ("s", "tag"),
                ("q", "index"),
                ("name_pattern", &pattern),
                ("limit", "100"),
                ("json", "1"),
            ]);
            req = self2.apply_auth(req);
            let mut merged: std::collections::BTreeMap<String, i64> =
                std::collections::BTreeMap::new();
            if let Ok(resp) = self2.send_with_retry(req).await {
                if let Ok(text) = resp.text().await {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                        if let Some(arr) = v.get("tag").and_then(|t| t.as_array()) {
                            for t in arr {
                                let name = t
                                    .get("name")
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let count = t.get("count").and_then(|c| c.as_i64()).unwrap_or(0);
                                if !name.is_empty() {
                                    merged
                                        .entry(name)
                                        .and_modify(|c| *c = (*c).max(count))
                                        .or_insert(count);
                                }
                            }
                        }
                    }
                }
            }
            merged
        });

        let t3 = tokio::spawn(async move {
            let url = format!("{}/index.php", self3.base_url);
            let mut req = self3.http.get(&url).query(&[
                ("page", "dapi"),
                ("s", "tag"),
                ("q", "index"),
                ("name", &prefix3),
                ("json", "1"),
            ]);
            req = self3.apply_auth(req);
            let mut merged: std::collections::BTreeMap<String, i64> =
                std::collections::BTreeMap::new();
            if let Ok(resp) = self3.send_with_retry(req).await {
                if let Ok(text) = resp.text().await {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                        if let Some(arr) = v.get("tag").and_then(|t| t.as_array()) {
                            for t in arr {
                                let name = t
                                    .get("name")
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let count = t.get("count").and_then(|c| c.as_i64()).unwrap_or(0);
                                if !name.is_empty() {
                                    merged
                                        .entry(name)
                                        .and_modify(|c| *c = (*c).max(count))
                                        .or_insert(count);
                                }
                            }
                        } else if let Some(t) = v.get("tag") {
                            if let Some(name) = t.get("name").and_then(|n| n.as_str()) {
                                let count = t.get("count").and_then(|c| c.as_i64()).unwrap_or(0);
                                if !name.is_empty() {
                                    merged
                                        .entry(name.to_string())
                                        .and_modify(|c| *c = (*c).max(count))
                                        .or_insert(count);
                                }
                            }
                        }
                    }
                }
            }
            merged
        });

        let (r1, r2, r3) = tokio::join!(t1, t2, t3);
        let mut merged = r1.unwrap_or_default();
        for (k, v) in r2.unwrap_or_default() {
            merged
                .entry(k)
                .and_modify(|c| *c = (*c).max(v))
                .or_insert(v);
        }
        for (k, v) in r3.unwrap_or_default() {
            merged
                .entry(k)
                .and_modify(|c| *c = (*c).max(v))
                .or_insert(v);
        }

        let mut out: Vec<(String, i64)> = merged.into_iter().collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out.truncate(20);
        Ok(out)
    }

    /// Rule34 exposes a single autocomplete endpoint (no auth required). Its
    /// labels look like `tag_name (1234)`, so the post count is parsed from
    /// there when no `post_count` field is present.
    async fn rule34_autocomplete(&self, prefix: &str) -> Result<Vec<(String, i64)>> {
        let url = format!("{}/autocomplete.php", self.base_url);
        let req = self.http.get(&url).query(&[("q", prefix)]);
        let resp = self.send_with_retry(req).await?;
        let text = resp
            .text()
            .await
            .map_err(|e| AppError::Http(e.to_string()))?;
        if text.trim().is_empty() {
            return Ok(Vec::new());
        }
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            AppError::Api(format!(
                "autocomplete json parse: {} body={}",
                e,
                text.chars().take(120).collect::<String>()
            ))
        })?;
        let mut out = Vec::new();
        for item in v.as_array().into_iter().flatten() {
            let value = item
                .get("value")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            if value.is_empty() {
                continue;
            }
            let count = item
                .get("post_count")
                .and_then(|c| c.as_i64())
                .or_else(|| parse_label_count(item.get("label").and_then(|l| l.as_str())))
                .unwrap_or(0);
            out.push((value, count));
        }
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out.truncate(20);
        Ok(out)
    }

    /// Fetch tag metadata (name -> type) for the given tag names. Names are
    /// batched (max ~100 per request due to URL length limits) and the
    /// batches run concurrently for speed.
    pub async fn tags_by_names(&self, names: &[String]) -> Result<Vec<(String, i64)>> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        // Rule34 has no reliable name lookup for tag types; search responses
        // already include `tag_info`, so callers seed their cache from that.
        if self.source == Source::Rule34 {
            return Ok(Vec::new());
        }
        const BATCH: usize = 100;
        let mut handles = Vec::new();
        for chunk in names.chunks(BATCH) {
            let client = self.clone();
            let chunk: Vec<String> = chunk.to_vec();
            handles.push(tokio::spawn(async move {
                let url = format!("{}/index.php", client.base_url);
                let joined = chunk.join(" ");
                let mut req = client.http.get(&url).query(&[
                    ("page", "dapi"),
                    ("s", "tag"),
                    ("q", "index"),
                    ("names", &joined),
                    ("limit", "1000"),
                    ("json", "1"),
                ]);
                req = client.apply_auth(req);
                let mut out = Vec::new();
                if let Ok(resp) = client.send_with_retry(req).await {
                    if let Ok(text) = resp.text().await {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                            if let Some(arr) = v.get("tag").and_then(|t| t.as_array()) {
                                for t in arr {
                                    let name = t
                                        .get("name")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let ty = t.get("type").and_then(|c| c.as_i64()).unwrap_or(0);
                                    if !name.is_empty() {
                                        out.push((name, ty));
                                    }
                                }
                            } else if let Some(t) = v.get("tag") {
                                if let Some(name) = t.get("name").and_then(|n| n.as_str()) {
                                    let ty = t.get("type").and_then(|c| c.as_i64()).unwrap_or(0);
                                    if !name.is_empty() {
                                        out.push((name.to_string(), ty));
                                    }
                                }
                            }
                        }
                    }
                }
                out
            }));
        }
        let mut out = Vec::new();
        for h in handles {
            if let Ok(batch) = h.await {
                out.extend(batch);
            }
        }
        Ok(out)
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

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn rule34_config(server: &MockServer) -> Config {
        let mut cfg = Config::default();
        cfg.rule34.base_url = server.uri();
        cfg.rule34.user_id = "42".to_string();
        cfg.rule34.api_key = "secret".to_string();
        cfg
    }

    #[test]
    fn source_names_round_trip() {
        assert_eq!(Source::from_name("gelbooru"), Source::Gelbooru);
        assert_eq!(Source::from_name(" RULE34 "), Source::Rule34);
        assert_eq!(Source::from_name("r34"), Source::Rule34);
        assert_eq!(Source::from_name("nonsense"), Source::Gelbooru);
        assert_eq!(Source::Gelbooru.toggle(), Source::Rule34);
        assert_eq!(Source::Rule34.as_str(), "rule34");
        assert!(Source::Rule34.website().contains("rule34"));
    }

    #[tokio::test]
    async fn rule34_search_sends_credentials_and_tag_info() {
        let server = MockServer::start().await;
        let body = r#"[{"id":7,"file_url":"https://video-cdn.rule34.xxx/x.mp4","width":100,"height":50,"hash":"abc","tag_info":[{"tag":"hero","type":"character","count":1}]}]"#;
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .and(query_param("user_id", "42"))
            .and(query_param("api_key", "secret"))
            .and(query_param("fields", "tag_info"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;

        let cfg = rule34_config(&server);
        let client = GelbooruClient::new_for(&cfg, Source::Rule34).unwrap();
        let (posts, _) = client.search_page("hero", 10, 1, None, None).await.unwrap();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].md5, "abc");
        assert_eq!(posts[0].tag_info[0].type_id(), 4);
    }

    #[tokio::test]
    async fn rule34_requires_credentials_before_requesting() {
        let cfg = Config::default();
        let client = GelbooruClient::new_for(&cfg, Source::Rule34).unwrap();
        let err = client
            .search_page("hero", 10, 1, None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Auth(_)));
        assert!(err.to_string().contains("Rule34"));
    }

    #[tokio::test]
    async fn rule34_authentication_message_becomes_auth_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "\"Missing authentication. Go to api.rule34.xxx for more information\"",
            ))
            .mount(&server)
            .await;
        let cfg = rule34_config(&server);
        let client = GelbooruClient::new_for(&cfg, Source::Rule34).unwrap();
        let err = client
            .search_page("hero", 10, 1, None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Auth(_)));
    }

    #[tokio::test]
    async fn rule34_empty_body_means_no_results() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .respond_with(ResponseTemplate::new(200).set_body_string(""))
            .mount(&server)
            .await;
        let cfg = rule34_config(&server);
        let client = GelbooruClient::new_for(&cfg, Source::Rule34).unwrap();
        let (posts, total) = client.search_page("hero", 10, 1, None, None).await.unwrap();
        assert!(posts.is_empty());
        assert_eq!(total, 0);
    }

    #[tokio::test]
    async fn rule34_error_object_becomes_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"success":false,"message":"search down"}"#),
            )
            .mount(&server)
            .await;
        let cfg = rule34_config(&server);
        let client = GelbooruClient::new_for(&cfg, Source::Rule34).unwrap();
        let err = client
            .search_page("hero", 10, 1, None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Api(_)));
        assert!(err.to_string().contains("search down"));
    }

    #[tokio::test]
    async fn rule34_autocomplete_parses_label_counts() {
        let server = MockServer::start().await;
        let body = r#"[
            {"label":"aisha_greyrat (318)","value":"aisha_greyrat"},
            {"label":"aisha (1048)","value":"aisha"}
        ]"#;
        Mock::given(method("GET"))
            .and(path("/autocomplete.php"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;
        let cfg = rule34_config(&server);
        let client = GelbooruClient::new_for(&cfg, Source::Rule34).unwrap();
        let tags = client.tag_autocomplete("aisha", 20).await.unwrap();
        assert_eq!(tags[0], ("aisha".to_string(), 1048));
        assert_eq!(tags[1], ("aisha_greyrat".to_string(), 318));
    }

    #[test]
    fn label_count_parser_handles_missing_parens() {
        assert_eq!(parse_label_count(Some("tag (12)")), Some(12));
        assert_eq!(parse_label_count(Some("tag")), None);
        assert_eq!(parse_label_count(None), None);
    }

    #[test]
    fn rule34_swapped_credentials_are_recovered() {
        let key = "0123456789abcdef".repeat(8);
        let mut cfg = Config::default();
        cfg.rule34.user_id = key.clone();
        cfg.rule34.api_key = " 1234567 ".to_string();
        let client = GelbooruClient::new_for(&cfg, Source::Rule34).unwrap();
        assert_eq!(client.user_id, "1234567");
        assert_eq!(client.api_key, key);
    }

    #[test]
    fn correct_rule34_credentials_are_left_alone() {
        let key = "0123456789abcdef".repeat(8);
        let mut cfg = Config::default();
        cfg.rule34.user_id = "1234567".to_string();
        cfg.rule34.api_key = key.clone();
        let client = GelbooruClient::new_for(&cfg, Source::Rule34).unwrap();
        assert_eq!(client.user_id, "1234567");
        assert_eq!(client.api_key, key);
    }

    #[test]
    fn referer_matches_the_cdn() {
        assert_eq!(
            referer_for_url("https://img4.gelbooru.com/x.jpg"),
            "https://gelbooru.com/"
        );
        assert_eq!(
            referer_for_url("https://video-cdn.rule34.xxx/x.mp4"),
            "https://rule34.xxx/"
        );
    }
}
