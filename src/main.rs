use std::sync::Arc;

use clap::Parser;
use update_delivery_system::cluster::{ClusterState, spawn_background_tasks};
use update_delivery_system::config::{Cli, CliCommand};
use update_delivery_system::{AppState, ServerConfig, build_router};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if let Some(CliCommand::Client { command }) = cli.command {
        update_delivery_system::logging::init_client_logging()?;
        update_delivery_system::client::run(command).await?;
        return Ok(());
    }

    let config = ServerConfig::load(&cli).await?;
    let logging = update_delivery_system::logging::init_server_logging(&config)?;
    let storage = update_delivery_system::storage::Storage::new(
        config.data_dir.clone(),
        config.public_base_url.clone(),
    )
    .await?;
    let stats = update_delivery_system::stats::StatsRecorder::new(
        config.data_dir.clone(),
        config.stats.clone(),
    )
    .await?;
    let cluster = ClusterState::new(&config).await?;
    tracing::info!(
        mode = ?config.mode,
        bind = %config.bind,
        public_base_url = %config.public_base_url,
        tls_mode = ?config.tls.mode,
        log_file = ?logging.active_file_path(),
        node_id = cluster.node_id(),
        "starting UDS"
    );

    spawn_background_tasks(config.clone(), cluster.clone());

    let state = AppState {
        config: Arc::new(config.clone()),
        storage: Arc::new(storage),
        stats: Arc::new(stats),
        cluster,
        logging: Arc::new(logging),
    };
    let router = build_router(state);

    update_delivery_system::tls::serve(config, router).await?;
    Ok(())
}
