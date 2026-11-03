use anyhow::Result;
use clap::Parser;
use gelbooru_downloader::cli::{Cli, run};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(if cli.verbose { "debug" } else { "info" }));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    run(cli).await
}
