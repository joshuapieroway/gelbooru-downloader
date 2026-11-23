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

#[derive(Clone)]
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

/// One conjunctive branch of a boolean search query.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryBranch {
    pub required: Vec<String>,
    pub excluded: Vec<String>,
}

impl QueryBranch {
    /// Tags string understood by both Gelbooru and Rule34. A leading `-`
    /// excludes a tag on both boards.
    pub fn to_server_tags(&self) -> String {
        let mut parts = self.required.clone();
        parts.extend(self.excluded.iter().map(|tag| format!("-{tag}")));
        parts.join(" ")
    }

    /// True when the branch only excludes tags and has no positive term. Such
    /// a branch can be sent to the server on its own, but cannot be enumerated
    /// as one side of a client-side OR.
    pub fn is_pure_exclusion(&self) -> bool {
        self.required.is_empty() && !self.excluded.is_empty()
    }
}

/// Parsed form of a search query: metatags plus disjunctive normal form.
#[derive(Debug, Clone, Default)]
pub struct QueryPlan {
    pub metatags: Vec<String>,
    pub branches: Vec<QueryBranch>,
}

impl QueryPlan {
    pub fn is_or(&self) -> bool {
        self.branches.len() > 1
    }

    /// Server tag string for a single conjunction, including metatags. Only
    /// meaningful when `is_or()` is false.
    pub fn single_branch_tags(&self) -> String {
        let mut parts = Vec::new();
        if let Some(branch) = self.branches.first() {
            let tags = branch.to_server_tags();
            if !tags.is_empty() {
                parts.push(tags);
            }
        }
        parts.extend(self.metatags.iter().cloned());
        parts.join(" ")
    }
}

