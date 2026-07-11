use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

use crate::errors::{Result, UdsError};

#[derive(Debug, Parser)]
#[command(name = "uds", version = crate::build_info::CLAP_VERSION)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<CliCommand>,
}

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
        #[command(subcommand)]
        command: Option<ClientCommand>,
    },
}

#[derive(Debug, Args)]
pub struct ServerArgs {
    /// Path to a TOML configuration file.
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Force single-node mode and disable peer discovery and replication.
    #[arg(long)]
    pub single_node_mode: bool,

    #[command(subcommand)]
    pub command: Option<ServerCommand>,
}

#[derive(Debug, Subcommand)]
pub enum ServerCommand {
    /// Interactively create or update a single-node server configuration.
    Configure(ConfigureServerArgs),
}

#[derive(Debug, Args)]
pub struct ConfigureServerArgs {
    /// Path to the TOML configuration file to create or update.
    #[arg(long)]
    pub config: Option<PathBuf>,
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

    pub public_api: ListenerConfig,
    pub admin_api: ListenerConfig,
    #[serde(default)]
    pub fleet_api: Option<FleetApiConfig>,

    pub public_base_url: String,
    pub data_dir: PathBuf,
    pub admin_token: String,

    #[serde(default)]
    pub cluster_token: Option<String>,

    #[serde(default = "default_channels")]
    pub channels: BTreeSet<String>,

    #[serde(default)]
    pub cluster: ClusterConfig,

    #[serde(default)]
    pub logging: LoggingConfig,

    #[serde(default)]
    pub upload: UploadConfig,

    #[serde(default)]
    pub stats: StatsConfig,

