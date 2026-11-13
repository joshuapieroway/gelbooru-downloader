use crate::api::client::GelbooruClient;
use crate::api::models::Post;
use crate::config::Config;
use crate::filters::rules::FilterRules;
use crate::utils::error::Result;
use std::collections::HashSet;

/// Gelbooru serves at most 100 posts per request. Keeping the request size
/// constant matters because `pid` is a page index: page 2 with `limit=50`
/// starts at post 50, while page 2 with `limit=100` starts at post 100.
pub const MAX_PAGE_SIZE: u32 = 100;

pub struct SearchParams {
    pub tags: String,
    pub rating: Option<String>,
    pub limit: u32,
    pub page_size: u32,
    pub sort: Option<String>,
    pub order: Option<String>,
    /// Applied while paging, so `limit` counts matching posts rather than raw
    /// API results.
    pub filters: FilterRules,
}

const VALID_SORTS: &[&str] = &[
    "score",
    "id",
    "width",
    "height",
    "created",
    "date",
    "created_at",
    "rating",
];
const VALID_ORDERS: &[&str] = &["asc", "desc"];

/// Validate sort/order values early so CLI and batch/watch fail with a clear
/// message instead of silently ignoring typos. Empty strings are treated as unset.
pub fn validate_sort_order(sort: &Option<String>, order: &Option<String>) -> Result<()> {
    use crate::utils::error::AppError;
    if let Some(s) = sort {
        let s = s.trim();
        if !s.is_empty() && !VALID_SORTS.contains(&s.to_ascii_lowercase().as_str()) {
            return Err(AppError::Config(format!(
                "invalid --sort '{s}': expected one of {}",
                VALID_SORTS.join(", ")
            )));
        }
    }
    if let Some(o) = order {
        let o = o.trim();
        if !o.is_empty() && !VALID_ORDERS.contains(&o.to_ascii_lowercase().as_str()) {
            return Err(AppError::Config(format!(
                "invalid --order '{o}': expected one of {}",
                VALID_ORDERS.join(", ")
            )));
        }
    }
    Ok(())
}

impl SearchParams {
    pub fn new(tags: String) -> Self {
        Self {
            tags,
            rating: None,
            limit: 100,
            page_size: MAX_PAGE_SIZE,
            sort: None,
            order: None,
            filters: FilterRules::default(),
        }
    }
}