/// Normalize whitespace without changing any operators.
fn normalize_tags(q: &str) -> String {
    q.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse a query into branches that can be sent to Gelbooru/Rule34.
///
/// Both boards understand whitespace (AND) and a leading `-` (NOT), but
/// neither understands `|`. OR queries therefore become several branches that
/// are fetched separately and merged client-side.
pub fn compile_query(q: &str) -> QueryPlan {
    let (metatags, rest) = split_metatags(q);
    if rest.trim().is_empty() {
        return QueryPlan {
            metatags,
            branches: Vec::new(),
        };
    }

    if let Some(expr) = parse_query_expr(&rest) {
        let branches = to_dnf(&expr)
            .into_iter()
            .map(|(required, excluded)| QueryBranch { required, excluded })
            .collect();
        return QueryPlan { metatags, branches };
    }

    // Be forgiving of a stray operator (`cat |`, `| cat`, unbalanced
    // brackets): keep the tags we can understand instead of sending `|`
    // literally to the board.
    QueryPlan {
        metatags,
        branches: vec![fallback_branch(&rest)],
    }
}

fn fallback_branch(q: &str) -> QueryBranch {
    let mut branch = QueryBranch::default();
    let mut negate_next = false;
    for tok in tokenize_expr(q) {
        match tok {
            Tok::Not => negate_next = true,
            Tok::Tag(tag) if tag != "-" => {
                if negate_next {
                    branch.excluded.push(tag);
                } else {
                    branch.required.push(tag);
                }
                negate_next = false;
            }
            _ => {}
        }
    }
    branch
}

/// True when a query needs the client-side OR path.
pub fn query_is_complex(q: &str) -> bool {
    compile_query(q).is_or()
}

/// Compile a single-conjunction query to the tag syntax both boards accept.
/// OR queries cannot be represented in one request; callers should use
/// [`query_is_complex`] first.
pub fn translate_operators(q: &str) -> String {
    let plan = compile_query(q);
    if plan.is_or() {
        normalize_tags(q)
    } else {
        plan.single_branch_tags()
    }
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
    search_expr(client, cfg, params).await
}

pub async fn search_single(
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

    let mut tags = translate_operators(&params.tags);
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

    fn rule34_config(server: &MockServer) -> Config {
        let mut cfg = Config::default();
        cfg.rule34.base_url = server.uri();
        cfg.rule34.user_id = "42".to_string();
        cfg.rule34.api_key = "secret".to_string();
        cfg
    }

    fn ids(posts: &[Post]) -> Vec<i64> {
        posts.iter().map(|p| p.id).collect()
    }

    #[test]
    fn compiles_plain_and_exclusion_queries() {
        let plan = compile_query("cat dog");
        assert_eq!(plan.branches.len(), 1);
        assert_eq!(plan.branches[0].required, vec!["cat", "dog"]);
        assert!(plan.branches[0].excluded.is_empty());
        assert_eq!(plan.single_branch_tags(), "cat dog");
        assert!(!plan.is_or());

        let plan = compile_query("cat -dog");
        assert_eq!(plan.single_branch_tags(), "cat -dog");
        assert!(!plan.is_or());

        let plan = compile_query("- dog");
        assert_eq!(plan.single_branch_tags(), "-dog");
        assert!(!plan.is_or());
    }

    #[test]
    fn compiles_lone_exclusion_without_dropping_it() {
        let plan = compile_query("-dog");
        assert_eq!(plan.branches.len(), 1);
        assert!(plan.branches[0].required.is_empty());
        assert_eq!(plan.branches[0].excluded, vec!["dog"]);
        assert_eq!(plan.single_branch_tags(), "-dog");
        assert!(!query_is_complex("-dog"));
    }

    #[test]
    fn compiles_or_queries_into_branches() {
        let plan = compile_query("cat -dog | bird");
        assert_eq!(plan.branches.len(), 2);
        assert_eq!(plan.branches[0].to_server_tags(), "cat -dog");
        assert_eq!(plan.branches[1].to_server_tags(), "bird");
        assert!(plan.is_or());
        assert!(query_is_complex("cat -dog | bird"));
        assert!(!query_is_complex("cat -dog"));
    }

    #[test]
    fn compiles_grouped_parentheses_without_breaking_tag_parentheses() {
        let plan = compile_query("(cat | dog) bird");
        assert_eq!(plan.branches.len(), 2);
        assert_eq!(plan.branches[0].to_server_tags(), "cat bird");
        assert_eq!(plan.branches[1].to_server_tags(), "dog bird");

        let plan = compile_query("(rem_(re:zero) | cat) -dog");
        assert_eq!(plan.branches.len(), 2);
        assert_eq!(plan.branches[0].to_server_tags(), "rem_(re:zero) -dog");
        assert_eq!(plan.branches[1].to_server_tags(), "cat -dog");

        let plan = compile_query("rem_(re:zero)");
        assert_eq!(plan.single_branch_tags(), "rem_(re:zero)");
    }

    #[test]
    fn compiles_negated_groups_with_de_morgan() {
        let plan = compile_query("-[cat | dog]");
        assert_eq!(plan.branches.len(), 1);
        assert!(plan.branches[0].required.is_empty());
        assert_eq!(plan.branches[0].excluded, vec!["cat", "dog"]);
        assert_eq!(plan.single_branch_tags(), "-cat -dog");
    }

    #[test]
    fn keeps_metatags_separate_from_branches() {
        let plan = compile_query("sort:score cat | dog");
        assert_eq!(plan.metatags, vec!["sort:score"]);
        assert_eq!(plan.branches.len(), 2);
        assert_eq!(plan.branches[0].to_server_tags(), "cat");
        assert_eq!(plan.branches[1].to_server_tags(), "dog");
    }

    #[test]
    fn forgives_stray_or_operators() {
        assert_eq!(compile_query("cat |").single_branch_tags(), "cat");
        assert_eq!(compile_query("| cat").single_branch_tags(), "cat");
        assert_eq!(compile_query("cat & dog").single_branch_tags(), "cat dog");
        assert_eq!(translate_operators("  cat   -dog "), "cat -dog");
    }

    #[tokio::test]
    async fn exclusion_is_sent_to_the_server() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .and(query_param("tags", "cat -dog"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body(&[post(1, 100)])))
            .mount(&server)
            .await;

        let mut cfg = Config::default();
        cfg.api.base_url = server.uri();
        let client = GelbooruClient::new(&cfg).unwrap();
        let params = SearchParams {
            rating: Some("all".into()),
            ..SearchParams::new("cat -dog".into())
        };
        let posts = search_all(&client, &cfg, &params).await.unwrap();
        assert_eq!(ids(&posts), vec![1]);
    }

    #[tokio::test]
    async fn lone_exclusion_is_sent_to_the_server() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .and(query_param("tags", "-dog"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body(&[post(5, 100)])))
            .mount(&server)
            .await;

        let mut cfg = Config::default();
        cfg.api.base_url = server.uri();
        let client = GelbooruClient::new(&cfg).unwrap();
        let params = SearchParams {
            rating: Some("all".into()),
            ..SearchParams::new("-dog".into())
        };
        let posts = search_all(&client, &cfg, &params).await.unwrap();
        assert_eq!(ids(&posts), vec![5]);
    }

    #[tokio::test]
    async fn or_results_are_interleaved_and_deduplicated() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .and(query_param("tags", "cat"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(body(&[post(1, 100), post(2, 100)])),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .and(query_param("tags", "dog"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(body(&[post(2, 100), post(3, 100)])),
            )
            .mount(&server)
            .await;

        let mut cfg = Config::default();
        cfg.api.base_url = server.uri();
        let client = GelbooruClient::new(&cfg).unwrap();
        let params = SearchParams {
            rating: Some("all".into()),
            limit: 10,
            ..SearchParams::new("cat | dog".into())
        };
        let posts = search_all(&client, &cfg, &params).await.unwrap();
        assert_eq!(ids(&posts), vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn or_windows_paginate_without_overlap() {
        let server = MockServer::start().await;
        let cat: Vec<_> = (1..=10).map(|i| post(i, 100)).collect();
        let dog: Vec<_> = (11..=20).map(|i| post(i, 100)).collect();
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .and(query_param("tags", "cat"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body(&cat)))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .and(query_param("tags", "dog"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body(&dog)))
            .mount(&server)
            .await;

        let mut cfg = Config::default();
        cfg.api.base_url = server.uri();
        let client = GelbooruClient::new(&cfg).unwrap();
        let params = SearchParams {
            rating: Some("all".into()),
            limit: 0,
            ..SearchParams::new("cat | dog".into())
        };

        let (first, more) = search_expr_window(&client, &cfg, &params, 0, 5)
            .await
            .unwrap();
        assert_eq!(ids(&first), vec![1, 11, 2, 12, 3]);
        assert!(more);

        let (second, more) = search_expr_window(&client, &cfg, &params, 5, 5)
            .await
            .unwrap();
        assert_eq!(ids(&second), vec![13, 4, 14, 5, 15]);
        assert!(more);
        assert!(first.iter().all(|p| !second.iter().any(|q| q.id == p.id)));

        let (last, more) = search_expr_window(&client, &cfg, &params, 15, 5)
            .await
            .unwrap();
        assert_eq!(last.len(), 5);
        assert!(!more);
    }

    #[tokio::test]
    async fn metatags_are_sent_to_every_or_branch() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .and(query_param("tags", "cat sort:score"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body(&[post(1, 100)])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .and(query_param("tags", "dog sort:score"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body(&[post(2, 100)])))
            .mount(&server)
            .await;

        let mut cfg = Config::default();
        cfg.api.base_url = server.uri();
        let client = GelbooruClient::new(&cfg).unwrap();
        let params = SearchParams {
            rating: Some("all".into()),
            limit: 10,
            ..SearchParams::new("sort:score cat | dog".into())
        };
        let posts = search_all(&client, &cfg, &params).await.unwrap();
        assert_eq!(ids(&posts), vec![1, 2]);
    }

    #[tokio::test]
    async fn rule34_uses_native_exclusions_and_or_branches() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .and(query_param("tags", "cat -dog"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body(&[post(1, 100)])))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .and(query_param("tags", "bird"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body(&[post(2, 100)])))
            .mount(&server)
            .await;

        let cfg = rule34_config(&server);
        let client = GelbooruClient::new_for(&cfg, crate::api::client::Source::Rule34).unwrap();
        let params = SearchParams {
            rating: Some("all".into()),
            limit: 10,
            ..SearchParams::new("cat -dog | bird".into())
        };
        let posts = search_all(&client, &cfg, &params).await.unwrap();
        assert_eq!(ids(&posts), vec![1, 2]);
    }

    #[tokio::test]
    async fn rule34_lone_exclusion_is_sent_to_the_server() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/index.php"))
            .and(query_param("tags", "-dog"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body(&[post(9, 100)])))
            .mount(&server)
            .await;

        let cfg = rule34_config(&server);
        let client = GelbooruClient::new_for(&cfg, crate::api::client::Source::Rule34).unwrap();
        let params = SearchParams {
            rating: Some("all".into()),
            ..SearchParams::new("-dog".into())
        };
        let posts = search_all(&client, &cfg, &params).await.unwrap();
        assert_eq!(ids(&posts), vec![9]);
    }

    #[tokio::test]
    async fn or_with_only_exclusions_is_rejected() {
        let cfg = Config::default();
        let client = GelbooruClient::new(&cfg).unwrap();
        let params = SearchParams::new("-cat | -dog".into());
        let err = search_all(&client, &cfg, &params).await.unwrap_err();
        assert!(err.to_string().contains("positive tag"), "{err}");
    }
}

// ---------------------------------------------------------------------------
// Boolean expression search (brackets, &, |, -)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Expr {
    Tag(String),
    Not(Box<Expr>),
    And(Vec<Expr>),
    Or(Vec<Expr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Open,
    Close,
    And,
    Or,
    Not,
    Tag(String),
}

fn tokenize_expr(s: &str) -> Vec<Tok> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut tokens = Vec::new();
    let mut group_depth = 0usize;
    let mut i = 0;
    while i < n {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        // Square brackets are always grouping brackets.
        if chars[i] == '[' {
            tokens.push(Tok::Open);
            group_depth += 1;
            i += 1;
            continue;
        }
        if chars[i] == ']' {
            tokens.push(Tok::Close);
            group_depth = group_depth.saturating_sub(1);
            i += 1;
            continue;
        }
        // Parentheses group when they start a token. This accepts both
        // `(a | b)` and `( a | b )` while keeping tags such as
        // `rem_(re:zero)` in one piece.
        if chars[i] == '(' && at_token_start(&chars, i) {
            tokens.push(Tok::Open);
            group_depth += 1;
            i += 1;
            continue;
        }
        if chars[i] == ')' {
            if group_depth > 0 {
                tokens.push(Tok::Close);
                group_depth -= 1;
            }
            i += 1;
            continue;
        }
        if chars[i] == '&' {
            tokens.push(Tok::And);
            i += 1;
            continue;
        }
        if chars[i] == '|' {
            tokens.push(Tok::Or);
            i += 1;
            continue;
        }
        // '-' is NOT at a token boundary. Accept both `-tag` and `- tag`.
        if chars[i] == '-' && at_operator_boundary(&chars, i) {
            tokens.push(Tok::Not);
            i += 1;
            continue;
        }

        // Read a tag. Parentheses inside a tag belong to the tag, so
        // `rem_(re:zero)` survives while `(a | b)` still groups.
        let mut tag = String::new();
        let mut tag_parens = 0usize;
        while i < n {
            let c = chars[i];
            if c.is_whitespace() || c == '[' || c == ']' || c == '&' || c == '|' {
                break;
            }
            if c == '(' {
                tag_parens += 1;
                tag.push(c);
                i += 1;
                continue;
            }
            if c == ')' {
                if tag_parens > 0 {
                    tag_parens -= 1;
                    tag.push(c);
                    i += 1;
                    continue;
                }
                // A closing parenthesis with no tag-local opener ends a
                // group, but only at a token boundary. Otherwise it is just
                // part of the tag.
                if group_depth > 0 && (i + 1 == n || is_token_separator(chars[i + 1])) {
                    break;
                }
                tag.push(c);
                i += 1;
                continue;
            }
            tag.push(c);
            i += 1;
        }
        if !tag.is_empty() {
            tokens.push(Tok::Tag(tag));
        }
    }
    tokens
}

/// True when `(` at `i` opens a group rather than being part of a tag.
fn at_token_start(chars: &[char], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let prev = chars[i - 1];
    if prev.is_whitespace() || matches!(prev, '[' | ']' | '&' | '|' | '(') {
        return true;
    }
    // `-(...)` starts a negated group, but `foo-(bar)` is a tag.
    prev == '-' && at_operator_boundary(chars, i - 1)
}

fn at_operator_boundary(chars: &[char], i: usize) -> bool {
    i == 0
        || chars[i - 1].is_whitespace()
        || matches!(chars[i - 1], '[' | ']' | '&' | '|' | '(' | ')')
}

fn is_token_separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '[' | ']' | '&' | '|' | ')' | '-')
}

