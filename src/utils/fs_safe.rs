use std::path::Path;

pub fn safe_join(base: &Path, name: &str) -> std::path::PathBuf {
    let sanitized = crate::utils::paths::sanitize_filename(name);
    base.join(sanitized)
}
