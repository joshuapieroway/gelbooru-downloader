use crate::api::client::GelbooruClient;
use crate::api::search;
use crate::batch;
use crate::config::Config;
use crate::database::history::HistoryDb;
use crate::downloader::engine::DownloadEngine;
use crate::filters::rules::FilterRules;
use crate::presets::Preset;
use crate::presets::PresetManager;
use crate::repl;
use crate::tui;
use crate::utils::paths;
use crate::watch::Watcher;
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(
    name = "gelbooru",
    version,
    about = "High-performance Gelbooru downloader",
    after_help = "Examples:\n  gelbooru search aisha_greyrat --rating explicit\n  gelbooru download norn_greyrat --limit 100 --sort score --order desc\n  gelbooru export rias_gremory csv results.csv --exclude solo_male\n  gelbooru tui\n  gelbooru queue list"
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    #[arg(long, global = true)]
    pub output: Option<PathBuf>,
    #[arg(long, global = true)]
    pub concurrency: Option<usize>,
    #[arg(long, global = true)]
    pub user_id: Option<String>,
    #[arg(long, global = true)]
    pub api_key: Option<String>,
    #[arg(long, global = true)]
    pub verbose: bool,
    #[arg(long, global = true)]
    pub json: bool,
    #[arg(long, global = true, help = "Disable progress bars")]
    pub quiet: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Search {
        tags: String,
        #[arg(long)]
        rating: Option<String>,
        /// Maximum matching posts (defaults to `search.default_limit`).
        #[arg(long)]
        limit: Option<u32>,
        #[arg(long)]
        min_width: Option<u32>,
        #[arg(long)]
        max_width: Option<u32>,
        #[arg(long)]
        min_height: Option<u32>,
        #[arg(long)]
        max_height: Option<u32>,
        #[arg(long)]
        min_size: Option<u64>,
        #[arg(long)]
        max_size: Option<u64>,
        #[arg(long)]
        min_score: Option<i64>,
        #[arg(long)]
        max_score: Option<i64>,
        #[arg(long)]
        ext: Vec<String>,
        /// Tags that must be present on every result.
        #[arg(long)]
        include: Vec<String>,
        /// Tags to exclude from results.
        #[arg(long)]
        exclude: Vec<String>,
        #[arg(long)]
        sort: Option<String>,
        #[arg(long)]
        order: Option<String>,
    },
    Inspect {
        id: i64,
    },
    Export {
        tags: String,
        /// Output format: json or csv.
        format: String,
        /// Destination file path.
        file: PathBuf,
        #[arg(long)]
        rating: Option<String>,
        /// Maximum matching posts (defaults to `search.default_limit`).
        #[arg(long)]
        limit: Option<u32>,
        #[arg(long)]
        min_width: Option<u32>,
        #[arg(long)]
        max_width: Option<u32>,
        #[arg(long)]
        min_height: Option<u32>,
        #[arg(long)]
        max_height: Option<u32>,
        #[arg(long)]
        min_size: Option<u64>,
        #[arg(long)]
        max_size: Option<u64>,
        #[arg(long)]
        min_score: Option<i64>,
        #[arg(long)]
        max_score: Option<i64>,
        #[arg(long)]
        ext: Vec<String>,
        #[arg(long)]
        include: Vec<String>,
        #[arg(long)]
        exclude: Vec<String>,
        #[arg(long)]
        sort: Option<String>,
        #[arg(long)]
        order: Option<String>,
    },
    Download {
        /// tags or post id
        target: String,
        #[arg(long)]
        rating: Option<String>,
        /// Maximum matching posts (defaults to `search.default_limit`).
        #[arg(long)]
        limit: Option<u32>,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        force: bool,
        /// Override the configured filename template for this download.
        #[arg(long)]
        filename_template: Option<String>,
        /// Override the configured folder template for this download.
        #[arg(long)]
        folder_template: Option<String>,
        #[arg(long)]
        include: Vec<String>,
        #[arg(long)]
        exclude: Vec<String>,
        #[arg(long)]
        sort: Option<String>,
        #[arg(long)]
        order: Option<String>,
    },
    Batch {
        file: String,
        /// Maximum matching posts per tag (defaults to `search.default_limit`).
        #[arg(long)]
        limit: Option<u32>,
        #[arg(long)]
        rating: Option<String>,
        #[arg(long)]
        sort: Option<String>,
        #[arg(long)]
        order: Option<String>,
        #[arg(long, default_value_t = 4)]
        jobs: usize,
    },
    History {
        #[arg(long)]
        search: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: i64,
        #[arg(long)]
        clear: bool,
        #[arg(long)]
        export: Option<PathBuf>,
    },
    Preset {
        #[command(subcommand)]
        cmd: PresetCmd,
    },
    Watch {
        tags: String,
        /// Poll interval in seconds (defaults to `watch.interval`).
        #[arg(long)]
        interval: Option<u64>,
        #[arg(long)]
        rating: Option<String>,
        /// Maximum matching posts per poll (defaults to 100).
        #[arg(long)]
        limit: Option<u32>,
        #[arg(long)]
        sort: Option<String>,
        #[arg(long)]
        order: Option<String>,
    },
    Repl,
    Config {
        #[arg(long)]
        show: bool,
        #[arg(long)]
        init: bool,
    },
    /// Inspect and resume downloads left in the persistent queue.
    Queue {
        #[command(subcommand)]
        cmd: QueueCmd,
    },
    Tui,
}