fn parse_expr_tokens(tokens: &[Tok]) -> Option<(Expr, &[Tok])> {
    parse_or(tokens)
}

fn parse_or(tokens: &[Tok]) -> Option<(Expr, &[Tok])> {
    let (left, mut rest) = parse_and(tokens)?;
    let mut ors = vec![left];
    while matches!(rest.first(), Some(Tok::Or)) {
        rest = &rest[1..];
        let (right, r) = parse_and(rest)?;
        ors.push(right);
        rest = r;
    }
    if ors.len() == 1 {
        Some((ors.pop().unwrap(), rest))
    } else {
        Some((Expr::Or(ors), rest))
    }
}

fn parse_and(tokens: &[Tok]) -> Option<(Expr, &[Tok])> {
    let (first, mut rest) = parse_not(tokens)?;
    let mut ands = vec![first];
    loop {
        if rest.is_empty() || matches!(rest.first(), Some(Tok::Or) | Some(Tok::Close)) {
            break;
        }
        if matches!(rest.first(), Some(Tok::And)) {
            rest = &rest[1..];
        }
        // implicit AND only if next is a primary (not/primary)
        if matches!(rest.first(), Some(Tok::Or) | Some(Tok::Close) | None) {
            break;
        }
        let (right, r) = parse_not(rest)?;
        ands.push(right);
        rest = r;
    }
    if ands.len() == 1 {
        Some((ands.pop().unwrap(), rest))
    } else {
        Some((Expr::And(ands), rest))
    }
}

