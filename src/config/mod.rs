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

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            default_rating: default_rating(),
            default_limit: default_limit(),
            page_size: default_page_size(),
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
        if let Ok(v) = std::env::var("GELBOORU_DOWNLOAD_DIR") {
            self.downloads.directory = v;
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
}
