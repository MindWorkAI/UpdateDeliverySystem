//! Command-line contract for the UDS server and administration client.
//!
//! Keeping Clap-specific types separate prevents command parsing concerns from
//! obscuring the persisted server configuration schema.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use super::LogLevel;

/// Root command-line input accepted by the `uds` executable.
#[derive(Debug, Parser)]
#[command(name = "uds", version = crate::build_info::CLAP_VERSION)]
pub struct Cli {
    /// Optional command; omitting it prints the top-level help.
    #[command(subcommand)]
    pub command: Option<CliCommand>,
}

/// Top-level operating modes exposed by the UDS executable.
#[derive(Debug, Subcommand)]
pub enum CliCommand {
    /// Show UDS version and build information.
    Version,

    /// Browse the embedded UDS changelog.
    Changelog,

    /// Run the UDS update delivery server.
    Server(ServerArgs),

    /// Run the interactive UDS administration client.
    Client {
        /// Optional client operation; omitting it starts the guided client flow.
        #[command(subcommand)]
        command: Option<ClientCommand>,
    },
}

/// Options used to start or configure the server process.
#[derive(Debug, Args)]
pub struct ServerArgs {
    /// Path to a TOML configuration file.
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Force single-node mode and disable peer discovery and replication.
    #[arg(long)]
    pub single_node_mode: bool,

    /// Optional server maintenance operation.
    #[command(subcommand)]
    pub command: Option<ServerCommand>,
}

/// Maintenance operations that belong to the server installation.
#[derive(Debug, Subcommand)]
pub enum ServerCommand {
    /// Interactively create or update a single-node server configuration.
    Configure(ConfigureServerArgs),
}

/// Input for the guided server configuration workflow.
#[derive(Debug, Args)]
pub struct ConfigureServerArgs {
    /// Path to the TOML configuration file to create or update.
    #[arg(long)]
    pub config: Option<PathBuf>,
}

/// Administrative operations executed by the interactive UDS client.
#[derive(Debug, Clone, Subcommand)]
pub enum ClientCommand {
    /// Create or update the local client configuration.
    Configure,

    /// Upload a release to UDS.
    Upload,

    /// Withdraw a release from a channel.
    Withdraw,

    /// Copy a release from one channel to another.
    Copy,

    /// Update the changelog for an existing release.
    Changelog,

    /// Show channel statistics.
    Stats,

    /// Manage personal admin tokens using the break-glass owner token.
    Tokens {
        /// Token operation to execute.
        #[command(subcommand)]
        command: TokenCommand,
    },

    /// Show UDS service logs.
    Logs {
        /// Follow appended log events.
        #[arg(long)]
        follow: bool,

        /// Number of recent log lines to show first.
        #[arg(long, default_value_t = 200)]
        lines: usize,

        /// Minimum level to display locally.
        #[arg(long)]
        level: Option<LogLevel>,

        /// Disable local terminal colors.
        #[arg(long)]
        no_color: bool,
    },
}

/// Owner-authorized operations for the personal admin-token lifecycle.
#[derive(Debug, Clone, Subcommand)]
pub enum TokenCommand {
    /// List token metadata and immutable status history.
    List,

    /// Create a personal or purpose-bound admin token.
    Create,

    /// Enable an admin token.
    Enable {
        /// Stable identifier of the administrator token to reactivate.
        id: uuid::Uuid,
    },

    /// Disable an admin token.
    Disable {
        /// Stable identifier of the administrator token to deactivate.
        id: uuid::Uuid,
    },
}