#[derive(Subcommand, Debug)]
pub enum QueueCmd {
    List {
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Resume rows left queued or downloading by an interrupted run.
    Resume,
    /// Requeue failed rows and resume them.
    Retry,
    Clear {
        /// Only clear failed rows.
        #[arg(long)]
        failed: bool,
    },
    Remove {
        id: i64,
    },
}

#[derive(Subcommand, Debug)]
pub enum PresetCmd {
    Create {
        name: String,
        /// Tags for the preset. Prompted interactively when omitted.
        #[arg(long)]
        tags: Option<String>,
        #[arg(long)]
        rating: Option<String>,
        #[arg(long)]
        sort: Option<String>,
        #[arg(long)]
        order: Option<String>,
        /// Override the configured filename template when this preset runs.
        #[arg(long)]
        filename_template: Option<String>,
        /// Override the configured folder template when this preset runs.
        #[arg(long)]
        folder_template: Option<String>,
    },
    List,
    Run {
        name: String,
        /// Override sort for this run (overrides the stored preset).
        #[arg(long)]
        sort: Option<String>,
        /// Override order for this run (overrides the stored preset).
        #[arg(long)]
        order: Option<String>,
    },
    Delete {
        name: String,
    },
}

pub async fn run(cli: Cli) -> anyhow::Result<()> {
    // load config
    let mut cfg = Config::load(cli.config.as_deref())?;
    cfg.apply_env();
    if let Some(c) = cli.concurrency {
        cfg.downloads.concurrency = c;
    }
    if let Some(o) = &cli.output {
        cfg.downloads.directory = o.to_string_lossy().to_string();
    }
    if let Some(u) = &cli.user_id {
        cfg.api.user_id = u.clone();
    }
    if let Some(k) = &cli.api_key {
        cfg.api.api_key = k.clone();
    }

    let db = if cfg.database.enabled {
        Some(Arc::new(HistoryDb::open(&cfg.db_path())?))
    } else {
        None
    };

    let client = GelbooruClient::new(&cfg)?;

    match cli.command.unwrap_or(Commands::Tui) {
        Commands::Search {
            tags,
            rating,
            limit,
            min_width,
            max_width,
            min_height,
            max_height,
            min_size,
            max_size,
            min_score,
            max_score,
            ext,
            include,
            exclude,
            sort,
            order,
        } => {
            let mut rules = FilterRules::default();
            if let Some(r) = &rating {
                rules.ratings = crate::filters::rules::parse_ratings(r);
            }
            rules.include_tags = include;
            rules.exclude_tags = exclude;
            rules.min_width = min_width;
            rules.max_width = max_width;
            rules.min_height = min_height;
            rules.max_height = max_height;
            rules.min_size = min_size;
            rules.max_size = max_size;
            rules.min_score = min_score;
            rules.max_score = max_score;
            rules.extensions = ext;
            let params = search::SearchParams {
                tags: tags.clone(),
                rating: rating.clone(),
                limit: limit.unwrap_or(cfg.search.default_limit),
                page_size: cfg.search.page_size,
                sort: sort.clone(),
                order: order.clone(),
                filters: rules,
            };
            let posts = search::search_all(&client, &cfg, &params).await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&posts)?);
            } else {
                for p in &posts {
                    println!(
                        "{} | {} | {}x{} | score {} | {} | {}",
                        p.id,
                        p.rating,
                        p.width,
                        p.height,
                        p.score,
                        p.extension(),
                        p.file_url
                    );
                }
                if !cli.quiet {
                    println!("Total: {}", posts.len());
                }
            }
        }
        Commands::Inspect { id } => {
            let post = client.get_post(id).await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&post)?);
            } else {
                println!("{:#?}", post);
            }
        }
        Commands::Export {
            tags,
            format,
            file,
            rating,
            limit,
            min_width,
            max_width,
            min_height,
            max_height,
            min_size,
            max_size,
            min_score,
            max_score,
            ext,
            include,
            exclude,
            sort,
            order,
        } => {
            let mut rules = FilterRules::default();
            if let Some(r) = &rating {
                rules.ratings = crate::filters::rules::parse_ratings(r);
            }
            rules.include_tags = include;
            rules.exclude_tags = exclude;
            rules.min_width = min_width;
            rules.max_width = max_width;
            rules.min_height = min_height;
            rules.max_height = max_height;
            rules.min_size = min_size;
            rules.max_size = max_size;
            rules.min_score = min_score;
            rules.max_score = max_score;
            rules.extensions = ext;
            let params = search::SearchParams {
                tags: tags.clone(),
                rating: rating.clone(),
                limit: limit.unwrap_or(cfg.search.default_limit),
                page_size: cfg.search.page_size,
                sort: sort.clone(),
                order: order.clone(),
                filters: rules,
            };
            let posts = search::search_all(&client, &cfg, &params).await?;
            let format = format.to_ascii_lowercase();
            match format.as_str() {
                "json" => {
                    let data = serde_json::to_string_pretty(&posts)?;
                    std::fs::write(&file, data)?;
                }
                "csv" => {
                    let mut w = String::new();
                    w.push_str("id,rating,width,height,score,extension,file_url,tags\n");
                    for p in &posts {
                        // Quote fields that may contain commas/newlines.
                        let tags = p.tags.replace('"', "\"\"");
                        let url = p.file_url.replace('"', "\"\"");
                        let line = format!(
                            "{},{},{},{},{},{},{},{}",
                            p.id,
                            p.rating,
                            p.width,
                            p.height,
                            p.score,
                            p.extension(),
                            url,
                            tags
                        );
                        w.push_str(&line);
                        w.push('\n');
                    }
                    std::fs::write(&file, w)?;
                }
                other => {
                    anyhow::bail!("unsupported export format '{}': expected 'json' or 'csv'", other);
                }
            }
            if !cli.quiet {
                println!("Exported {} post(s) to {}", posts.len(), file.display());
            }
        }
        Commands::Download {
            target,
            rating,
            limit,
            output,
            force,
            filename_template,
            folder_template,
            include,
            exclude,
            sort,
            order,
        } => {
            let out_dir = output
                .map(|p| paths::expand_tilde(&p.to_string_lossy()))
                .or_else(|| Some(cfg.download_dir()));
            if force {
                cfg.downloads.skip_duplicates = false;
            }
            let mut engine = DownloadEngine::new(cfg.clone(), db.clone());
            engine.label = Some(target.clone());
            engine.rating = rating.clone();
            engine.sort = sort.clone();
            engine.order = order.clone();
            engine.filename_template = filename_template;
            engine.folder_template = folder_template;
            // determine if target is id
            if let Ok(id) = target.parse::<i64>() {
                let post = client.get_post(id).await?;
                let pb = if cli.quiet {
                    None
                } else {
                    Some(download_bytes_bar(post.file_size.max(0) as u64, 1))
                };
                engine.progress_bar = pb.clone();
                let outcome = engine.download_post(post, out_dir).await;
                if let Some(pb) = &pb {
                    pb.finish_and_clear();
                }
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&outcome)?);
                } else {
                    println!("{}", outcome.summary());
                    if let Some(err) = &outcome.error {
                        eprintln!("error: {err}");
                    }
                }
            } else {
                let mut rules = FilterRules::default();
                if let Some(r) = &rating {
                    rules.ratings = crate::filters::rules::parse_ratings(r);
                }
                rules.include_tags = include;
                rules.exclude_tags = exclude;
                search::validate_sort_order(&sort, &order)?;
                let params = search::SearchParams {
                    tags: target.clone(),
                    rating: rating.clone(),
                    limit: limit.unwrap_or(cfg.search.default_limit),
                    page_size: cfg.search.page_size,
                    sort: sort.clone(),
                    order: order.clone(),
                    filters: rules,
                };
                let posts = search::search_all(&client, &cfg, &params).await?;
                let total_bytes: u64 = posts
                    .iter()
                    .filter(|p| p.file_size > 0)
                    .map(|p| p.file_size as u64)
                    .sum();
                let pb = if cli.quiet || posts.is_empty() {
                    None
                } else if total_bytes > 0 {
                    Some(download_bytes_bar(total_bytes, posts.len()))
                } else {
                    Some(download_count_bar(posts.len()))
                };
                let outcomes = engine.download_many(posts, out_dir, pb).await;
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&outcomes)?);
                } else {
                    let ok = outcomes
                        .iter()
                        .filter(|o| o.error.is_none() && !o.skipped)
                        .count();
                    let skip = outcomes.iter().filter(|o| o.skipped).count();
                    let fail = outcomes.iter().filter(|o| o.error.is_some()).count();
                    println!("completed={} skipped={} failed={}", ok, skip, fail);
                    for o in outcomes {
                        if let Some(e) = o.error {
                            eprintln!("{}: {}", o.post_id, e);
                        }
                    }
                }
            }
        }
        Commands::Batch {
            file,
            limit,
            jobs,
            rating,
            sort,
            order,
        } => {
            search::validate_sort_order(&sort, &order)?;
            let mut engine = DownloadEngine::new(cfg.clone(), db.clone());
            engine.sort = sort.clone();
            engine.order = order.clone();
            let report = batch::run_batch(
                &file,
                &client,
                &cfg,
                &engine,
                batch::BatchOptions {
                    filters: FilterRules::default(),
                    limit_per_tag: limit.unwrap_or(cfg.search.default_limit),
                    rating,
                    sort,
                    order,
                    output_dir: Some(cfg.download_dir()),
                    concurrent_jobs: jobs,
                },
            )
            .await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "Processed {} tag groups: completed={} skipped={} failed={}",
                    report.tags.len(),
                    report.completed(),
                    report.skipped(),
                    report.failed()
                );
                for (tag, error) in &report.errors {
                    if tag.is_empty() {
                        eprintln!("batch error: {error}");
                    } else {
                        eprintln!("{}: search failed: {}", tag, error);
                    }
                }
            }
            if !report.errors.is_empty() {
                anyhow::bail!("{} tag groups failed", report.errors.len());
            }
        }
        Commands::History {
            search,
            limit,
            clear,
            export,
        } => {
            if let Some(db) = &db {
                if clear {
                    db.clear().await?;
                    println!("History cleared.");
                } else if let Some(path) = export {
                    let recs = db.list(100000, search).await?;
                    let mut wtr = std::io::BufWriter::new(csv_writer(&path)?);
                    writeln!(
                        wtr,
                        "post_id,url,local_path,file_size,file_hash,downloaded_at,tags,rating,status"
                    )?;
                    for r in recs {
                        writeln!(
                            wtr,
                            "{},{},{},{},{},{},{},{},{}",
                            r.post_id,
                            csv_escape(&r.url),
                            csv_escape(&r.local_path),
                            r.file_size,
                            csv_escape(&r.file_hash),
                            csv_escape(&r.downloaded_at),
                            csv_escape(&r.tags),
                            csv_escape(&r.rating),
                            csv_escape(&r.status)
                        )?;
                    }
                    println!("Exported to {}", path.display());
                } else {
                    let recs = db.list(limit, search).await?;
                    for r in recs {
                        println!(
                            "{} | {} | {} | {} bytes | {}",
                            r.post_id, r.status, r.rating, r.file_size, r.local_path
                        );
                    }
                    println!("Total records: {}", db.count().await?);
                }
            } else {
                println!("Database disabled.");
            }
        }
        Commands::Preset { cmd } => {
            let db = db
                .clone()
                .unwrap_or_else(|| Arc::new(HistoryDb::open_in_memory().unwrap()));
            let pm = PresetManager::new((*db).clone());
            match cmd {
                PresetCmd::Create {
                    name,
                    tags,
                    rating,
                    sort,
                    order,
                    filename_template,
                    folder_template,
                } => {
                    crate::api::search::validate_sort_order(&sort, &order)?;
                    let tags = match tags {
                        Some(tags) => tags,
                        None => {
                            use std::io::{self, BufRead};
                            print!("Tags: ");
                            io::stdout().flush()?;
                            let mut tags = String::new();
                            io::stdin().lock().read_line(&mut tags)?;
                            tags.trim().to_string()
                        }
                    };
                    let preset = Preset {
                        name: name.clone(),
                        tags,
                        limit: cfg.search.default_limit,
                        rating,
                        sort,
                        order,
                        filename_template,
                        folder_template,
                        ..Default::default()
                    };
                    pm.create(preset).await?;
                    println!("Preset '{}' created.", name);
                }
                PresetCmd::List => {
                    let ps = pm.list().await?;
                    for p in ps {
                        println!("{}: {} (limit {})", p.name, p.tags, p.limit);
                    }
                }
                PresetCmd::Delete { name } => {
                    pm.delete(&name).await?;
                    println!("Deleted {}", name);
                }
                PresetCmd::Run { name, sort, order } => {
                    crate::api::search::validate_sort_order(&sort, &order)?;
                    if let Some(preset) = pm.get(&name).await? {
                        let mut engine = DownloadEngine::new(cfg.clone(), Some(db.clone()));
                        engine.filename_template = preset.filename_template.clone();
                        engine.folder_template = preset.folder_template.clone();
                        let filters = preset.to_filter_rules();
                        let effective_sort = sort.clone().or_else(|| preset.sort.clone());
                        let effective_order = order.clone().or_else(|| preset.order.clone());
                        engine.sort = effective_sort.clone();
                        engine.order = effective_order.clone();
                        let params = search::SearchParams {
                            tags: preset.tags.clone(),
                            rating: preset.rating.clone(),
                            limit: preset.limit,
                            page_size: cfg.search.page_size,
                            sort: effective_sort,
                            order: effective_order,
                            filters,
                        };
                        let posts = search::search_all(&client, &cfg, &params).await?;
                        let out_dir = preset.output_dir.as_ref().map(|d| paths::expand_tilde(d));
                        let total_bytes: u64 = posts
                            .iter()
                            .filter(|p| p.file_size > 0)
                            .map(|p| p.file_size as u64)
                            .sum();
                        let pb = if cli.quiet || posts.is_empty() {
                            None
                        } else if total_bytes > 0 {
                            Some(download_bytes_bar(total_bytes, posts.len()))
                        } else {
                            Some(download_count_bar(posts.len()))
                        };
                        let outcomes = engine.download_many(posts, out_dir, pb).await;
                        if cli.json {
                            println!("{}", serde_json::to_string_pretty(&outcomes)?);
                        } else {
                            let ok = outcomes
                                .iter()
                                .filter(|o| o.error.is_none() && !o.skipped)
                                .count();
                            let skip = outcomes.iter().filter(|o| o.skipped).count();
                            let fail = outcomes.iter().filter(|o| o.error.is_some()).count();
                            println!(
                                "Run '{}': completed={} skipped={} failed={}",
                                name, ok, skip, fail
                            );
                            for o in outcomes {
                                if let Some(e) = o.error {
                                    eprintln!("{}: {}", o.post_id, e);
                                }
                            }
                        }
                    } else {
                        println!("Preset not found.");
                    }
                }
            }
        }
        Commands::Watch {
            tags,
            interval,
            rating,
            limit,
            sort,
            order,
        } => {
            search::validate_sort_order(&sort, &order)?;
            let db = db
                .clone()
                .unwrap_or_else(|| Arc::new(HistoryDb::open_in_memory().unwrap()));
            let mut engine = DownloadEngine::new(cfg.clone(), Some(db.clone()));
            engine.sort = sort.clone();
            engine.order = order.clone();
            let interval = interval.unwrap_or(cfg.watch.interval).max(1);
            let watcher = Watcher {
                tags: tags.clone(),
                interval,
                client: client.clone(),
                cfg: cfg.clone(),
                db: db.clone(),
                engine,
                filters: FilterRules::default(),
                rating,
                limit: limit.unwrap_or(100),
                sort,
                order,
                output_dir: Some(cfg.download_dir()),
            };
            let cancel = tokio_util::sync::CancellationToken::new();
            let c = cancel.clone();
            tokio::spawn(async move {
                tokio::signal::ctrl_c().await.ok();
                c.cancel();
            });
            println!(
                "Watching '{}' every {}s. Press Ctrl+C to stop.",
                tags, interval
            );
            watcher.run(cancel).await;
        }
        Commands::Repl => {
            let db = db
                .clone()
                .unwrap_or_else(|| Arc::new(HistoryDb::open_in_memory().unwrap()));
            let ctx = repl::ReplContext {
                config: cfg,
                client,
                db,
            };
            repl::run(ctx).await?;
        }
        Commands::Config { show, init } => {
            if init {
                cfg.save(cli.config.as_deref())?;
                println!("Config written.");
            }
            if show || !init {
                println!("{}", toml::to_string_pretty(&cfg)?);
            }
        }
        Commands::Queue { cmd } => {
            let db = db.as_ref().ok_or_else(|| {
                anyhow::anyhow!("database is disabled; enable [database] in the config")
            })?;
            match cmd {
                QueueCmd::List { limit } => {
                    let rows = db.queue_list(limit).await?;
                    if cli.json {
                        println!("{}", serde_json::to_string_pretty(&rows)?);
                    } else if rows.is_empty() {
                        println!("Download queue is empty.");
                    } else {
                        for row in rows {
                            let error = row
                                .error
                                .as_deref()
                                .map(|e| format!(" | {e}"))
                                .unwrap_or_default();
                            println!(
                                "{} | {} | post {} | {}x{} | {} | attempts {}{}",
                                row.id,
                                row.status,
                                row.post_id,
                                row.post.width,
                                row.post.height,
                                row.output_dir,
                                row.attempts,
                                error
                            );
                        }
                        println!("Total queue records: {}", db.queue_list(-1).await?.len());
                    }
                }
                QueueCmd::Resume => {
                    let report = crate::queue::resume(db, &cfg).await?;
                    if cli.json {
                        println!("{}", serde_json::to_string_pretty(&report)?);
                    } else if report.resumed == 0 {
                        println!("Download queue is empty.");
                    } else {
                        println!(
                            "Resumed {} download(s): completed={} skipped={} failed={}",
                            report.resumed, report.completed, report.skipped, report.failed
                        );
                    }
                }
                QueueCmd::Retry => {
                    let report = crate::queue::retry_failed(db, &cfg).await?;
                    if cli.json {
                        println!("{}", serde_json::to_string_pretty(&report)?);
                    } else if report.resumed == 0 {
                        println!("No failed downloads to retry.");
                    } else {
                        println!(
                            "Retried {} download(s): completed={} skipped={} failed={}",
                            report.resumed, report.completed, report.skipped, report.failed
                        );
                    }
                }
                QueueCmd::Clear { failed } => {
                    let status = failed.then_some("failed");
                    let removed = db.queue_clear(status).await?;
                    println!("Removed {removed} queue record(s).");
                }
                QueueCmd::Remove { id } => {
                    db.queue_remove(id).await?;
                    println!("Removed queue record {id}.");
                }
            }
        }
        Commands::Tui => {
            tui::run_tui(cfg).await?;
        }
    }
    Ok(())
}

