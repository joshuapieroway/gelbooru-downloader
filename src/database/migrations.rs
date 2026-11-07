pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS downloads (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    post_id INTEGER NOT NULL,
    url TEXT NOT NULL,
    local_path TEXT NOT NULL,
    file_size INTEGER NOT NULL DEFAULT 0,
    file_hash TEXT NOT NULL DEFAULT '',
    downloaded_at TEXT NOT NULL,
    tags TEXT NOT NULL DEFAULT '',
    rating TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'completed',
    UNIQUE(post_id)
);
CREATE INDEX IF NOT EXISTS idx_downloads_post_id ON downloads(post_id);
CREATE INDEX IF NOT EXISTS idx_downloads_status ON downloads(status);
CREATE TABLE IF NOT EXISTS watch_state (
    tags TEXT PRIMARY KEY,
    last_seen_id INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS presets (
    name TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS download_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    post_id INTEGER NOT NULL,
    post_json TEXT NOT NULL,
    output_dir TEXT NOT NULL,
    filename_template TEXT NOT NULL DEFAULT '',
    folder_template TEXT NOT NULL DEFAULT '',
    sort TEXT,
    "order" TEXT,
    status TEXT NOT NULL DEFAULT 'queued',
    error TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(post_id, output_dir)
);
CREATE INDEX IF NOT EXISTS idx_download_queue_status ON download_queue(status);
"#;
