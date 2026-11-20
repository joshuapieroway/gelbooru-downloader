use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
}

fn default_base_url() -> String {
    "https://gelbooru.com".to_string()
}
fn default_timeout() -> u64 {
    30
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            user_id: String::new(),
            api_key: String::new(),
            timeout: default_timeout(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule34Config {
    #[serde(default = "default_rule34_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub api_key: String,
}

fn default_rule34_base_url() -> String {
    "https://api.rule34.xxx".to_string()
}

impl Default for Rule34Config {
    fn default() -> Self {
        Self {
            base_url: default_rule34_base_url(),
            user_id: String::new(),
            api_key: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadsConfig {
    #[serde(default = "default_dir")]
    pub directory: String,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    #[serde(default = "default_retries")]
    pub max_retries: u32,
    #[serde(default = "default_true")]
    pub resume: bool,
    #[serde(default = "default_true")]
    pub validate_files: bool,
    #[serde(default = "default_true")]
    pub skip_duplicates: bool,
    #[serde(default)]
    pub bandwidth_limit: Option<u64>,
    #[serde(default = "default_filename_template")]
    pub filename_template: String,
    #[serde(default)]
    pub folder_template: String,
    /// When true, append the series (copyright) to the extracted character
    /// name, e.g. `hero` -> `hero (series)`.
    #[serde(default)]
    pub character_include_series: bool,
    /// Directory for quick downloads from the image viewer. Empty/unset
    /// defaults to `<download_dir>/quick-downloads`.
    #[serde(default)]
    pub quick_download_directory: Option<String>,
}

fn default_dir() -> String {
    "~/Pictures/Gelbooru".to_string()
}
fn default_concurrency() -> usize {
    16
}
fn default_retries() -> u32 {
    5
}
fn default_filename_template() -> String {
    crate::downloader::naming::DEFAULT_FILENAME_TEMPLATE.to_string()
}
fn default_true() -> bool {
    true
}

impl Default for DownloadsConfig {
    fn default() -> Self {
        Self {
            directory: default_dir(),
            concurrency: default_concurrency(),
            max_retries: default_retries(),
            resume: true,
            validate_files: true,
            skip_duplicates: true,
            bandwidth_limit: None,
            filename_template: default_filename_template(),
            folder_template: String::new(),
            character_include_series: false,
            quick_download_directory: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    #[serde(default = "default_rating")]
    pub default_rating: String,
    #[serde(default = "default_limit")]
    pub default_limit: u32,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    /// Image board selected when the TUI opens: `gelbooru` or `rule34`.
    #[serde(default = "default_site")]
    pub default_site: String,
    /// Columns shown in the TUI search results, in order. Valid values:
    /// id, rating, score, size, character, series, tags, date.
    #[serde(default = "default_search_columns")]
    pub columns: Vec<String>,
}

fn default_rating() -> String {
    "safe".to_string()
}
fn default_limit() -> u32 {
    100
}
fn default_page_size() -> u32 {
    100
}
fn default_site() -> String {
    "gelbooru".to_string()
}

fn default_search_columns() -> Vec<String> {
    vec![
        "rating".to_string(),
        "score".to_string(),
        "character".to_string(),
        "series".to_string(),
        "tags".to_string(),
    ]
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            default_rating: default_rating(),
            default_limit: default_limit(),
            page_size: default_page_size(),
            default_site: default_site(),
            columns: default_search_columns(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_interval() -> u64 {
    300
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval: default_interval(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub downloads: DownloadsConfig,
    #[serde(default)]
    pub search: SearchConfig,
    #[serde(default)]
    pub rule34: Rule34Config,
    #[serde(default)]
    pub watch: WatchConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: None,
        }
    }
}

impl Config {
    pub fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        let path = path
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::utils::paths::config_dir().join("config.toml"));
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let cfg: Config = toml::from_str(&content)?;
            Ok(cfg)
        } else {
            Ok(Config::default())
        }
    }

    pub fn save(&self, path: Option<&Path>) -> anyhow::Result<()> {
        let path = path
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::utils::paths::config_dir().join("config.toml"));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    pub fn db_path(&self) -> PathBuf {
        if let Some(p) = &self.database.path {
            crate::utils::paths::expand_tilde(p)
        } else {
            crate::utils::paths::data_dir().join("history.db")
        }
    }

    pub fn download_dir(&self) -> PathBuf {
        crate::utils::paths::expand_tilde(&self.downloads.directory)
    }

    /// Validated list of search result columns, in display order.
    pub fn search_columns(&self) -> Vec<String> {
        const VALID: &[&str] = &[
            "id",
            "rating",
            "score",
            "size",
            "character",
            "series",
            "tags",
            "date",
        ];
        let mut out: Vec<String> = self
            .search
            .columns
            .iter()
            .map(|c| c.trim().to_ascii_lowercase())
            .filter(|c| VALID.contains(&c.as_str()))
            .collect();
        // de-dup
        let mut seen = std::collections::HashSet::new();
        out.retain(|c| seen.insert(c.clone()));
        if out.is_empty() {
            default_search_columns()
        } else {
            out
        }
    }

    pub fn quick_download_dir(&self) -> PathBuf {
        match &self.downloads.quick_download_directory {
            Some(p) if !p.trim().is_empty() => crate::utils::paths::expand_tilde(p),
            _ => self.download_dir().join("quick-downloads"),
        }
    }

    /// Apply environment variable overrides.
    pub fn apply_env(&mut self) {
        if let Ok(v) = std::env::var("GELBOORU_BASE_URL") {
            self.api.base_url = v;
        }
        if let Ok(v) = std::env::var("GELBOORU_USER_ID") {
            self.api.user_id = v;
        }
        if let Ok(v) = std::env::var("GELBOORU_API_KEY") {
            self.api.api_key = v;
        }
        if let Ok(v) = std::env::var("RULE34_BASE_URL") {
            self.rule34.base_url = v;
        }
        if let Ok(v) = std::env::var("RULE34_USER_ID") {
            self.rule34.user_id = v;
        }
        if let Ok(v) = std::env::var("RULE34_API_KEY") {
            self.rule34.api_key = v;
        }
        if let Ok(v) = std::env::var("GELBOORU_DOWNLOAD_DIR") {
            self.downloads.directory = v;
        }
        if let Ok(v) = std::env::var("GELBOORU_QUICK_DOWNLOAD_DIR") {
            self.downloads.quick_download_directory = Some(v);
        }
        if let Ok(v) = std::env::var("GELBOORU_CONCURRENCY") {
            if let Ok(n) = v.parse() {
                self.downloads.concurrency = n;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_usable() {
        let cfg = Config::default();
        assert_eq!(cfg.watch.interval, 300);
        assert_eq!(cfg.search.page_size, 100);
        assert_eq!(cfg.search.default_limit, 100);
        assert_eq!(cfg.downloads.concurrency, 16);
        assert_eq!(
            cfg.downloads.filename_template,
            crate::downloader::naming::DEFAULT_FILENAME_TEMPLATE
        );
        assert!(cfg.downloads.folder_template.is_empty());
        assert!(cfg.database.enabled);
    }

    #[test]
    fn partial_watch_section_keeps_the_default_interval() {
        let cfg: Config = toml::from_str("[watch]\nenabled = true\n").unwrap();
        assert!(cfg.watch.enabled);
        assert_eq!(cfg.watch.interval, 300);
    }

    #[test]
    fn partial_search_section_keeps_the_defaults() {
        let cfg: Config = toml::from_str("[search]\ndefault_rating = \"explicit\"\n").unwrap();
        assert_eq!(cfg.search.default_rating, "explicit");
        assert_eq!(cfg.search.default_limit, 100);
        assert_eq!(cfg.search.page_size, 100);
    }

    #[test]
    fn new_fields_have_sensible_defaults() {
        let cfg = Config::default();
        assert!(cfg.search.columns == vec!["rating", "score", "character", "series", "tags"]);
        assert_eq!(cfg.search.default_site, "gelbooru");
        assert_eq!(cfg.rule34.base_url, "https://api.rule34.xxx");
        assert!(cfg.rule34.user_id.is_empty());
        assert!(cfg.rule34.api_key.is_empty());
        assert!(cfg.downloads.quick_download_directory.is_none());
        assert!(!cfg.downloads.character_include_series);
        assert!(cfg.downloads.folder_template.is_empty());
        assert!(cfg.downloads.bandwidth_limit.is_none());
    }

    #[test]
    fn partial_rule34_section_keeps_defaults() {
        let cfg: Config = toml::from_str("[rule34]\nuser_id = \"42\"\n").unwrap();
        assert_eq!(cfg.rule34.user_id, "42");
        assert!(cfg.rule34.api_key.is_empty());
        assert_eq!(cfg.rule34.base_url, "https://api.rule34.xxx");
    }

    #[test]
    fn rule34_credentials_round_trip() {
        let mut cfg = Config::default();
        cfg.rule34.user_id = "123".to_string();
        cfg.rule34.api_key = "secret".to_string();
        cfg.search.default_site = "rule34".to_string();
        let s = toml::to_string_pretty(&cfg).unwrap();
        let parsed: Config = toml::from_str(&s).unwrap();
        assert_eq!(parsed.rule34.user_id, "123");
        assert_eq!(parsed.rule34.api_key, "secret");
        assert_eq!(parsed.search.default_site, "rule34");
    }

    #[test]
    fn search_columns_validates_and_dedups() {
        let mut cfg = Config::default();
        cfg.search.columns = vec![
            "RATING".to_string(),
            "bogus".to_string(),
            "score".to_string(),
            "rating".to_string(),
            "tags".to_string(),
        ];
        let cols = cfg.search_columns();
        assert_eq!(cols, vec!["rating", "score", "tags"]);
    }

    #[test]
    fn search_columns_falls_back_to_default_when_empty() {
        let mut cfg = Config::default();
        cfg.search.columns = vec![];
        let cols = cfg.search_columns();
        assert_eq!(cols, vec!["rating", "score", "character", "series", "tags"]);
        cfg.search.columns = vec!["nope".to_string()];
        let cols = cfg.search_columns();
        assert_eq!(cols, vec!["rating", "score", "character", "series", "tags"]);
    }

    #[test]
    fn quick_download_dir_defaults_to_subdir() {
        let mut cfg = Config::default();
        cfg.downloads.directory = "/tmp/gelbooru-test".to_string();
        assert_eq!(
            cfg.quick_download_dir(),
            std::path::PathBuf::from("/tmp/gelbooru-test/quick-downloads")
        );
    }

    #[test]
    fn quick_download_dir_honors_override() {
        let mut cfg = Config::default();
        cfg.downloads.directory = "/tmp/gelbooru-test".to_string();
        cfg.downloads.quick_download_directory = Some("/tmp/custom-qd".to_string());
        assert_eq!(
            cfg.quick_download_dir(),
            std::path::PathBuf::from("/tmp/custom-qd")
        );
    }

    #[test]
    fn quick_download_dir_empty_override_falls_back() {
        let mut cfg = Config::default();
        cfg.downloads.directory = "/tmp/gelbooru-test".to_string();
        cfg.downloads.quick_download_directory = Some("   ".to_string());
        assert_eq!(
            cfg.quick_download_dir(),
            std::path::PathBuf::from("/tmp/gelbooru-test/quick-downloads")
        );
    }

    #[test]
    fn full_config_round_trips_through_toml() {
        let mut cfg = Config::default();
        cfg.downloads.directory = "~/Pictures/Gelbooru".to_string();
        cfg.downloads.concurrency = 4;
        cfg.downloads.character_include_series = true;
        cfg.search.columns = vec!["id".to_string(), "rating".to_string(), "tags".to_string()];
        cfg.search.default_rating = "explicit".to_string();
        let s = toml::to_string_pretty(&cfg).unwrap();
        let parsed: Config = toml::from_str(&s).unwrap();
        assert_eq!(parsed.downloads.concurrency, 4);
        assert!(parsed.downloads.character_include_series);
        assert_eq!(parsed.search.columns, vec!["id", "rating", "tags"]);
        assert_eq!(parsed.search.default_rating, "explicit");
        assert_eq!(parsed.quick_download_dir(), cfg.quick_download_dir());
    }

    #[test]
    fn env_override_applies() {
        let mut cfg = Config::default();
        unsafe {
            std::env::set_var("GELBOORU_QUICK_DOWNLOAD_DIR", "/tmp/env-qd");
            std::env::set_var("RULE34_USER_ID", "env-user");
            std::env::set_var("RULE34_API_KEY", "env-key");
        }
        cfg.apply_env();
        assert_eq!(
            cfg.downloads.quick_download_directory.as_deref(),
            Some("/tmp/env-qd")
        );
        assert_eq!(cfg.rule34.user_id, "env-user");
        assert_eq!(cfg.rule34.api_key, "env-key");
        unsafe {
            std::env::remove_var("GELBOORU_QUICK_DOWNLOAD_DIR");
            std::env::remove_var("RULE34_USER_ID");
            std::env::remove_var("RULE34_API_KEY");
        }
    }
}
