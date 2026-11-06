use crate::api::models::Post;

#[derive(Debug, Clone, Default)]
pub struct FilterRules {
    pub ratings: Vec<String>, // safe/questionable/explicit (or aliases)
    pub min_width: Option<u32>,
    pub max_width: Option<u32>,
    pub min_height: Option<u32>,
    pub max_height: Option<u32>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub min_score: Option<i64>,
    pub max_score: Option<i64>,
    pub extensions: Vec<String>,
    pub min_id: Option<i64>,
    pub max_id: Option<i64>,
    pub include_tags: Vec<String>,
    pub exclude_tags: Vec<String>,
}

impl FilterRules {
    pub fn matches(&self, post: &Post) -> bool {
        if !self.ratings.is_empty() {
            let allowed: Vec<String> = self.ratings.iter().map(|r| normalize_rating(r)).collect();
            let pr = normalize_rating(&post.rating);
            if !allowed.iter().any(|r| r.eq_ignore_ascii_case(&pr)) {
                return false;
            }
        }
        if let Some(v) = self.min_width {
            if post.width < v as i64 {
                return false;
            }
        }
        if let Some(v) = self.max_width {
            if post.width > v as i64 {
                return false;
            }
        }
        if let Some(v) = self.min_height {
            if post.height < v as i64 {
                return false;
            }
        }
        if let Some(v) = self.max_height {
            if post.height > v as i64 {
                return false;
            }
        }
        if let Some(v) = self.min_size {
            if post.file_size > 0 && (post.file_size as u64) < v {
                return false;
            }
        }
        if let Some(v) = self.max_size {
            if post.file_size > 0 && (post.file_size as u64) > v {
                return false;
            }
        }
        if let Some(v) = self.min_score {
            if post.score < v {
                return false;
            }
        }
        if let Some(v) = self.max_score {
            if post.score > v {
                return false;
            }
        }
        if let Some(v) = self.min_id {
            if post.id < v {
                return false;
            }
        }
        if let Some(v) = self.max_id {
            if post.id > v {
                return false;
            }
        }
        if !self.extensions.is_empty() {
            let ext = post.extension();
            if !self.extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext)) {
                return false;
            }
        }
        let tags: Vec<String> = post.tag_list().iter().map(|t| t.to_lowercase()).collect();
        for t in &self.include_tags {
            if !tags.contains(&t.to_lowercase()) {
                return false;
            }
        }
        for t in &self.exclude_tags {
            if tags.contains(&t.to_lowercase()) {
                return false;
            }
        }
        true
    }

    pub fn filter_posts(&self, posts: Vec<Post>) -> Vec<Post> {
        posts.into_iter().filter(|p| self.matches(p)).collect()
    }
}

pub fn normalize_rating(r: &str) -> String {
    match r.trim().to_lowercase().as_str() {
        "safe" | "general" | "g" => "general".to_string(),
        "sensitive" | "s" => "sensitive".to_string(),
        "questionable" | "q" => "questionable".to_string(),
        "explicit" | "e" | "nsfw" => "explicit".to_string(),
        other => other.to_string(),
    }
}

pub fn parse_ratings(s: &str) -> Vec<String> {
    s.split(',')
        .map(normalize_rating)
        .filter(|x| {
            !x.is_empty() && !x.eq_ignore_ascii_case("all") && !x.eq_ignore_ascii_case("any")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post() -> Post {
        Post {
            id: 10,
            rating: "safe".to_string(),
            width: 1920,
            height: 1080,
            file_size: 1000,
            score: 5,
            file_url: "https://x/a.jpg".to_string(),
            tags: "landscape sky".to_string(),
            ..Post::default()
        }
    }

    #[test]
    fn rating_filter() {
        let mut r = FilterRules {
            ratings: vec!["safe".to_string()],
            ..FilterRules::default()
        };
        assert!(r.matches(&post()));
        r.ratings = vec!["explicit".to_string()];
        assert!(!r.matches(&post()));
    }

    #[test]
    fn dimension_filter() {
        let mut r = FilterRules {
            min_width: Some(2000),
            ..FilterRules::default()
        };
        assert!(!r.matches(&post()));
        r.min_width = Some(1000);
        assert!(r.matches(&post()));
    }

    #[test]
    fn exclude_tags() {
        let r = FilterRules {
            exclude_tags: vec!["sky".to_string()],
            ..FilterRules::default()
        };
        assert!(!r.matches(&post()));
    }

    #[test]
    fn include_tags() {
        let r = FilterRules {
            include_tags: vec!["missing".to_string()],
            ..FilterRules::default()
        };
        assert!(!r.matches(&post()));
    }

    #[test]
    fn ext_filter() {
        let mut r = FilterRules {
            extensions: vec!["png".to_string()],
            ..FilterRules::default()
        };
        assert!(!r.matches(&post()));
        r.extensions = vec!["jpg".to_string()];
        assert!(r.matches(&post()));
    }
}
