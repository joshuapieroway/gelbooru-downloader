//! Filename and folder templates for downloaded posts.
//!
//! Templates use `{placeholder}` substitutions and `/` as the folder
//! separator. Values are sanitized before they touch the filesystem, so a tag
//! containing `../` cannot escape the configured download directory.

use crate::api::models::Post;
use crate::utils::paths::sanitize_filename;
use std::path::{Path, PathBuf};

/// The default keeps the historical `<id>_<md5>.<ext>` names, with an optional
/// sort prefix when the caller asked for a particular ordering.
pub const DEFAULT_FILENAME_TEMPLATE: &str = "{sort_prefix}{id}_{md5}.{ext}";

struct NamingContext<'a> {
    post: &'a Post,
    sort_prefix: &'a str,
}

impl NamingContext<'_> {
    fn value(&self, key: &str) -> String {
        let post = self.post;
        match key {
            "id" => post.id.to_string(),
            // The legacy filename sanitized only the md5 field, so an empty
            // hash produced `9_file.png` rather than `9_.png`.
            "md5" => sanitize_filename(&post.md5),
            "ext" => effective_extension(post),
            "rating" => post.rating.clone(),
            "score" => post.score.to_string(),
            "score_desc" => descending_score(post.score),
            "width" => post.width.to_string(),
            "height" => post.height.to_string(),
            "artist" => first_tag_value(&post.tags, "artist:"),
            "copyright" => first_tag_value(&post.tags, "copyright:"),
            "character" => first_tag_value(&post.tags, "character:"),
            "meta" => first_tag_value(&post.tags, "meta:"),
            "title" => post.title.clone(),
            "source" => source_host(&post.source),
            "tag_count" => post.tags.split_whitespace().count().to_string(),
            "year" => post
                .created_at
                .get(0..4)
                .filter(|year| year.chars().all(|c| c.is_ascii_digit()))
                .unwrap_or_default()
                .to_string(),
            "sort_prefix" => self.sort_prefix.to_string(),
            _ => String::new(),
        }
    }
}

/// Build the final path for a post below `dir`.
pub fn output_path(
    dir: &Path,
    post: &Post,
    filename_template: &str,
    folder_template: &str,
    sort_prefix: &str,
) -> PathBuf {
    let ctx = NamingContext { post, sort_prefix };
    let mut path = dir.to_path_buf();

    for segment in folder_template.split('/') {
        let segment = render(segment, &ctx);
        let segment = segment.trim();
        if segment.is_empty() || segment == "." || segment == ".." {
            continue;
        }
        path.push(sanitize_filename(segment));
    }

    let mut filename = render(filename_template, &ctx);
    if filename.trim().is_empty() {
        filename = render(DEFAULT_FILENAME_TEMPLATE, &ctx);
    }
    if Path::new(&filename).extension().is_none() {
        let ext = effective_extension(post);
        if !ext.is_empty() {
            filename.push('.');
            filename.push_str(&ext);
        }
    }
    path.push(sanitize_filename(&filename));
    path
}

