use directories::ProjectDirs;
use std::path::{Path, PathBuf};

pub fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("com", "gelbooru", "gelbooru-downloader")
}

pub fn config_dir() -> PathBuf {
    project_dirs()
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".config/gelbooru-downloader"))
}

pub fn data_dir() -> PathBuf {
    project_dirs()
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".local/share/gelbooru-downloader"))
}

pub fn cache_dir() -> PathBuf {
    project_dirs()
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".cache/gelbooru-downloader"))
}

pub fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    if p == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(p)
}

pub fn sanitize_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() || out == "." || out == ".." {
        out = "file".to_string();
    }
    // prevent path traversal
    out = out.replace("..", "_");
    out
}

pub fn ensure_within(dir: &Path, path: &Path) -> bool {
    if let (Ok(a), Ok(b)) = (dir.canonicalize(), path.canonicalize()) {
        b.starts_with(&a)
    } else {
        // best-effort lexical check
        path.starts_with(dir)
    }
}
