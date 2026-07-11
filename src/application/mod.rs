//! Binary-only command dispatch and server lifecycle orchestration.
//!
//! Keeping this work outside `main.rs` leaves the executable entry point as a
//! concise overview while the library remains independently testable.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum_server::Handle;
use clap::CommandFactory;
use update_delivery_system::cluster::{ClusterState, spawn_background_tasks};
use update_delivery_system::config::LogLevel;
use update_delivery_system::config::{Cli, CliCommand, ServerArgs, ServerCommand};
use update_delivery_system::logging::{LogEventKind, LoggingRuntime};
use update_delivery_system::shutdown::{ActiveTransfer, ShutdownState};
use update_delivery_system::{AppState, ServerConfig, build_admin_router, build_fleet_router, build_public_router};

/// Dispatches the parsed command and owns the binary application's lifecycle.
pub async fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Some(CliCommand::Version) => {
            println!("{}", update_delivery_system::build_info::display_version());
            Ok(())
        }
        Some(CliCommand::Changelog) => {
            update_delivery_system::changelog::run()?;
            Ok(())
        }
        Some(CliCommand::Server(mut args)) => match args.command.take() {
            Some(ServerCommand::Configure(configure_args)) => {
                update_delivery_system::server_configure::run(configure_args).await?;
                Ok(())
            }
            None => run_server(args).await,
        },
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
    //
    // Initialize every long-lived service before exposing a network listener.
    // This prevents partially initialized nodes from accepting traffic.
    //
    let config = ServerConfig::load(&args).await?;
    let logging = Arc::new(update_delivery_system::logging::init_server_logging(
        &config,
    )?);

    // Storage and statistics share the configured data root but own separate
    // subdirectories and persistence lifecycles.
    let storage =
        update_delivery_system::storage::Storage::new(config.data_dir.clone(), config.public_base_url.clone()).await?;
    let stats =
        update_delivery_system::stats::StatsRecorder::new(config.data_dir.clone(), config.stats.clone()).await?;
    let cluster = ClusterState::new(&config).await?;
    let auth = update_delivery_system::auth::AdminTokenStore::open(&config.data_dir).await?;
    tracing::info!(
        mode = ?config.mode,
        public_base_url = %config.public_base_url,
        public_bind = %config.public_api.bind,
        admin_bind = %config.admin_api.bind,
        fleet_bind = config.fleet_api.as_ref().map(|v| tracing::field::display(v.bind)),
        log_file = logging
            .active_file_path()
            .map(|path| tracing::field::display(path.display())),
        node_id = cluster.node_id(),
        "starting UDS"
    );
    let node_id = cluster.node_id().to_string();

    spawn_background_tasks(config.clone(), cluster.clone());

    // Share lightweight service handles with all three API routers.
    let shutdown = Arc::new(ShutdownState::default());
    let state = AppState {
        config: Arc::new(config.clone()),
        storage: Arc::new(storage),
        stats: Arc::new(stats),
        cluster,
        logging: logging.clone(),
        shutdown: shutdown.clone(),
        auth: Arc::new(auth),
    };
    let stats = state.stats.clone();
    warn_insecure_listener("public", config.public_api.bind, &config.public_api.tls);
    warn_insecure_listener("admin", config.admin_api.bind, &config.admin_api.tls);
    if let Some(fleet) = &config.fleet_api {
        warn_insecure_listener("fleet", fleet.bind, &fleet.tls);
    }

    //
    // Start each configured listener with an independent shutdown handle. A
    // listener failure triggers the same controlled drain as an OS signal.
    //
    let mut listeners = tokio::task::JoinSet::new();
    let mut handles = Vec::new();
    for (name, bind, tls, router) in [
        (
            "public",
            config.public_api.bind,
            config.public_api.tls.clone(),
            build_public_router(state.clone()),
        ),
        (
            "admin",
            config.admin_api.bind,
            config.admin_api.tls.clone(),
            build_admin_router(state.clone()),
        ),
    ] {
        let handle = Handle::new();
        handles.push(handle.clone());
        listeners.spawn(update_delivery_system::tls::serve(
            name, bind, tls, router, handle,
        ));
    }
    if let Some(fleet) = &config.fleet_api {
        let handle = Handle::new();
        handles.push(handle.clone());
        listeners.spawn(update_delivery_system::tls::serve(
            "fleet",
            fleet.bind,
            fleet.tls.clone(),
            build_fleet_router(state),
            handle,
        ));
    }
    let grace_period = Duration::from_secs(config.shutdown.grace_period_seconds);
    let first = tokio::select! {
        result = listeners.join_next() => Err(anyhow::anyhow!("listener ended unexpectedly: {:?}", result)),
        signal = shutdown_signal() => Ok(signal),
    };
    let startup_error = first.err();
    {
        //
        // Stop accepting new connections, allow active transfers to finish,
        // and force the remaining work only after the grace period expires.
        //
        let signal = if startup_error.is_some() {
            "listener-failure"
        } else {
            "signal"
        };
        let started = Instant::now();
        let totals_before = shutdown.totals();
        shutdown.begin_draining();
        emit_shutdown_started(&logging, signal, grace_period, shutdown.active_count());
        for handle in &handles {
            handle.graceful_shutdown(None);
        }

        let deadline = tokio::time::sleep(grace_period);
        tokio::pin!(deadline);
        let mut forced = false;
        let result = loop {
            tokio::select! {
                result = listeners.join_next(), if !listeners.is_empty() => {
                    if listeners.is_empty() { break result; }
                }
                _ = &mut deadline, if !forced => {
                    forced = true;
                    emit_forced_transfers(&logging, shutdown.mark_active_forced(), "deadline");
                    for handle in &handles { handle.shutdown(); }
                }
                second_signal = shutdown_signal(), if !forced => {
                    forced = true;
                    emit_forced_transfers(&logging, shutdown.mark_active_forced(), second_signal);
                    for handle in &handles { handle.shutdown(); }
                }
                else => break None,
            }
        };

        shutdown.wait_for_no_transfers().await;
        stats.flush().await?;
        let totals_after = shutdown.totals();
        emit_shutdown_finished(
            &logging,
            started.elapsed(),
            totals_after
                .completed
                .saturating_sub(totals_before.completed),
            totals_after.aborted.saturating_sub(totals_before.aborted),
            forced,
            &node_id,
        );
        if let Some(result) = result {
            result??;
        }
    }
    if let Some(error) = startup_error {
        return Err(error);
    }
    Ok(())
}