pub async fn search_all(
    client: &GelbooruClient,
    cfg: &Config,
    params: &SearchParams,
) -> Result<Vec<Post>> {
    validate_sort_order(&params.sort, &params.order)?;
    // An explicit `--rating` wins; otherwise the configured default applies.
    // `all`/`any` (or an empty value) disables the default, and a `rating:`
    // tag already present in the query takes precedence so a safe-by-default
    // config does not silently hide explicit searches.
    let query_has_rating = params
        .tags
        .split_whitespace()
        .any(|tag| tag.to_ascii_lowercase().starts_with("rating:"));
    let effective_rating = params
        .rating
        .clone()
        .or_else(|| {
            let default = cfg.search.default_rating.trim();
            if query_has_rating
                || default.is_empty()
                || default.eq_ignore_ascii_case("all")
                || default.eq_ignore_ascii_case("any")
            {
                None
            } else {
                Some(default.to_string())
            }
        })
        .and_then(|rating| {
            let rating = rating.trim().to_string();
            if rating.is_empty()
                || rating.eq_ignore_ascii_case("all")
                || rating.eq_ignore_ascii_case("any")
            {
                None
            } else {
                Some(rating)
            }
        });
    let mut filters = params.filters.clone();
    if filters.ratings.is_empty() {
        if let Some(rating) = &effective_rating {
            filters.ratings = crate::filters::rules::parse_ratings(rating);
        }
    }

    let mut tags = params.tags.clone();
    // An explicit `--rating` overrides a `rating:` tag in the query instead of
    // combining with it into an impossible AND filter.
    if effective_rating.is_some() && query_has_rating {
        tags = tags
            .split_whitespace()
            .filter(|tag| !tag.to_ascii_lowercase().starts_with("rating:"))
            .collect::<Vec<_>>()
            .join(" ");
    }
    // Only push a rating tag when a single rating is requested; multiple ratings
    // are filtered client-side after fetching.
    if let Some(r) = &effective_rating {
        let ratings: Vec<&str> = r
            .split(',')
            .map(|x| x.trim())
            .filter(|x| !x.is_empty())
            .collect();
        if ratings.len() == 1 {
            let t = rating_tag(ratings[0]);
            if !t.is_empty() {
                if !tags.trim().is_empty() {
                    tags.push(' ');
                }
                tags.push_str(t);
            }
        }
    }
    let page_size = params.page_size.clamp(1, MAX_PAGE_SIZE);
    let total_limit = params.limit; // 0 means unlimited
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    // Gelbooru caps a single search at ~1000 results ("Too deep!"). To
    // fetch more (e.g. `--limit 0`), we continue with an `id:<min>` filter
    // that skips already-collected posts, repeating until no new posts arrive.
    let mut min_id: Option<i64> = None;
    let mut round = 0u32;
    'outer: loop {
        if total_limit > 0 && out.len() as u32 >= total_limit {
            break;
        }
        if round >= 50 {
            break;
        }
        round += 1;
        let mut page = 1u32;
        let mut hit_too_deep = false;
        let mut collected_this_round = 0usize;
        loop {
            if total_limit > 0 && out.len() as u32 >= total_limit {
                break;
            }
            // Always request a full page, even when only a few results are still
            // wanted. Shrinking the last page changes its `pid` offset and would
            // return posts that were already fetched.
            let search_result = client
                .search_page(
                    &tags,
                    page_size,
                    page,
                    params.sort.as_deref(),
                    params.order.as_deref(),
                )
                .await;
            let posts = match search_result {
                Ok((p, _)) => p,
                Err(crate::utils::error::AppError::TooDeep) => {
                    hit_too_deep = true;
                    break;
                }
                Err(e) => return Err(e),
            };
            let fetched = posts.len();
            if fetched == 0 {
                break;
            }
            let mut new_ids = 0usize;
            for post in posts {
                let id_is_new = post.id == 0 || seen.insert(post.id);
                if !id_is_new {
                    continue;
                }
                new_ids += 1;
                if post.id != 0 {
                    min_id = Some(min_id.map_or(post.id, |m| m.min(post.id)));
                }
                if !filters.matches(&post) {
                    continue;
                }
                out.push(post);
                if total_limit > 0 && out.len() as u32 >= total_limit {
                    break;
                }
            }
            collected_this_round += new_ids;
            // A server that ignores `pid` would otherwise loop forever.
            if new_ids == 0 || fetched < page_size as usize {
                break;
            }
            page += 1;
            // Politeness delay to avoid Gelbooru rate-limiting (HTTP 429) on
            // large paginated searches.
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        if !hit_too_deep {
            break 'outer;
        }
        // Continue past the deep-pagination cap using an id filter.
        if min_id.is_none() || collected_this_round == 0 {
            break 'outer;
        }
        let mid = min_id.unwrap();
        // Drop any previous `id:<...` filter and apply the new (lower) bound
        // so we can keep paging through the remaining posts.
        let base_tags: Vec<&str> = tags
            .split_whitespace()
            .filter(|t| !t.to_ascii_lowercase().starts_with("id:<"))
            .collect();
        let new_tags = base_tags.join(" ");
        tags = format!("{} id:<{}", new_tags, mid);
    }
    if total_limit > 0 {
        out.truncate(total_limit as usize);
    }
    // client-side sort
    if let Some(sort) = &params.sort {
        let desc = params
            .order
            .as_deref()
            .map(|o| o.eq_ignore_ascii_case("desc"))
            .unwrap_or(true);
        out.sort_by(|a, b| {
            let cmp = match sort.to_lowercase().as_str() {
                "score" => a.score.cmp(&b.score),
                "id" => a.id.cmp(&b.id),
                "width" => a.width.cmp(&b.width),
                "height" => a.height.cmp(&b.height),
                "created" | "date" | "created_at" => a.created_at.cmp(&b.created_at),
                "rating" => a.rating.cmp(&b.rating),
                _ => a.id.cmp(&b.id),
            };
            if desc { cmp.reverse() } else { cmp }
        });
    }
    Ok(out)
}

fn rating_tag(r: &str) -> &'static str {
    match crate::filters::rules::normalize_rating(r).as_str() {
        "general" => "rating:general",
        "sensitive" => "rating:sensitive",
        "questionable" => "rating:questionable",
        "explicit" => "rating:explicit",
        _ => "",
    }
}

