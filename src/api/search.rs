use crate::api::client::GelbooruClient;
use crate::api::models::Post;
use crate::config::Config;
use crate::utils::error::Result;

pub struct SearchParams {
    pub tags: String,
    pub rating: Option<String>,
    pub limit: u32,
    pub page_size: u32,
    pub sort: Option<String>,
    pub order: Option<String>,
}

impl SearchParams {
    pub fn new(tags: String) -> Self {
        Self {
            tags,
            rating: None,
            limit: 100,
            page_size: 100,
            sort: None,
            order: None,
        }
    }
}

pub async fn search_all(
    client: &GelbooruClient,
    _cfg: &Config,
    params: &SearchParams,
) -> Result<Vec<Post>> {
    let mut tags = params.tags.clone();
    // Only push a rating tag when a single rating is requested; multiple ratings
    // are filtered client-side after fetching.
    if let Some(r) = &params.rating {
        let ratings: Vec<&str> = r
            .split(',')
            .map(|x| x.trim())
            .filter(|x| !x.is_empty())
            .collect();
        if ratings.len() == 1 {
            let t = rating_tag(ratings[0]);
            if !t.is_empty() {
                tags.push(' ');
                tags.push_str(t);
            }
        }
    }
    let page_size = params.page_size.max(1).min(1000);
    let total_limit = params.limit; // 0 means unlimited
    let mut out = Vec::new();
    let mut page = 1u32;
    loop {
        if total_limit > 0 && out.len() as u32 >= total_limit {
            break;
        }
        let remaining = if total_limit > 0 {
            total_limit - out.len() as u32
        } else {
            page_size
        };
        let req_limit = remaining.min(page_size);
        let (posts, _total) = client
            .search_page(
                &tags,
                req_limit,
                page,
                params.sort.as_deref(),
                params.order.as_deref(),
            )
            .await?;
        if posts.is_empty() {
            break;
        }
        let n = posts.len();
        out.extend(posts);
        if n < req_limit as usize {
            break;
        }
        page += 1;
        // small politeness delay
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
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
