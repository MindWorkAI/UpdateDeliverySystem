use std::sync::Arc;

use clap::Parser;
use tracing_subscriber::EnvFilter;
use update_delivery_system::cluster::{ClusterState, spawn_background_tasks};
use update_delivery_system::config::Cli;
use update_delivery_system::{AppState, ServerConfig, build_router};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let cli = Cli::parse();
    let config = ServerConfig::load(&cli).await?;
    let storage = update_delivery_system::storage::Storage::new(config.data_dir.clone(), config.public_base_url.clone()).await?;
    let stats = update_delivery_system::stats::StatsRecorder::new(config.data_dir.clone()).await?;
    let cluster = ClusterState::new(&config).await?;

    spawn_background_tasks(config.clone(), cluster.clone());

    let state = AppState {
        config: Arc::new(config.clone()),
        storage: Arc::new(storage),
        stats: Arc::new(stats),
        cluster,
    };
    let router = build_router(state);

    update_delivery_system::tls::serve(config, router).await?;
    Ok(())
}