fn parse_not(tokens: &[Tok]) -> Option<(Expr, &[Tok])> {
    if matches!(tokens.first(), Some(Tok::Not)) {
        let (inner, rest) = parse_primary(&tokens[1..])?;
        return Some((Expr::Not(Box::new(inner)), rest));
    }
    parse_primary(tokens)
}

fn parse_primary(tokens: &[Tok]) -> Option<(Expr, &[Tok])> {
    match tokens.first()? {
        Tok::Open => {
            let (expr, rest) = parse_or(&tokens[1..])?;
            match rest.first()? {
                Tok::Close => Some((expr, &rest[1..])),
                _ => None,
            }
        }
        Tok::Tag(t) => Some((Expr::Tag(t.clone()), &tokens[1..])),
        _ => None,
    }
}

pub fn parse_query_expr(s: &str) -> Option<Expr> {
    let toks = tokenize_expr(s);
    let (expr, rest) = parse_expr_tokens(&toks)?;
    if !rest.is_empty() {
        return None;
    }
    Some(expr)
}

/// Convert an expression to DNF: a list of (required_tags, excluded_tags).
fn to_dnf(expr: &Expr) -> Vec<(Vec<String>, Vec<String>)> {
    match expr {
        Expr::Tag(t) => vec![(vec![t.clone()], vec![])],
        Expr::Not(inner) => match inner.as_ref() {
            Expr::Tag(t) => vec![(vec![], vec![t.clone()])],
            // Push negation inwards so grouped exclusions such as
            // `-[cat | dog]` and `-(cat dog)` still become valid DNF.
            Expr::Not(e) => to_dnf(e),
            Expr::And(exprs) => {
                let negated = Expr::Or(
                    exprs
                        .iter()
                        .map(|e| Expr::Not(Box::new(e.clone())))
                        .collect(),
                );
                to_dnf(&negated)
            }
            Expr::Or(exprs) => {
                let negated = Expr::And(
                    exprs
                        .iter()
                        .map(|e| Expr::Not(Box::new(e.clone())))
                        .collect(),
                );
                to_dnf(&negated)
            }
        },
        Expr::And(exprs) => {
            let mut result = vec![(vec![], vec![])];
            for e in exprs {
                let dnf = to_dnf(e);
                let mut new_result = Vec::new();
                for (r1, e1) in &result {
                    for (r2, e2) in &dnf {
                        let mut r = r1.clone();
                        r.extend(r2.clone());
                        let mut ex = e1.clone();
                        ex.extend(e2.clone());
                        new_result.push((r, ex));
                    }
                }
                result = new_result;
            }
            result
        }
        Expr::Or(exprs) => {
            let mut result = Vec::new();
            for e in exprs {
                result.extend(to_dnf(e));
            }
            result
        }
    }
}

