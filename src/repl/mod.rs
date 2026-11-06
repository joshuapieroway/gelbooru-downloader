use crate::api::client::GelbooruClient;
use crate::config::Config;
use crate::database::history::HistoryDb;
use crate::utils::error::Result;
use rustyline::DefaultEditor;
use std::sync::Arc;

pub struct ReplContext {
    pub config: Config,
    pub client: GelbooruClient,
    pub db: Arc<HistoryDb>,
}

pub async fn run(ctx: ReplContext) -> Result<()> {
    let mut rl = DefaultEditor::new()
        .map_err(|e| crate::utils::error::AppError::Other(anyhow::anyhow!(e)))?;
    let _ = rl.load_history(&crate::utils::paths::data_dir().join("repl_history.txt"));
    println!("Gelbooru Downloader REPL. Type 'help' for commands, 'exit' to quit.");
    loop {
        let readline = rl.readline("gelbooru> ");
        match readline {
            Ok(line) => {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(&line);
                let parts: Vec<&str> = line.split_whitespace().collect();
                match parts.as_slice() {
                    ["exit"] | ["quit"] => break,
                    ["help"] => {
                        println!(
                            "Commands: search <tags>, inspect <id>, download <id|tags>, history [query], presets, config, exit"
                        );
                    }
                    ["search", rest @ ..] => {
                        let tags = rest.join(" ");
                        println!("Searching: {}", tags);
                        match crate::api::search::search_all(
                            &ctx.client,
                            &ctx.config,
                            &crate::api::search::SearchParams {
                                tags: tags.clone(),
                                rating: None,
                                limit: ctx.config.search.default_limit,
                                page_size: ctx.config.search.page_size,
                                sort: None,
                                order: None,
                                filters: crate::filters::rules::FilterRules::default(),
                            },
                        )
                        .await
                        {
                            Ok(posts) => {
                                for p in posts.iter().take(20) {
                                    println!(
                                        "{} {} {}x{} {}",
                                        p.id, p.rating, p.width, p.height, p.file_url
                                    );
                                }
                                println!("Total: {}", posts.len());
                            }
                            Err(e) => println!("Error: {}", e),
                        }
                    }
                    ["download", rest @ ..] if !rest.is_empty() => {
                        let target = rest.join(" ");
                        let engine = crate::downloader::engine::DownloadEngine::new(
                            ctx.config.clone(),
                            Some(ctx.db.clone()),
                        );
                        let out_dir = Some(ctx.config.download_dir());
                        if let Ok(id) = target.parse::<i64>() {
                            match ctx.client.get_post(id).await {
                                Ok(post) => {
                                    let outcome = engine.download_post(post, out_dir).await;
                                    println!("{}", outcome.summary());
                                }
                                Err(e) => println!("Error: {}", e),
                            }
                        } else {
                            let params = crate::api::search::SearchParams {
                                tags: target.clone(),
                                rating: None,
                                limit: ctx.config.search.default_limit,
                                page_size: ctx.config.search.page_size,
                                sort: None,
                                order: None,
                                filters: crate::filters::rules::FilterRules::default(),
                            };
                            match crate::api::search::search_all(&ctx.client, &ctx.config, &params)
                                .await
                            {
                                Ok(posts) => {
                                    let outcomes = engine.download_many(posts, out_dir, None).await;
                                    for outcome in outcomes {
                                        println!("{}", outcome.summary());
                                    }
                                }
                                Err(e) => println!("Error: {}", e),
                            }
                        }
                    }
                    ["inspect", id] => {
                        if let Ok(n) = id.parse::<i64>() {
                            match ctx.client.get_post(n).await {
                                Ok(p) => println!("{:#?}", p),
                                Err(e) => println!("Error: {}", e),
                            }
                        } else {
                            println!("invalid id");
                        }
                    }
                    ["history"] => match ctx.db.list(20, None).await {
                        Ok(recs) => {
                            for r in recs {
                                println!("{} {} {}", r.post_id, r.status, r.local_path);
                            }
                        }
                        Err(e) => println!("Error: {}", e),
                    },
                    ["history", q] => match ctx.db.list(20, Some(q.to_string())).await {
                        Ok(recs) => {
                            for r in recs {
                                println!("{} {} {}", r.post_id, r.status, r.local_path);
                            }
                        }
                        Err(e) => println!("Error: {}", e),
                    },
                    ["presets"] => {
                        let manager = crate::presets::PresetManager::new((*ctx.db).clone());
                        match manager.list().await {
                            Ok(presets) if presets.is_empty() => println!("No presets."),
                            Ok(presets) => {
                                for preset in presets {
                                    println!(
                                        "{}: {} (limit {})",
                                        preset.name, preset.tags, preset.limit
                                    );
                                }
                            }
                            Err(e) => println!("Error: {}", e),
                        }
                    }
                    ["config"] => match toml::to_string_pretty(&ctx.config) {
                        Ok(text) => print!("{text}"),
                        Err(e) => println!("Error: {}", e),
                    },
                    _ => println!("Unknown command. Type 'help'."),
                }
            }
            Err(_) => break,
        }
    }
    let _ = rl.save_history(&crate::utils::paths::data_dir().join("repl_history.txt"));
    Ok(())
}
