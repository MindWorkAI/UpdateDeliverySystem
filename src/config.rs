use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

use crate::errors::{Result, UdsError};

#[derive(Debug, Parser)]
#[command(name = "uds", about = "MindWork AI Studio Update Delivery System")]
pub struct Cli {
    /// Path to a TOML configuration file.
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Force single-node mode and disable peer discovery and replication.
    #[arg(long)]
    pub single_node_mode: bool,

    #[command(subcommand)]
    pub command: Option<CliCommand>,
}

#[derive(Debug, Subcommand)]
pub enum CliCommand {
    /// Run the interactive UDS administration client.
    Client {
        #[command(subcommand)]
        command: Option<ClientCommand>,
    },
}

#[derive(Debug, Clone, Copy, Subcommand)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServerMode {
    Fleet,
    SingleNode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TlsMode {
    Off,
    Files,
    Acme,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_mode")]
    pub mode: ServerMode,

    #[serde(default = "default_bind")]
    pub bind: SocketAddr,

    pub public_base_url: String,
    pub data_dir: PathBuf,
    pub admin_token: String,

    #[serde(default)]
    pub cluster_token: Option<String>,

    #[serde(default = "default_channels")]
    pub channels: BTreeSet<String>,

    #[serde(default)]
    pub tls: TlsConfig,

    #[serde(default)]
    pub cluster: ClusterConfig,

    #[serde(default)]
    pub logging: LoggingConfig,

    #[serde(default)]
    pub upload: UploadConfig,

    #[serde(default)]
    pub stats: StatsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadConfig {
    #[serde(default = "default_max_artifact_size_mb")]
    pub max_artifact_size_mb: u64,

    #[serde(default = "default_max_total_artifact_size_mb")]
    pub max_total_artifact_size_mb: u64,

    #[serde(default = "default_max_metadata_size_kb")]
    pub max_metadata_size_kb: u64,

    #[serde(default = "default_max_platforms")]
    pub max_platforms: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsConfig {
    #[serde(default = "default_stats_queue_capacity")]
    pub queue_capacity: usize,

    #[serde(default = "default_stats_max_pending_events")]
    pub max_pending_events: usize,

    #[serde(default = "default_stats_rollup_trigger_events")]
    pub rollup_trigger_events: usize,

    #[serde(default = "default_stats_rollup_interval_seconds")]
    pub rollup_interval_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    #[serde(default = "default_tls_mode")]
    pub mode: TlsMode,

    #[serde(default)]
    pub cert_path: Option<PathBuf>,

    #[serde(default)]
    pub key_path: Option<PathBuf>,

    #[serde(default)]
    pub acme_domains: Vec<String>,

    #[serde(default)]
    pub acme_contact_email: Option<String>,

    #[serde(default)]
    pub acme_use_staging: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterConfig {
    #[serde(default = "default_node_id_path")]
    pub node_id_path: PathBuf,

    #[serde(default = "default_broadcast_addr")]
    pub broadcast_addr: SocketAddr,

    #[serde(default = "default_broadcast_interval_seconds")]
    pub broadcast_interval_seconds: u64,

    #[serde(default = "default_reconcile_interval_seconds")]
    pub reconcile_interval_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,

    #[serde(default)]
    pub filter: String,

    #[serde(default)]
    pub console: LoggingConsoleConfig,

    #[serde(default)]
    pub file: LoggingFileConfig,

    #[serde(default)]
    pub admin_api: LoggingAdminApiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConsoleConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default)]
    pub color: LoggingColorMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingFileConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default)]
    pub path: Option<PathBuf>,

    #[serde(default = "default_max_log_size_mb")]
    pub max_size_mb: u64,

    #[serde(default = "default_max_archived_log_files")]
    pub max_archived_files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingAdminApiConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LoggingColorMode {
    #[default]
    Auto,
    Always,
    Never,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            mode: default_tls_mode(),
            cert_path: None,
            key_path: None,
            acme_domains: Vec::new(),
            acme_contact_email: None,
            acme_use_staging: false,
        }
    }
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            node_id_path: default_node_id_path(),
            broadcast_addr: default_broadcast_addr(),
            broadcast_interval_seconds: default_broadcast_interval_seconds(),
            reconcile_interval_seconds: default_reconcile_interval_seconds(),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            filter: String::new(),
            console: LoggingConsoleConfig::default(),
            file: LoggingFileConfig::default(),
            admin_api: LoggingAdminApiConfig::default(),
        }
    }
}