fn is_metatag(name: &str) -> bool {
    const META: &[&str] = &[
        "user",
        "approver",
        "commenter",
        "comm",
        "noter",
        "noteupdater",
        "artcomm",
        "commentaryupdater",
        "flagger",
        "appealer",
        "upvote",
        "downvote",
        "fav",
        "ordfav",
        "favgroup",
        "ordfavgroup",
        "pool",
        "ordpool",
        "note",
        "comment",
        "commentary",
        "id",
        "rating",
        "locked",
        "source",
        "status",
        "filetype",
        "disapproved",
        "parent",
        "child",
        "search",
        "embedded",
        "md5",
        "width",
        "height",
        "mpixels",
        "ratio",
        "score",
        "favcount",
        "filesize",
        "date",
        "age",
        "order",
        "limit",
        "tagcount",
        "pixiv_id",
        "pixiv",
        "unaliased",
        "comment_count",
        "deleted_comment_count",
        "active_comment_count",
        "note_count",
        "deleted_note_count",
        "active_note_count",
        "flag_count",
        "child_count",
        "deleted_child_count",
        "active_child_count",
        "pool_count",
        "deleted_pool_count",
        "active_pool_count",
        "series_pool_count",
        "collection_pool_count",
        "appeal_count",
        "approval_count",
        "replacement_count",
        "comments",
        "deleted_comments",
        "active_comments",
        "notes",
        "deleted_notes",
        "active_notes",
        "flags",
        "children",
        "deleted_children",
        "active_children",
        "pools",
        "deleted_pools",
        "active_pools",
        "series_pools",
        "collection_pools",
        "appeals",
        "approvals",
        "replacements",
        "gentags",
        "chartags",
        "copytags",
        "arttags",
        "metatags",
        "sort",
    ];
    META.contains(&name.to_ascii_lowercase().as_str())
}