pub async fn inspect(client: &GelbooruClient, id: i64) -> Result<Post> {
    client.get_post(id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filters::rules::FilterRules;
    use crate::utils::error::AppError;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn post(id: i64, width: i64) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "file_url": format!("http://x/{id}.jpg"),
            "rating": "safe",
            "image_width": width,
            "image_height": 100,
            "tags": "tag",
            "score": 1,
        })
    }

    fn body(posts: &[serde_json::Value]) -> String {
        serde_json::to_string(posts).unwrap()
    }

    #[tokio::test]
    async fn limit_smaller_than_page_does_not_shrink_next_page() {
        let server = MockServer::start().await;
        let first: Vec<_> = (0..100).map(|i| post(i + 1, 100)).collect();
        let second: Vec<_> = (100..150).map(|i| post(i + 1, 100)).collect();
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .and(query_param("pid", "0"))
            .and(query_param("limit", "100"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body(&first)))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .and(query_param("pid", "1"))
            .and(query_param("limit", "100"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body(&second)))
            .mount(&server)
            .await;

        let mut cfg = Config::default();
        cfg.api.base_url = server.uri();
        let client = GelbooruClient::new(&cfg).unwrap();
        let params = SearchParams {
            tags: "tag".into(),
            limit: 150,
            ..SearchParams::new("tag".into())
        };
        let posts = search_all(&client, &cfg, &params).await.unwrap();
        assert_eq!(posts.len(), 150);
        assert_eq!(posts[0].id, 1);
        assert_eq!(posts[149].id, 150);
    }

    #[tokio::test]
    async fn filters_are_applied_before_the_limit() {
        let server = MockServer::start().await;
        let first: Vec<_> = (0..100).map(|i| post(i + 1, 100)).collect();
        let second = vec![post(101, 5000), post(102, 100)];
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .and(query_param("pid", "0"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body(&first)))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .and(query_param("pid", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body(&second)))
            .mount(&server)
            .await;

        let mut cfg = Config::default();
        cfg.api.base_url = server.uri();
        let client = GelbooruClient::new(&cfg).unwrap();
        let filters = FilterRules {
            min_width: Some(4000),
            ..FilterRules::default()
        };
        let params = SearchParams {
            tags: "tag".into(),
            limit: 10,
            filters,
            ..SearchParams::new("tag".into())
        };
        let posts = search_all(&client, &cfg, &params).await.unwrap();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].id, 101);
    }

    #[tokio::test]
    async fn repeated_pages_do_not_loop_forever() {
        let server = MockServer::start().await;
        let first: Vec<_> = (0..100).map(|i| post(i + 1, 100)).collect();
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body(&first)))
            .mount(&server)
            .await;

        let mut cfg = Config::default();
        cfg.api.base_url = server.uri();
        let client = GelbooruClient::new(&cfg).unwrap();
        let params = SearchParams {
            tags: "tag".into(),
            limit: 0,
            ..SearchParams::new("tag".into())
        };
        let posts = search_all(&client, &cfg, &params).await.unwrap();
        assert_eq!(posts.len(), 100);
    }

    #[tokio::test]
    async fn explicit_all_rating_disables_the_rating_filter() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .and(query_param("tags", "tag"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body(&[post(1, 100)])))
            .mount(&server)
            .await;

        let mut cfg = Config::default();
        cfg.api.base_url = server.uri();
        let client = GelbooruClient::new(&cfg).unwrap();
        let params = SearchParams {
            rating: Some("all".into()),
            ..SearchParams::new("tag".into())
        };
        let posts = search_all(&client, &cfg, &params).await.unwrap();
        assert_eq!(posts.len(), 1);
    }

    #[tokio::test]
    async fn explicit_rating_replaces_a_query_rating_tag() {
        let server = MockServer::start().await;
        let mut explicit = post(1, 100);
        explicit["rating"] = "explicit".into();
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .and(query_param("tags", "tag rating:explicit"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body(&[explicit])))
            .mount(&server)
            .await;

        let mut cfg = Config::default();
        cfg.api.base_url = server.uri();
        let client = GelbooruClient::new(&cfg).unwrap();
        let params = SearchParams {
            tags: "tag rating:safe".into(),
            rating: Some("explicit".into()),
            ..SearchParams::new("tag rating:safe".into())
        };
        let posts = search_all(&client, &cfg, &params).await.unwrap();
        assert_eq!(posts.len(), 1, "{posts:?}");
    }

    #[tokio::test]
    async fn configured_default_rating_is_sent() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .and(query_param("tags", "tag rating:general"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body(&[post(1, 100)])))
            .mount(&server)
            .await;

        let mut cfg = Config::default();
        cfg.api.base_url = server.uri();
        let client = GelbooruClient::new(&cfg).unwrap();
        let posts = search_all(&client, &cfg, &SearchParams::new("tag".into()))
            .await
            .unwrap();
        assert_eq!(posts.len(), 1);
    }

    #[tokio::test]
    async fn query_rating_tag_overrides_the_safe_default() {
        let server = MockServer::start().await;
        let explicit = serde_json::json!({
            "id": 7,
            "file_url": "http://x/7.jpg",
            "rating": "explicit",
            "image_width": 100,
            "image_height": 100,
            "tags": "tag",
            "score": 0,
        });
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .and(query_param("tags", "rating:explicit"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body(&[explicit])))
            .mount(&server)
            .await;

        let mut cfg = Config::default();
        cfg.api.base_url = server.uri();
        let client = GelbooruClient::new(&cfg).unwrap();
        let params = SearchParams {
            tags: "rating:explicit".into(),
            ..SearchParams::new("rating:explicit".into())
        };
        let posts = search_all(&client, &cfg, &params).await.unwrap();
        assert_eq!(posts.len(), 1, "{posts:?}");
        assert_eq!(posts[0].rating, "explicit");
    }

    #[tokio::test]
    async fn api_errors_are_not_swallowed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .respond_with(ResponseTemplate::new(400))
            .mount(&server)
            .await;
        let mut cfg = Config::default();
        cfg.api.base_url = server.uri();
        cfg.downloads.max_retries = 0;
        let client = GelbooruClient::new(&cfg).unwrap();
        let params = SearchParams::new("tag".into());
        let res = search_all(&client, &cfg, &params).await;
        assert!(matches!(res, Err(AppError::Http(_))), "{res:?}");
    }
}