impl Default for LoggingConsoleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            color: LoggingColorMode::Auto,
        }
    }
}

impl Default for LoggingFileConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: None,
            max_size_mb: default_max_log_size_mb(),
            max_archived_files: default_max_archived_log_files(),
        }
    }
}

impl Default for LoggingAdminApiConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl Default for UploadConfig {
    fn default() -> Self {
        Self {
            max_artifact_size_mb: default_max_artifact_size_mb(),
            max_total_artifact_size_mb: default_max_total_artifact_size_mb(),
            max_metadata_size_kb: default_max_metadata_size_kb(),
            max_platforms: default_max_platforms(),
        }
    }
}

impl Default for StatsConfig {
    fn default() -> Self {
        Self {
            queue_capacity: default_stats_queue_capacity(),
            max_pending_events: default_stats_max_pending_events(),
            rollup_trigger_events: default_stats_rollup_trigger_events(),
            rollup_interval_seconds: default_stats_rollup_interval_seconds(),
        }
    }
}

impl UploadConfig {
    pub fn policy(&self) -> Result<crate::models::UploadPolicy> {
        let max_artifact_bytes = self
            .max_artifact_size_mb
            .checked_mul(1024 * 1024)
            .ok_or_else(|| {
                UdsError::Config("upload.max_artifact_size_mb is too large".to_string())
            })?;
        let max_total_artifact_bytes = self
            .max_total_artifact_size_mb
            .checked_mul(1024 * 1024)
            .ok_or_else(|| {
                UdsError::Config("upload.max_total_artifact_size_mb is too large".to_string())
            })?;
        let max_metadata_bytes = self.max_metadata_size_kb.checked_mul(1024).ok_or_else(|| {
            UdsError::Config("upload.max_metadata_size_kb is too large".to_string())
        })?;
        Ok(crate::models::UploadPolicy {
            max_artifact_bytes,
            max_total_artifact_bytes,
            max_metadata_bytes,
            max_platforms: self.max_platforms,
        })
    }
}

impl ServerConfig {
    pub async fn load(cli: &Cli) -> Result<Self> {
        let mut config = if let Some(path) = &cli.config {
            let text = tokio::fs::read_to_string(path).await?;
            toml::from_str::<ServerConfig>(&text)?
        } else {
            Self::development_default()
        };

        if cli.single_node_mode {
            config.mode = ServerMode::SingleNode;
        }

        config.validate()?;
        Ok(config)
    }