fn split_metatags(q: &str) -> (Vec<String>, String) {
    let mut meta = Vec::new();
    let mut rest = Vec::new();
    for tok in q.split_whitespace() {
        if let Some((prefix, _)) = tok.split_once(':') {
            if is_metatag(prefix) {
                meta.push(tok.to_string());
                continue;
            }
        }
        rest.push(tok.to_string());
    }
    (meta, rest.join(" "))
}

pub async fn search_expr(
    client: &GelbooruClient,
    cfg: &Config,
    params: &SearchParams,
) -> Result<Vec<Post>> {
    let (posts, _) = search_expr_window(client, cfg, params, 0, params.limit as usize).await?;
    Ok(posts)
}

/// Execute a query and return one window of results plus whether another
/// window may exist.
///
/// `count == 0` means "everything left". OR queries are fetched as separate
/// conjunctions and interleaved, so paging through a union shows results from
/// every branch instead of draining the first one.
pub async fn search_expr_window(
    client: &GelbooruClient,
    cfg: &Config,
    params: &SearchParams,
    offset: usize,
    count: usize,
) -> Result<(Vec<Post>, bool)> {
    let plan = compile_query(&params.tags);

    if !plan.is_or() {
        let mut p = params.clone();
        p.tags = plan.single_branch_tags();
        let fetch_limit = if count == 0 {
            params.limit
        } else {
            offset.saturating_add(count).min(u32::MAX as usize) as u32
        };
        p.limit = fetch_limit;
        let posts = search_single(client, cfg, &p).await?;
        let total = posts.len();
        let start = offset.min(total);
        let end = if count == 0 {
            total
        } else {
            offset.saturating_add(count).min(total)
        };
        let has_more =
            end < total || (count > 0 && fetch_limit > 0 && posts.len() >= fetch_limit as usize);
        return Ok((posts[start..end].to_vec(), has_more));
    }

    search_or(client, cfg, params, &plan, offset, count).await
}