fn warn_insecure_listener(name: &str, bind: std::net::SocketAddr, tls: &update_delivery_system::config::TlsConfig) {
    if tls.mode == update_delivery_system::config::TlsMode::Off && !bind.ip().is_loopback() {
        tracing::warn!(listener = name, %bind, "listener is exposed beyond loopback without TLS; tokens will be transmitted unencrypted");
    }
}

fn emit_shutdown_started(logging: &LoggingRuntime, signal: &str, grace_period: Duration, active_transfers: usize) {
    let mut fields = std::collections::BTreeMap::new();
    fields.insert("signal".into(), serde_json::Value::from(signal));
    fields.insert(
        "grace_period_seconds".into(),
        serde_json::Value::from(grace_period.as_secs()),
    );
    fields.insert(
        "active_transfers".into(),
        serde_json::Value::from(active_transfers as u64),
    );
    let event = logging.event(
        LogLevel::Warn,
        LogEventKind::System,
        "uds::shutdown",
        None,
        fields,
        "shutdown initiated; node is no longer accepting new connections",
    );
    logging.emit(&event);
}

fn emit_forced_transfers(logging: &LoggingRuntime, transfers: Vec<ActiveTransfer>, reason: &str) {
    for transfer in transfers {
        let mut fields = transfer.fields;
        fields.insert(
            "transfer_id".into(),
            serde_json::Value::from(transfer.transfer_id.to_string()),
        );
        fields.insert(
            "request_id".into(),
            serde_json::Value::from(transfer.request_id),
        );
        fields.insert(
            "transfer_kind".into(),
            serde_json::Value::from(transfer.kind.as_str()),
        );
        fields.insert("reason".into(), serde_json::Value::from(reason));
        let event = logging.event(
            LogLevel::Warn,
            LogEventKind::System,
            "uds::shutdown",
            None,
            fields,
            "aborting transfer during forced shutdown",
        );
        logging.emit(&event);
    }
}

fn emit_shutdown_finished(
    logging: &LoggingRuntime,
    elapsed: Duration,
    completed: u64,
    aborted: u64,
    forced: bool,
    node_id: &str,
) {
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "elapsed_ms".into(),
        serde_json::Value::from(elapsed.as_millis() as u64),
    );
    fields.insert(
        "completed_transfers".into(),
        serde_json::Value::from(completed),
    );
    fields.insert("aborted_transfers".into(), serde_json::Value::from(aborted));
    fields.insert("forced".into(), serde_json::Value::from(forced));
    fields.insert("node_id".into(), serde_json::Value::from(node_id));
    let event = logging.event(
        LogLevel::Info,
        LogEventKind::System,
        "uds::shutdown",
        None,
        fields,
        "all running transfers have ended; stopping UDS",
    );
    logging.emit(&event);
}

#[cfg(unix)]
async fn shutdown_signal() -> &'static str {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
    tokio::select! {
        _ = terminate.recv() => "SIGTERM",
        result = tokio::signal::ctrl_c() => {
            result.expect("failed to install Ctrl-C handler");
            "SIGINT"
        }
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() -> &'static str {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install Ctrl-C handler");
    "Ctrl-C"
}
