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
    let post = Post {
        id: 7,
        file_url: format!("{}/img.png", server.uri()),
        md5: "abc".to_string(),
        ..Post::default()
    };
    let out = engine.download_post(post, None).await;
    assert!(out.error.is_none(), "{:?}", out.error);
    assert!(out.path.exists());
    assert!(!out.skipped);
    // no .part leftover
    assert!(!out.path.with_extension("part").exists());
    // A successful post must not stay in the persistent queue.
    assert!(db.queue_list(-1).await.unwrap().is_empty());
}

#[tokio::test]
async fn interrupted_downloads_resume_from_the_queue() {
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

    let post = Post {
        id: 42,
        md5: "abc".to_string(),
        file_url: format!("{}/img.png", server.uri()),
        file_size: bytes.len() as i64,
        ..Post::default()
    };
    let id = db
        .enqueue_queue(
            post,
            dir.path().to_string_lossy().to_string(),
            "{id}_{md5}.{ext}".to_string(),
            String::new(),
            None,
            None,
        )
        .await
        .unwrap();
    // A killed process leaves the row as `downloading`.
    db.queue_set_downloading(id).await.unwrap();

    let report = gelbooru_downloader::queue::resume(&db, &cfg).await.unwrap();
    assert_eq!(report.resumed, 1);
    assert_eq!(report.completed, 1);
    assert_eq!(report.failed, 0);
    assert!(dir.path().join("42_abc.png").exists());
    assert!(db.queue_list(-1).await.unwrap().is_empty());
}

#[tokio::test]
async fn failed_download_remains_in_the_persistent_queue() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"not an image".to_vec()))
        .mount(&server)
        .await;
    let mut cfg = Config::default();
    cfg.api.base_url = server.uri();
    cfg.downloads.validate_files = true;
    cfg.downloads.resume = false;
    let dir = tempdir().unwrap();
    cfg.downloads.directory = dir.path().to_string_lossy().to_string();
    let db = Arc::new(HistoryDb::open_in_memory().unwrap());
    let mut engine = DownloadEngine::new(cfg, Some(db.clone()));
    engine.filename_template = Some("{id}.{ext}".to_string());
    engine.folder_template = Some("{rating}".to_string());

    let post = Post {
        id: 11,
        rating: "safe".to_string(),
        file_url: format!("{}/img.png", server.uri()),
        md5: "abc".to_string(),
        ..Post::default()
    };
    let out = engine.download_post(post, None).await;
    assert!(out.error.is_some());

    let failed = db.queue_failed().await.unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].post_id, 11);
    assert_eq!(failed[0].filename_template, "{id}.{ext}");
    assert_eq!(failed[0].folder_template, "{rating}");
    assert!(failed[0].error.is_some());

    // Retrying requeues the row instead of dropping it.
    assert_eq!(db.queue_retry_failed().await.unwrap(), 1);
    assert!(db.queue_failed().await.unwrap().is_empty());
    assert_eq!(db.queue_pending().await.unwrap().len(), 1);
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
        posts.push(Post {
            id: 100 + i,
            md5: format!("hash{i}"),
            file_url: format!("{}/img{i}.png", server.uri()),
            file_size: bytes.len() as i64,
            ..Post::default()
        });
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
async fn download_many_deduplicates_repeated_posts() {
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
    let dir = tempdir().unwrap();
    cfg.downloads.directory = dir.path().to_string_lossy().to_string();
    let db = Arc::new(HistoryDb::open_in_memory().unwrap());
    let engine = DownloadEngine::new(cfg, Some(db));

    let post = Post {
        id: 11,
        md5: "abc".to_string(),
        file_url: format!("{}/img.png", server.uri()),
        ..Post::default()
    };

    let out = engine
        .download_many(vec![post.clone(), post], None, None)
        .await;
    assert_eq!(out.len(), 1, "{out:?}");
    assert!(out[0].error.is_none(), "{out:?}");
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "duplicate post should only be fetched once"
    );
}

#[tokio::test]
async fn hash_duplicate_keeps_the_new_file_when_the_old_copy_is_gone() {
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
    let dir = tempdir().unwrap();
    cfg.downloads.directory = dir.path().to_string_lossy().to_string();
    let db = Arc::new(HistoryDb::open_in_memory().unwrap());

    let hash = blake3::hash(&bytes).to_hex().to_string();
    db.insert(gelbooru_downloader::database::history::DownloadRecord {
        id: None,
        post_id: 1,
        url: "old".into(),
        local_path: dir.path().join("missing.png").to_string_lossy().to_string(),
        file_size: bytes.len() as i64,
        file_hash: hash,
        downloaded_at: gelbooru_downloader::database::history::now(),
        tags: String::new(),
        rating: String::new(),
        status: "completed".into(),
    })
    .await
    .unwrap();

    let engine = DownloadEngine::new(cfg, Some(db));
    let post = Post {
        id: 2,
        md5: "abc".to_string(),
        file_url: format!("{}/img.png", server.uri()),
        ..Post::default()
    };
    let out = engine.download_post(post, None).await;
    assert!(out.error.is_none(), "{out:?}");
    assert!(!out.skipped, "missing copy must not count as a duplicate");
    assert!(out.path.exists(), "the newly downloaded file was deleted");
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
    let post = Post {
        id: 9,
        md5: "".to_string(),
        file_url: format!("{}/img.png", server.uri()),
        ..Post::default()
    };
    let out = engine.download_post(post, None).await;
    assert!(out.skipped);
}