async fn search_or(
    client: &GelbooruClient,
    cfg: &Config,
    params: &SearchParams,
    plan: &QueryPlan,
    offset: usize,
    count: usize,
) -> Result<(Vec<Post>, bool)> {
    // A branch that only excludes tags matches almost the whole board, so it
    // cannot be one side of a useful union. A lone exclusion is still handled
    // natively above; only ORs with a positive term are enumerated.
    let branches: Vec<&QueryBranch> = plan
        .branches
        .iter()
        .filter(|branch| !branch.is_pure_exclusion())
        .collect();
    if branches.is_empty() {
        return Err(crate::utils::error::AppError::Api(
            "An OR query needs at least one positive tag; '-a | -b' cannot be enumerated"
                .to_string(),
        ));
    }

    let fetch_limit = if count == 0 {
        params.limit
    } else {
        offset.saturating_add(count).min(u32::MAX as usize) as u32
    };

    // Fetch each conjunction in parallel. Metatags (`sort:`, `rating:`, ...)
    // apply to every branch.
    let mut handles = Vec::new();
    for branch in branches {
        let mut branch_params = params.clone();
        let mut tags = branch.to_server_tags();
        for metatag in &plan.metatags {
            if !tags.is_empty() {
                tags.push(' ');
            }
            tags.push_str(metatag);
        }
        branch_params.tags = tags;
        branch_params.limit = fetch_limit;
        let client = client.clone();
        let cfg = cfg.clone();
        handles.push(tokio::spawn(async move {
            search_single(&client, &cfg, &branch_params).await
        }));
    }

    let mut per_branch: Vec<Vec<Post>> = Vec::with_capacity(handles.len());
    for handle in handles {
        let posts = handle
            .await
            .map_err(|e| crate::utils::error::AppError::Http(e.to_string()))??;
        per_branch.push(posts);
    }

    // Round-robin interleaving keeps every branch visible in the first page.
    // Deduplicating by post id makes posts that match several branches appear
    // once.
    let mut merged = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();
    let mut index = 0usize;
    loop {
        let mut any = false;
        for posts in &per_branch {
            if let Some(post) = posts.get(index) {
                any = true;
                if post.id == 0 || seen.insert(post.id) {
                    merged.push(post.clone());
                }
            }
        }
        if !any {
            break;
        }
        index += 1;
    }

    sort_posts(&mut merged, params, &plan.metatags);

    let total = merged.len();
    let start = offset.min(total);
    let end = if count == 0 {
        total
    } else {
        offset.saturating_add(count).min(total)
    };
    let branch_may_have_more = count > 0
        && fetch_limit > 0
        && per_branch
            .iter()
            .any(|posts| posts.len() >= fetch_limit as usize);
    let has_more = end < total || branch_may_have_more;
    Ok((merged[start..end].to_vec(), has_more))
}

/// Apply an explicit `sort`/`order` or a `sort:` metatag to merged OR results.
fn sort_posts(posts: &mut [Post], params: &SearchParams, metatags: &[String]) {
    let meta_sort = metatags
        .iter()
        .find_map(|m| m.strip_prefix("sort:").map(|s| s.to_string()));
    let Some(sort) = params.sort.clone().or(meta_sort) else {
        return;
    };
    let desc = params
        .order
        .as_deref()
        .map(|o| o.eq_ignore_ascii_case("desc"))
        .unwrap_or(true);
    let sort = sort.to_ascii_lowercase();
    posts.sort_by(|a, b| {
        let cmp = match sort.as_str() {
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
