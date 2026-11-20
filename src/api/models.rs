use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

/// Deserialize a string, accepting null/bool/number values returned by
/// different boorus instead of rejecting the whole post.
fn de_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(match v {
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::String(s)) => s,
        Some(serde_json::Value::Bool(b)) => b.to_string(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(other) => other.to_string(),
    })
}

/// Deserialize an integer, treating null/invalid values as zero. Different
/// boorus occasionally send numbers as strings.
fn de_i64<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(match v {
        Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(serde_json::Value::String(s)) => s.trim().parse().unwrap_or(0),
        Some(serde_json::Value::Bool(b)) => i64::from(b),
        _ => 0,
    })
}

/// Deserialize a vector, treating null as empty.
fn de_vec_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<TagInfo>, D::Error> {
    Ok(Option::<Vec<TagInfo>>::deserialize(d)?.unwrap_or_default())
}

/// Per-tag metadata included by boards that support `fields=tag_info`
/// (currently Rule34). `type` is a descriptive string such as `character`.
#[derive(Debug, Clone, Default)]
pub struct TagInfo {
    pub name: String,
    pub tag_type: serde_json::Value,
    pub count: i64,
}

impl<'de> Deserialize<'de> for TagInfo {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default, alias = "tag", alias = "name", deserialize_with = "de_string")]
            name: String,
            #[serde(default, rename = "type")]
            tag_type: serde_json::Value,
            #[serde(default, deserialize_with = "de_i64")]
            count: i64,
        }
        let raw = Raw::deserialize(d)?;
        Ok(TagInfo {
            name: raw.name,
            tag_type: raw.tag_type,
            count: raw.count,
        })
    }
}

impl TagInfo {
    /// Map the board's descriptive type to Gelbooru's numeric tag-type ids
    /// (0 general, 1 artist, 3 copyright, 4 character, 5 metadata).
    pub fn type_id(&self) -> i64 {
        match &self.tag_type {
            serde_json::Value::Number(n) => n.as_i64().unwrap_or(0),
            serde_json::Value::String(s) => match s.to_ascii_lowercase().as_str() {
                "artist" => 1,
                "copyright" | "series" => 3,
                "character" => 4,
                "metadata" | "meta" => 5,
                _ => 0,
            },
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Post {
    #[serde(default, alias = "id", deserialize_with = "de_i64")]
    pub id: i64,
    #[serde(default, deserialize_with = "de_string")]
    pub created_at: String,
    #[serde(default, deserialize_with = "de_i64")]
    pub score: i64,
    #[serde(default, deserialize_with = "de_string")]
    pub source: String,
    #[serde(default, alias = "hash", deserialize_with = "de_string")]
    pub md5: String,
    #[serde(default, deserialize_with = "de_string")]
    pub rating: String,
    #[serde(default, alias = "image_width", deserialize_with = "de_i64")]
    pub width: i64,
    #[serde(default, alias = "image_height", deserialize_with = "de_i64")]
    pub height: i64,
    #[serde(default, alias = "owner", deserialize_with = "de_string")]
    pub owner_id: String,
    #[serde(default, deserialize_with = "de_i64")]
    pub parent_id: i64,
    #[serde(default, deserialize_with = "de_i64")]
    pub sample: i64,
    #[serde(default, deserialize_with = "de_string")]
    pub preview_url: String,
    #[serde(default, deserialize_with = "de_string")]
    pub sample_url: String,
    #[serde(default, deserialize_with = "de_string")]
    pub file_url: String,
    #[serde(default, deserialize_with = "de_string")]
    pub tags: String,
    #[serde(default, deserialize_with = "de_string")]
    pub title: String,
    #[serde(default, deserialize_with = "de_string")]
    pub has_notes: String,
    #[serde(default, deserialize_with = "de_string")]
    pub status: String,
    #[serde(default, deserialize_with = "de_i64")]
    pub file_size: i64,
    #[serde(default, deserialize_with = "de_i64")]
    pub change: i64,
    /// Tag type metadata returned by Rule34 (`fields=tag_info`). Not persisted
    /// to the history/queue database.
    #[serde(default, skip_serializing, deserialize_with = "de_vec_opt")]
    pub tag_info: Vec<TagInfo>,
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

    #[test]
    fn parse_rule34_post_shape() {
        let json = r#"[{
            "id": 7,
            "score": 5,
            "file_url": "https://video-cdn.rule34.xxx/images/1.mp4",
            "preview_url": "https://wimg.rule34.xxx/thumbnails/1.jpg",
            "rating": "explicit",
            "width": 1920,
            "height": 1080,
            "hash": "abc123",
            "tags": "hero series artist",
            "has_notes": true,
            "source": null,
            "tag_info": [
                {"tag": "hero", "type": "character", "count": 10},
                {"tag": "series", "type": "copyright", "count": 20},
                {"tag": "artist", "type": "artist", "count": 1},
                {"tag": "meta", "type": "metadata", "count": 3}
            ]
        }]"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let posts = parse_posts(&v);
        assert_eq!(posts.len(), 1);
        let p = &posts[0];
        assert_eq!(p.md5, "abc123");
        assert_eq!(p.has_notes, "true");
        assert_eq!(p.source, "");
        assert_eq!(p.tag_info.len(), 4);
        assert_eq!(p.tag_info[0].type_id(), 4);
        assert_eq!(p.tag_info[1].type_id(), 3);
        assert_eq!(p.tag_info[2].type_id(), 1);
        assert_eq!(p.tag_info[3].type_id(), 5);
    }

    #[test]
    fn parse_rule34_nullable_fields_do_not_drop_the_post() {
        let json = r#"[{"id": 9, "width": null, "height": null, "score": null, "tag_info": null, "has_notes": null}]"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let posts = parse_posts(&v);
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].id, 9);
        assert_eq!(posts[0].width, 0);
        assert_eq!(posts[0].score, 0);
        assert!(posts[0].tag_info.is_empty());
        assert_eq!(posts[0].has_notes, "");
    }

    #[test]
    fn tag_info_is_not_persisted() {
        let json = r#"[{"id": 1, "tag_info": [{"tag": "hero", "type": "character"}]}]"#;
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let post = parse_posts(&v).pop().unwrap();
        let stored = serde_json::to_string(&post).unwrap();
        assert!(!stored.contains("tag_info"));
    }
}
