use gelbooru_downloader::api::client::GelbooruClient;
use gelbooru_downloader::config::Config;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn search_parses() {
    let server = MockServer::start().await;
    let body = r#"[{"id":123,"file_url":"http://x/img.jpg","rating":"safe","image_width":10,"image_height":20,"tags":"a b","score":5}]"#;
    Mock::given(method("GET"))
        .and(path("/index.php"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let mut cfg = Config::default();
    cfg.api.base_url = server.uri();
    let client = GelbooruClient::new(&cfg).unwrap();
    let posts = client.search("landscape", 10, 1, None, None).await.unwrap();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].id, 123);
}

#[tokio::test]
async fn get_post_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/index.php"))
        .respond_with(ResponseTemplate::new(200).set_body_string("[]"))
        .mount(&server)
        .await;
    let mut cfg = Config::default();
    cfg.api.base_url = server.uri();
    let client = GelbooruClient::new(&cfg).unwrap();
    let res = client.get_post(999).await;
    assert!(res.is_err());
}

#[tokio::test]
async fn retry_on_500() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/index.php"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1..)
        .mount(&server)
        .await;
    let mut cfg = Config::default();
    cfg.api.base_url = server.uri();
    cfg.downloads.max_retries = 1;
    let client = GelbooruClient::new(&cfg).unwrap();
    let res = client.search("x", 1, 1, None, None).await;
    assert!(res.is_err());
}
