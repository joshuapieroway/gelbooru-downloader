use gelbooru_downloader::api::models::Post;
use gelbooru_downloader::config::Config;
use gelbooru_downloader::database::history::HistoryDb;
use gelbooru_downloader::downloader::engine::DownloadEngine;
use std::sync::Arc;
use tempfile::tempdir;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn png_bytes() -> Vec<u8> {
    let mut v = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend_from_slice(&[0u8; 64]);
    v
}

#[tokio::test]
async fn downloads_file_atomically() {
    let server = MockServer::start().await;
    let bytes = png_bytes();
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.clone()))
        .mount(&server)
        .await;
    let mut cfg = Config::default();
    cfg.api.base_url = server.uri();
    cfg.downloads.validate_files = true;
    cfg.downloads.resume = false;
    let dir = tempdir().unwrap();
    cfg.downloads.directory = dir.path().to_string_lossy().to_string();
    let db = Arc::new(HistoryDb::open_in_memory().unwrap());
    let engine = DownloadEngine::new(cfg, Some(db.clone()));
    let mut post = Post::default();
    post.id = 7;
    post.file_url = format!("{}/img.png", server.uri());
    post.md5 = "abc".to_string();
    let out = engine.download_post(post, None).await;
    assert!(out.error.is_none(), "{:?}", out.error);
    assert!(out.path.exists());
    assert!(!out.skipped);
    // no .part leftover
    assert!(!out.path.with_extension("part").exists());
}

#[tokio::test]
async fn progress_bar_tracks_downloaded_bytes() {
    let server = MockServer::start().await;
    let bytes = png_bytes();
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.clone()))
        .mount(&server)
        .await;
    let mut cfg = Config::default();
    cfg.api.base_url = server.uri();
    cfg.downloads.validate_files = false;
    cfg.downloads.resume = false;
    cfg.downloads.skip_duplicates = false;
    let dir = tempdir().unwrap();
    cfg.downloads.directory = dir.path().to_string_lossy().to_string();
    let db = Arc::new(HistoryDb::open_in_memory().unwrap());
    let engine = DownloadEngine::new(cfg, Some(db));

    let mut posts = Vec::new();
    for i in 0..2 {
        let mut post = Post::default();
        post.id = 100 + i;
        post.md5 = format!("hash{i}");
        post.file_url = format!("{}/img{i}.png", server.uri());
        post.file_size = bytes.len() as i64;
        posts.push(post);
    }

    // A hidden bar still keeps length/position state, so we can assert on it
    // without writing escape codes into the test output.
    let pb = indicatif::ProgressBar::hidden();
    let out = engine.download_many(posts, None, Some(pb.clone())).await;

    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|o| o.error.is_none()), "{out:?}");
    assert_eq!(pb.length(), Some(bytes.len() as u64 * 2));
    // `finish` pins the position to the full length.
    assert_eq!(pb.position(), bytes.len() as u64 * 2);
    assert_eq!(pb.message(), "2/2 files");
}

#[tokio::test]
async fn duplicate_post_skipped() {
    let server = MockServer::start().await;
    let bytes = png_bytes();
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.clone()))
        .mount(&server)
        .await;
    let mut cfg = Config::default();
    cfg.api.base_url = server.uri();
    cfg.downloads.validate_files = false;
    let dir = tempdir().unwrap();
    cfg.downloads.directory = dir.path().to_string_lossy().to_string();
    let db = Arc::new(HistoryDb::open_in_memory().unwrap());
    // pre-insert
    let rec = gelbooru_downloader::database::history::DownloadRecord {
        id: None,
        post_id: 9,
        url: "x".into(),
        local_path: "y".into(),
        file_size: 1,
        file_hash: "h".into(),
        downloaded_at: gelbooru_downloader::database::history::now(),
        tags: "".into(),
        rating: "".into(),
        status: "completed".into(),
    };
    db.insert(rec).await.unwrap();
    let engine = DownloadEngine::new(cfg, Some(db.clone()));
    // create the expected on-disk file so the duplicate is skipped
    let expected = dir.path().join("9_file.png");
    std::fs::write(&expected, b"x").unwrap();
    let mut post = Post::default();
    post.id = 9;
    post.md5 = "".to_string();
    post.file_url = format!("{}/img.png", server.uri());
    let out = engine.download_post(post, None).await;
    assert!(out.skipped);
}
