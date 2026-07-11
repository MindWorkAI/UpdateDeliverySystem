use std::sync::Arc;

use clap::{CommandFactory, Parser};
use update_delivery_system::cluster::{ClusterState, spawn_background_tasks};
use update_delivery_system::config::{Cli, CliCommand, ServerArgs};
use update_delivery_system::{AppState, ServerConfig, build_router};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(CliCommand::Server(args)) => run_server(args).await,
        Some(CliCommand::Client { command }) => {
            update_delivery_system::logging::init_client_logging()?;
            update_delivery_system::client::run(command).await?;
            Ok(())
        }
        None => {
            Cli::command().print_help()?;
            println!();
            Ok(())
        }
    }
}

async fn run_server(args: ServerArgs) -> anyhow::Result<()> {
    let config = ServerConfig::load(&args).await?;
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
