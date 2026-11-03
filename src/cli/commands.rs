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
    about = "High-performance Gelbooru downloader"
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
    #[arg(long, global = true)]
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
        #[arg(long, default_value_t = 100)]
        limit: u32,
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
        sort: Option<String>,
        #[arg(long, default_value = "desc")]
        order: Option<String>,
    },
    Inspect {
        id: i64,
    },
    Download {
        /// tags or post id
        target: String,
        #[arg(long)]
        rating: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: u32,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        sort: Option<String>,
        #[arg(long, default_value = "desc")]
        order: Option<String>,
        #[arg(long)]
        force: bool,
    },
    Batch {
        file: String,
        #[arg(long, default_value_t = 100)]
        limit: u32,
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
        #[arg(long, default_value_t = 300)]
        interval: u64,
    },
    Repl,
    Config {
        #[arg(long)]
        show: bool,
        #[arg(long)]
        init: bool,
    },
    Tui,
}

#[derive(Subcommand, Debug)]
pub enum PresetCmd {
    Create { name: String },
    List,
    Run { name: String },
    Delete { name: String },
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
            sort,
            order,
        } => {
            let params = search::SearchParams {
                tags: tags.clone(),
                rating: rating.clone(),
                limit,
                page_size: cfg.search.page_size,
                sort: sort.clone(),
                order: order.clone(),
            };
            // rating default?
            let posts = search::search_all(&client, &cfg, &params).await?;
            let mut rules = FilterRules::default();
            if let Some(r) = &rating {
                rules.ratings = crate::filters::rules::parse_ratings(r);
            }
            rules.min_width = min_width;
            rules.max_width = max_width;
            rules.min_height = min_height;
            rules.max_height = max_height;
            rules.min_size = min_size;
            rules.max_size = max_size;
            rules.min_score = min_score;
            rules.max_score = max_score;
            rules.extensions = ext;
            let posts = rules.filter_posts(posts);
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
                println!("Total: {}", posts.len());
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
        Commands::Download {
            target,
            rating,
            limit,
            output,
            sort,
            order,
            force,
        } => {
            let out_dir = output
                .map(|p| paths::expand_tilde(&p.to_string_lossy()))
                .or_else(|| Some(cfg.download_dir()));
            if force {
                cfg.downloads.skip_duplicates = false;
            }
            let mut engine = DownloadEngine::new(cfg.clone(), db.clone());
            engine.sort = sort.clone();
            engine.order = order.clone();
            engine.label = Some(target.clone());
            // determine if target is id
            if let Ok(id) = target.parse::<i64>() {
                let post = client.get_post(id).await?;
                let o = engine.download_post(post, out_dir).await;
                println!("{:?}", o);
            } else {
                let params = search::SearchParams {
                    tags: target.clone(),
                    rating: rating.clone(),
                    limit,
                    page_size: cfg.search.page_size,
                    sort: sort.clone(),
                    order: order.clone(),
                };
                let posts = search::search_all(&client, &cfg, &params).await?;
                let mut rules = FilterRules::default();
                if let Some(r) = &rating {
                    rules.ratings = crate::filters::rules::parse_ratings(r);
                }
                let posts = rules.filter_posts(posts);
                let pb = ProgressBar::new(posts.len() as u64);
                pb.set_style(ProgressStyle::default_bar()
                    .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")
                    .unwrap()
                    .progress_chars("#>-"));
                let outcomes = engine.download_many(posts, out_dir, Some(pb.clone())).await;
                pb.finish_with_message("done");
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
        Commands::Batch { file, limit, jobs } => {
            let engine = DownloadEngine::new(cfg.clone(), db.clone());
            let done = batch::run_batch(
                &file,
                &client,
                &cfg,
                &engine,
                FilterRules::default(),
                limit,
                Some(cfg.download_dir()),
                jobs,
            )
            .await?;
            println!("Processed {} tag groups", done.len());
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
                    let recs = db.list(100000, None).await?;
                    let mut wtr = csv_writer(&path)?;
                    for r in recs {
                        writeln!(
                            wtr,
                            "{},{},{},{},{}",
                            r.post_id, r.url, r.local_path, r.file_size, r.file_hash
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
                PresetCmd::Create { name } => {
                    // interactive minimal: create empty preset with name and tags prompt
                    use std::io::{self, Write};
                    print!("Tags: ");
                    io::stdout().flush().unwrap();
                    let mut tags = String::new();
                    io::stdin().read_line(&mut tags).unwrap();
                    let preset = Preset {
                        name: name.clone(),
                        tags: tags.trim().to_string(),
                        limit: 100,
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
                PresetCmd::Run { name } => {
                    if let Some(preset) = pm.get(&name).await? {
                        let engine = DownloadEngine::new(cfg.clone(), Some(db.clone()));
                        let params = search::SearchParams {
                            tags: preset.tags.clone(),
                            rating: preset.rating.clone(),
                            limit: preset.limit,
                            page_size: cfg.search.page_size,
                            sort: None,
                            order: None,
                        };
                        let posts = search::search_all(&client, &cfg, &params).await?;
                        let posts = preset.to_filter_rules().filter_posts(posts);
                        let out_dir = preset.output_dir.as_ref().map(|d| paths::expand_tilde(d));
                        let outcomes = engine.download_many(posts, out_dir, None).await;
                        println!("Run '{}': {} downloads", name, outcomes.len());
                    } else {
                        println!("Preset not found.");
                    }
                }
            }
        }
        Commands::Watch { tags, interval } => {
            let db = db
                .clone()
                .unwrap_or_else(|| Arc::new(HistoryDb::open_in_memory().unwrap()));
            let engine = DownloadEngine::new(cfg.clone(), Some(db.clone()));
            let watcher = Watcher {
                tags: tags.clone(),
                interval,
                client: client.clone(),
                cfg: cfg.clone(),
                db: db.clone(),
                engine,
                filters: FilterRules::default(),
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