    #[serde(default)]
    pub shutdown: ShutdownConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenerConfig {
    pub bind: SocketAddr,
    #[serde(default)]
    pub tls: TlsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetApiConfig {
    pub bind: SocketAddr,
    pub fleet_base_url: String,
    #[serde(default)]
    pub tls: TlsConfig,
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
pub struct ShutdownConfig {
    #[serde(default = "default_shutdown_grace_period_seconds")]
    pub grace_period_seconds: u64,
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
    pub client_ip: ClientIpLoggingMode,

    #[serde(default)]
    pub console: LoggingConsoleConfig,

    #[serde(default)]
    pub file: LoggingFileConfig,

    #[serde(default)]
    pub admin_api: LoggingAdminApiConfig,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientIpLoggingMode {
    Never,
    #[default]
    AuditSecurity,
    Always,
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
            client_ip: ClientIpLoggingMode::AuditSecurity,
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

impl Default for ShutdownConfig {
    fn default() -> Self {
        Self {
            grace_period_seconds: default_shutdown_grace_period_seconds(),
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
    pub async fn load(args: &ServerArgs) -> Result<Self> {
        let mut config = if let Some(path) = &args.config {
            let text = tokio::fs::read_to_string(path).await?;
            toml::from_str::<ServerConfig>(&text)?
        } else {
            Self::development_default()
        };

        if args.single_node_mode {
            config.mode = ServerMode::SingleNode;
        }

        config.validate()?;
        Ok(config)
    }

    pub fn development_default() -> Self {
        Self {
            mode: ServerMode::SingleNode,
            public_api: ListenerConfig {
                bind: default_public_bind(),
                tls: TlsConfig::default(),
            },
            admin_api: ListenerConfig {
                bind: default_admin_bind(),
                tls: TlsConfig::default(),
            },
            fleet_api: None,
            public_base_url: "http://127.0.0.1:8080".to_string(),
            data_dir: PathBuf::from("./uds-data"),
            admin_token: "change-me-admin-token".to_string(),
            cluster_token: None,
            channels: default_channels(),
            cluster: ClusterConfig::default(),
            logging: LoggingConfig::default(),
            upload: UploadConfig::default(),
            stats: StatsConfig::default(),
            shutdown: ShutdownConfig::default(),
        }
    }

    /// Safe starting point for an interactively configured production server.
    pub fn production_single_node_default() -> Self {
        let mut config = Self::development_default();
        config.public_base_url = "https://updates.example.org".to_string();
        config.data_dir = PathBuf::from("/var/lib/uds");
        config.admin_token = String::new();
        config.cluster.node_id_path = config.data_dir.join("node-id");
        config
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

        match (self.mode, &self.fleet_api) {
            (ServerMode::Fleet, None) => {
                return Err(UdsError::Config(
                    "fleet_api is required in fleet mode".into(),
                ));
            }
            (ServerMode::SingleNode, Some(_)) => {
                return Err(UdsError::Config(
                    "fleet_api is not allowed in single-node mode".into(),
                ));
            }
            _ => {}
        }

        validate_tls(&self.public_api.tls, "public_api.tls")?;
        validate_tls(&self.admin_api.tls, "admin_api.tls")?;
        if let Some(fleet) = &self.fleet_api {
            validate_tls(&fleet.tls, "fleet_api.tls")?;
            validate_fleet_base_url(&fleet.fleet_base_url)?;
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
        if self.shutdown.grace_period_seconds == 0 {
            return Err(UdsError::Config(
                "shutdown.grace_period_seconds must be greater than zero".to_string(),
            ));
        }

        Ok(())
    }

    pub fn channel_is_allowed(&self, channel: &str) -> bool {
        self.channels.contains(channel)
    }
}

fn validate_tls(tls: &TlsConfig, name: &str) -> Result<()> {
    match tls.mode {
        TlsMode::Off => Ok(()),
        TlsMode::Files => {
            require_existing_file(tls.cert_path.as_deref(), &format!("{name}.cert_path"))?;
            require_existing_file(tls.key_path.as_deref(), &format!("{name}.key_path"))
        }
        TlsMode::Acme => {
            if tls.acme_domains.is_empty() || tls.acme_contact_email.is_none() {
                return Err(UdsError::Config(format!(
                    "{name} requires domains and contact email in ACME mode"
                )));
            }
            Ok(())
        }
    }
}

fn validate_fleet_base_url(value: &str) -> Result<()> {
    let url = url::Url::parse(value)
        .map_err(|e| UdsError::Config(format!("fleet_api.fleet_base_url is invalid: {e}")))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(UdsError::Config("fleet_api.fleet_base_url must be an absolute HTTP(S) URL without query, fragment, or path".into()));
    }
    if url
        .host_str()
        .and_then(|h| h.parse::<IpAddr>().ok())
        .is_some_and(|ip| ip.is_unspecified())
    {
        return Err(UdsError::Config(
            "fleet_api.fleet_base_url must not use a wildcard address".into(),
        ));
    }
    Ok(())
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

fn default_public_bind() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080)
}
fn default_admin_bind() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8081)
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
fn default_shutdown_grace_period_seconds() -> u64 {
    300
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn server_command_accepts_server_options() {
        let cli = Cli::try_parse_from([
            "uds",
            "server",
            "--config",
            "/etc/uds/config.toml",
            "--single-node-mode",
        ])
        .unwrap();

        let Some(CliCommand::Server(args)) = cli.command else {
            panic!("expected server command");
        };
        assert_eq!(args.config, Some(PathBuf::from("/etc/uds/config.toml")));
        assert!(args.single_node_mode);
        assert!(args.command.is_none());
    }

    #[test]
    fn server_configure_has_its_own_config_option() {
        let cli = Cli::try_parse_from(["uds", "server", "configure", "--config", "/tmp/uds.toml"])
            .unwrap();

        let Some(CliCommand::Server(args)) = cli.command else {
            panic!("expected server command");
        };
        assert!(args.config.is_none());
        assert!(matches!(
            args.command,
            Some(ServerCommand::Configure(ConfigureServerArgs { config }))
                if config == Some(PathBuf::from("/tmp/uds.toml"))
        ));
    }

    #[test]
    fn client_subcommands_still_parse() {
        let cli = Cli::try_parse_from(["uds", "client", "upload"]).unwrap();

        assert!(matches!(
            cli.command,
            Some(CliCommand::Client {
                command: Some(ClientCommand::Upload)
            })
        ));
    }

    #[test]
    fn old_root_level_server_options_are_rejected() {
        assert!(Cli::try_parse_from(["uds", "--single-node-mode"]).is_err());
        assert!(Cli::try_parse_from(["uds", "--config", "config.toml"]).is_err());
    }

    #[test]
    fn root_help_lists_available_commands() {
        let mut help = Vec::new();
        Cli::command().write_long_help(&mut help).unwrap();
        let help = String::from_utf8(help).unwrap();

        assert!(help.contains("server"));
        assert!(help.contains("Run the UDS update delivery server"));
        assert!(help.contains("client"));
        assert!(help.contains("Run the interactive UDS administration client"));
    }

    #[test]
    fn fleet_mode_requires_cluster_token() {
        let mut config = ServerConfig::development_default();
        config.mode = ServerMode::Fleet;
        config.cluster_token = None;
        let result = config.validate();
        assert!(result.is_err());
    }

    #[test]
    fn fleet_api_matches_server_mode_and_rejects_wildcard_url() {
        let mut config = ServerConfig::development_default();
        config.fleet_api = Some(FleetApiConfig {
            bind: "10.20.0.12:8082".parse().unwrap(),
            fleet_base_url: "http://10.20.0.12:8082".into(),
            tls: TlsConfig::default(),
        });
        assert!(config.validate().is_err());
        config.mode = ServerMode::Fleet;
        config.cluster_token = Some("a-long-cluster-token".into());
        assert!(config.validate().is_ok());
        config.fleet_api.as_mut().unwrap().fleet_base_url = "http://0.0.0.0:8082".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn client_ip_logging_modes_parse_and_default() {
        assert_eq!(
            LoggingConfig::default().client_ip,
            ClientIpLoggingMode::AuditSecurity
        );
        for (value, expected) in [
            ("never", ClientIpLoggingMode::Never),
            ("audit-security", ClientIpLoggingMode::AuditSecurity),
            ("always", ClientIpLoggingMode::Always),
        ] {
            let parsed: ClientIpLoggingMode =
                serde_json::from_str(&format!("\"{value}\"")).unwrap();
            assert_eq!(parsed, expected);
        }
        assert!(serde_json::from_str::<ClientIpLoggingMode>("\"sometimes\"").is_err());
    }

    #[test]
    fn shutdown_defaults_to_five_minutes_and_rejects_zero() {
        let mut config = ServerConfig::development_default();
        assert_eq!(config.shutdown.grace_period_seconds, 300);
        config.shutdown.grace_period_seconds = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn existing_config_without_shutdown_section_gets_default() {
        let config: ServerConfig = toml::from_str(
            r#"
                mode = "single-node"
                public_base_url = "https://updates.example.org"
                data_dir = "/var/lib/uds"
                admin_token = "a-long-admin-token"

                [public_api]
                bind = "127.0.0.1:8080"
                [admin_api]
                bind = "127.0.0.1:8081"
            "#,
        )
        .unwrap();
        assert_eq!(config.shutdown.grace_period_seconds, 300);
    }
}
