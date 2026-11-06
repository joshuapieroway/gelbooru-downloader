use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Post {
    #[serde(default, alias = "id")]
    pub id: i64,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub score: i64,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub md5: String,
    #[serde(default)]
    pub rating: String,
    #[serde(default, alias = "image_width")]
    pub width: i64,
    #[serde(default, alias = "image_height")]
    pub height: i64,
    #[serde(default)]
    pub owner_id: String,
    #[serde(default)]
    pub parent_id: i64,
    #[serde(default)]
    pub sample: i64,
    #[serde(default)]
    pub preview_url: String,
    #[serde(default)]
    pub sample_url: String,
    #[serde(default)]
    pub file_url: String,
    #[serde(default)]
    pub tags: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub has_notes: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub file_size: i64,
    #[serde(default)]
    pub change: i64,
}

impl Post {
    pub fn tag_list(&self) -> Vec<String> {
        self.tags
            .split_whitespace()
            .map(|s| s.to_string())
            .collect()
    }
    pub fn extension(&self) -> String {
        // Take the extension from the URL path only, not from the hostname or
        // query string (`http://x.co/a` must not yield `co/a`).
        let path = url::Url::parse(&self.file_url)
            .ok()
            .map(|u| u.path().to_string())
            .unwrap_or_else(|| {
                self.file_url
                    .split(['?', '#'])
                    .next()
                    .unwrap_or_default()
                    .to_string()
            });
        let ext = std::path::Path::new(&path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        if !ext.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
            ext.to_ascii_lowercase()
        } else {
            String::new()
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SearchResponse {
    #[serde(default, alias = "post")]
    pub posts: Vec<Post>,
    #[serde(default, rename = "@attributes", alias = "attributes")]
    pub attributes: Option<Attributes>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Attributes {
    #[serde(default)]
    pub count: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
}

/// Normalize various Gelbooru JSON shapes into Vec<Post>.
pub fn parse_posts(value: &serde_json::Value) -> Vec<Post> {
    // Case 1: array of posts
    if let Some(arr) = value.as_array() {
        return arr
            .iter()
            .filter_map(|v| serde_json::from_value(v.clone()).ok())
            .collect();
    }
    // Case 2: object with "post" key
    if let Some(obj) = value.as_object() {
        if let Some(post) = obj.get("post") {
            if let Some(arr) = post.as_array() {
                return arr
                    .iter()
                    .filter_map(|v| serde_json::from_value(v.clone()).ok())
                    .collect();
            }
            if let Some(single) = post.as_object() {
                if let Ok(p) = serde_json::from_value::<Post>(post.clone()) {
                    return vec![p];
                }
                let _ = single;
            }
        }
        // Some responses: {"@attributes":..., "post":[...]} handled above.
        // Maybe top-level is a post object with id
        if obj.contains_key("file_url") || obj.contains_key("id") {
            if let Ok(p) = serde_json::from_value::<Post>(value.clone()) {
                return vec![p];
            }
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_array() {
        let json = r#"[{"id":1,"file_url":"http://x/a.jpg","rating":"safe","image_width":10,"image_height":20,"tags":"a b"}]"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let posts = parse_posts(&v);
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].id, 1);
        assert_eq!(posts[0].width, 10);
    }
    #[test]
    fn parse_object_with_post() {
        let json = r#"{"post":[{"id":2,"file_url":"http://x/b.png"}]}"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let posts = parse_posts(&v);
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].id, 2);
    }
    #[test]
    fn extension() {
        let mut p = Post {
            file_url: "http://x/y.jpeg?z=1".to_string(),
            ..Post::default()
        };
        assert_eq!(p.extension(), "jpeg");

        p.file_url = "https://img4.gelbooru.com/images/ab/cd/123.PNG".to_string();
        assert_eq!(p.extension(), "png");

        // Short TLDs and dots in the path are not file extensions.
        p.file_url = "http://x.co/a".to_string();
        assert_eq!(p.extension(), "");
        p.file_url = "http://x/a.b".to_string();
        assert_eq!(p.extension(), "b");
        p.file_url = String::new();
        assert_eq!(p.extension(), "");
    }
}
