pub mod error;
pub mod fs_safe;
pub mod paths;
pub mod retry;

pub use error::AppError;

/// Shorten `s` to at most `max` characters, appending an ellipsis when it was
/// cut. Counting characters (not bytes) keeps the result valid UTF-8.
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::truncate_chars;

    #[test]
    fn truncation_counts_characters_not_bytes() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello", 4), "hel…");
        assert_eq!(truncate_chars("日本語のタグ", 3), "日本…");
        assert_eq!(truncate_chars("hello", 0), "");
    }
}