fn csv_writer(path: &PathBuf) -> anyhow::Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(std::fs::File::create(path)?)
}

/// Quote a CSV field when it contains a comma, quote, or line break.
fn csv_escape(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

/// Spinner frames, ending in a check mark for the finished state.
const SPINNER_TICKS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "✔"];

fn styled_progress_bar(len: u64, template: &str) -> ProgressBar {
    let pb = ProgressBar::new(len);
    pb.set_style(
        ProgressStyle::with_template(template)
            .expect("valid progress template")
            .progress_chars("█░")
            .tick_strings(SPINNER_TICKS),
    );
    pb.enable_steady_tick(std::time::Duration::from_millis(120));
    pb
}

/// Byte-based bar used while downloading. Shows transferred bytes, total
/// size, speed and the number of finished files.
const BYTES_TEMPLATE: &str = "{spinner:.green} [{bar:24.cyan/blue}] {percent:>3}% {binary_bytes:>9}/{binary_total_bytes:>9} {binary_bytes_per_sec:>10} {msg}";

/// File-count bar used when the API does not report any file sizes.
const COUNT_TEMPLATE: &str =
    "{spinner:.green} [{bar:24.cyan/blue}] {percent:>3}% {pos}/{len} files";

fn download_bytes_bar(total_bytes: u64, total_files: usize) -> ProgressBar {
    let pb = styled_progress_bar(total_bytes, BYTES_TEMPLATE);
    pb.set_message(format!("0/{total_files} files"));
    pb
}

fn download_count_bar(total_files: usize) -> ProgressBar {
    let pb = styled_progress_bar(total_files as u64, COUNT_TEMPLATE);
    pb.set_message("downloading");
    pb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_bar_templates_are_valid() {
        for template in [BYTES_TEMPLATE, COUNT_TEMPLATE] {
            ProgressStyle::with_template(template)
                .unwrap_or_else(|e| panic!("invalid template {template:?}: {e}"));
        }
    }

    #[test]
    fn csv_fields_are_escaped() {
        assert_eq!(csv_escape("plain"), "plain");
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
        assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_escape("line\nbreak"), "\"line\nbreak\"");
    }
}