    pub fn development_default() -> Self {
        Self {
            mode: ServerMode::SingleNode,
            bind: default_bind(),
            public_base_url: "http://127.0.0.1:8080".to_string(),
            data_dir: PathBuf::from("./uds-data"),
            admin_token: "change-me-admin-token".to_string(),
            cluster_token: None,
            channels: default_channels(),
            tls: TlsConfig::default(),
            cluster: ClusterConfig::default(),
            logging: LoggingConfig::default(),
            upload: UploadConfig::default(),
            stats: StatsConfig::default(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.public_base_url.trim().is_empty() {
            return Err(UdsError::Config(
                "public_base_url must not be empty".to_string(),
            ));
        }

        if self.admin_token.len() < 16 {
            return Err(UdsError::Config(
                "admin_token must contain at least 16 characters".to_string(),
            ));
        }

        if self.channels.is_empty() {
            return Err(UdsError::Config(
                "at least one channel must be configured".to_string(),
            ));
        }

        if self.mode == ServerMode::Fleet
            && self.cluster_token.as_deref().unwrap_or_default().len() < 16
        {
            return Err(UdsError::Config(
                "cluster_token must contain at least 16 characters in fleet mode".to_string(),
            ));
        }

        match self.tls.mode {
            TlsMode::Off => {}
            TlsMode::Files => {
                require_existing_file(self.tls.cert_path.as_deref(), "tls.cert_path")?;
                require_existing_file(self.tls.key_path.as_deref(), "tls.key_path")?;
            }
            TlsMode::Acme => {
                if self.tls.acme_domains.is_empty() {
                    return Err(UdsError::Config(
                        "tls.acme_domains must contain at least one domain in ACME mode"
                            .to_string(),
                    ));
                }
                if self.tls.acme_contact_email.is_none() {
                    return Err(UdsError::Config(
                        "tls.acme_contact_email is required in ACME mode".to_string(),
                    ));
                }
            }
        }

        if self.logging.level.trim().is_empty() {
            return Err(UdsError::Config(
                "logging.level must not be empty".to_string(),
            ));
        }
        if self.logging.file.enabled && self.logging.file.max_size_mb == 0 {
            return Err(UdsError::Config(
                "logging.file.max_size_mb must be greater than 0".to_string(),
            ));
        }

        if self.upload.max_artifact_size_mb == 0
            || self.upload.max_total_artifact_size_mb == 0
            || self.upload.max_metadata_size_kb == 0
            || self.upload.max_platforms == 0
        {
            return Err(UdsError::Config(
                "upload limits must be greater than zero".to_string(),
            ));
        }
        if self.upload.max_artifact_size_mb > self.upload.max_total_artifact_size_mb {
            return Err(UdsError::Config(
                "upload.max_artifact_size_mb must not exceed upload.max_total_artifact_size_mb"
                    .to_string(),
            ));
        }
        self.upload.policy()?;
        if self.stats.queue_capacity == 0
            || self.stats.max_pending_events == 0
            || self.stats.rollup_trigger_events == 0
            || self.stats.rollup_interval_seconds == 0
        {
            return Err(UdsError::Config(
                "stats limits and intervals must be greater than zero".to_string(),
            ));
        }
        if self.stats.rollup_trigger_events > self.stats.max_pending_events {
            return Err(UdsError::Config(
                "stats.rollup_trigger_events must not exceed stats.max_pending_events".to_string(),
            ));
        }

        Ok(())
    }

    pub fn channel_is_allowed(&self, channel: &str) -> bool {
        self.channels.contains(channel)
    }
}

fn require_existing_file(path: Option<&Path>, name: &str) -> Result<()> {
    let path = path.ok_or_else(|| UdsError::Config(format!("{name} is required")))?;
    if !path.is_file() {
        return Err(UdsError::Config(format!(
            "{name} must point to an existing file"
        )));
    }
    Ok(())
}

fn default_mode() -> ServerMode {
    ServerMode::Fleet
}

fn default_tls_mode() -> TlsMode {
    TlsMode::Off
}

fn default_bind() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8080)
}

fn default_channels() -> BTreeSet<String> {
    ["stable", "beta", "experimental", "mature"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn default_node_id_path() -> PathBuf {
    PathBuf::from("node-id")
}

fn default_broadcast_addr() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), 44231)
}

fn default_broadcast_interval_seconds() -> u64 {
    30
}

fn default_reconcile_interval_seconds() -> u64 {
    300
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_true() -> bool {
    true
}

fn default_max_log_size_mb() -> u64 {
    100
}

fn default_max_archived_log_files() -> usize {
    5
}

fn default_max_artifact_size_mb() -> u64 {
    512
}
fn default_max_total_artifact_size_mb() -> u64 {
    2048
}
fn default_max_metadata_size_kb() -> u64 {
    1024
}
fn default_max_platforms() -> usize {
    32
}
fn default_stats_queue_capacity() -> usize {
    8192
}
fn default_stats_max_pending_events() -> usize {
    100_000
}
fn default_stats_rollup_trigger_events() -> usize {
    10_000
}
fn default_stats_rollup_interval_seconds() -> u64 {
    900
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_can_force_single_node_mode() {
        let mut config = ServerConfig::development_default();
        config.mode = ServerMode::Fleet;
        assert_eq!(config.mode, ServerMode::Fleet);
    }

    #[test]
    fn fleet_mode_requires_cluster_token() {
        let mut config = ServerConfig::development_default();
        config.mode = ServerMode::Fleet;
        config.cluster_token = None;
        let result = config.validate();
        assert!(result.is_err());
    }
}