fn render(template: &str, ctx: &NamingContext<'_>) -> String {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::new();
    let mut i = 0;

    while i < chars.len() {
        match chars[i] {
            '{' if chars.get(i + 1) == Some(&'{') => {
                out.push('{');
                i += 2;
            }
            '}' if chars.get(i + 1) == Some(&'}') => {
                out.push('}');
                i += 2;
            }
            '{' => {
                if let Some(offset) = chars[i + 1..].iter().position(|&c| c == '}') {
                    let key: String = chars[i + 1..i + 1 + offset].iter().collect();
                    out.push_str(&ctx.value(key.trim()));
                    i += offset + 2;
                } else {
                    out.push('{');
                    i += 1;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }

    out
}

/// Zero-padded, inverted score so ascending filename order lists the highest
/// score first. The result always has 10 digits; scores beyond the offset are
/// clamped so the width cannot change.
fn descending_score(score: i64) -> String {
    const OFFSET: i64 = 1_000_000_000;
    let inverted = OFFSET.saturating_sub(score.clamp(-OFFSET, OFFSET));
    format!("{inverted:010}")
}

fn first_tag_value(tags: &str, prefix: &str) -> String {
    tags.split_whitespace()
        .find_map(|tag| tag.strip_prefix(prefix))
        .unwrap_or_default()
        .to_string()
}

fn effective_extension(post: &Post) -> String {
    let ext = post.extension();
    if ext.is_empty() {
        "jpg".to_string()
    } else {
        ext
    }
}

fn source_host(source: &str) -> String {
    url::Url::parse(source)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post() -> Post {
        Post {
            id: 42,
            md5: "abc123".to_string(),
            rating: "safe".to_string(),
            score: 12,
            width: 1920,
            height: 1080,
            tags: "artist:some_artist copyright:series character:hero general".to_string(),
            created_at: "2024-05-06 07:08:09".to_string(),
            source: "https://www.pixiv.net/artworks/1".to_string(),
            file_url: "https://img.example/42.jpg".to_string(),
            ..Post::default()
        }
    }

    #[test]
    fn default_template_matches_the_old_filename() {
        let post = post();
        assert_eq!(
            output_path(Path::new("/tmp"), &post, DEFAULT_FILENAME_TEMPLATE, "", ""),
            PathBuf::from("/tmp/42_abc123.jpg")
        );
        assert_eq!(
            output_path(
                Path::new("/tmp"),
                &post,
                DEFAULT_FILENAME_TEMPLATE,
                "",
                "0001_"
            ),
            PathBuf::from("/tmp/0001_42_abc123.jpg")
        );
    }

    #[test]
    fn placeholders_and_folders_are_rendered() {
        let post = post();
        let path = output_path(
            Path::new("/tmp"),
            &post,
            "{artist}_{id}_{year}.{ext}",
            "{rating}/{copyright}",
            "",
        );
        assert_eq!(
            path,
            PathBuf::from("/tmp/safe/series/some_artist_42_2024.jpg")
        );
    }

    #[test]
    fn folder_templates_cannot_escape_the_output_directory() {
        let post = post();
        let path = output_path(
            Path::new("/tmp/library"),
            &post,
            "{id}.{ext}",
            "../../{rating}",
            "",
        );
        assert_eq!(path, PathBuf::from("/tmp/library/safe/42.jpg"));
        assert!(path.starts_with("/tmp/library"));
    }

    #[test]
    fn a_missing_extension_is_added() {
        let post = post();
        assert_eq!(
            output_path(Path::new("/tmp"), &post, "{id}", "", ""),
            PathBuf::from("/tmp/42.jpg")
        );
    }

    #[test]
    fn empty_and_unknown_values_do_not_break_the_name() {
        let post = Post::default();
        let path = output_path(
            Path::new("/tmp"),
            &post,
            "{unknown}_{id}_{md5}.{ext}",
            "{artist}/{rating}",
            "",
        );
        assert_eq!(path, PathBuf::from("/tmp/_0_file.jpg"));

        assert_eq!(
            output_path(Path::new("/tmp"), &post, "", "", ""),
            PathBuf::from("/tmp/0_file.jpg")
        );
    }

    #[test]
    fn escaped_braces_are_literal() {
        let post = post();
        let ctx = NamingContext {
            post: &post,
            sort_prefix: "",
        };
        assert_eq!(render("{{{id}}}.{ext}", &ctx), "{42}.jpg");
    }

    #[test]
    fn score_desc_lists_the_highest_score_first() {
        let mut names = [
            descending_score(2),
            descending_score(100),
            descending_score(0),
            descending_score(-5),
        ];
        names.sort();
        assert_eq!(
            names,
            [
                descending_score(100),
                descending_score(2),
                descending_score(0),
                descending_score(-5),
            ]
        );
        assert!(names.iter().all(|name| name.len() == 10));
    }

    #[test]
    fn score_desc_is_usable_in_a_template() {
        let post = post();
        assert_eq!(
            output_path(Path::new("/tmp"), &post, "{score_desc}_{id}.{ext}", "", ""),
            PathBuf::from("/tmp/0999999988_42.jpg")
        );
    }
}
