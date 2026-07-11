//! Server configuration schema, defaults, loading, and validation.
//!
//! UDS validates configuration before starting any listener so later runtime
//! code can rely on security-sensitive invariants.

mod cli;

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::errors::{Result, UdsError};

pub use cli::{Cli, CliCommand, ClientCommand, ConfigureServerArgs, ServerArgs, ServerCommand, TokenCommand};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
/// Severity threshold shared by server logging and client-side filtering.
pub enum LogLevel {
    /// Represents the item case in UDS.
    Trace,

    /// Represents the item case in UDS.
    Debug,

    /// Represents the item case in UDS.
    Info,

    /// Represents the item case in UDS.
    Warn,

    /// Represents the item case in UDS.
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Deployment topology that controls whether fleet-only services are enabled.
pub enum ServerMode {
    /// Represents the item case in UDS.
    Fleet,

    /// Represents the item case in UDS.
    SingleNode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// TLS provisioning strategy for one HTTP listener.
pub enum TlsMode {
    /// Represents the item case in UDS.
    Off,

    /// Represents the item case in UDS.
    Files,

    /// Represents the item case in UDS.
    Acme,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Complete validated configuration required to run a UDS server node.
pub struct ServerConfig {
    /// The mode carried by this UDS data contract.
    #[serde(default = "default_mode")]
    pub mode: ServerMode,

    /// The public api carried by this UDS data contract.
    pub public_api: ListenerConfig,

    /// The admin api carried by this UDS data contract.
    pub admin_api: ListenerConfig,

    /// The fleet api carried by this UDS data contract.
    #[serde(default)]
    pub fleet_api: Option<FleetApiConfig>,

    /// The public base url carried by this UDS data contract.
    pub public_base_url: String,

    /// The data dir carried by this UDS data contract.
    pub data_dir: PathBuf,

    /// The owner token verifier carried by this UDS data contract.
    pub owner_token_verifier: String,

    /// The cluster token carried by this UDS data contract.
    #[serde(default)]
    pub cluster_token: Option<String>,

    /// The channels carried by this UDS data contract.
    #[serde(default = "default_channels")]
    pub channels: BTreeSet<String>,

    /// The cluster carried by this UDS data contract.
    #[serde(default)]
    pub cluster: ClusterConfig,

    /// The logging carried by this UDS data contract.
    #[serde(default)]
    pub logging: LoggingConfig,

    /// The upload carried by this UDS data contract.
    #[serde(default)]
    pub upload: UploadConfig,

    /// The stats carried by this UDS data contract.
    #[serde(default)]
    pub stats: StatsConfig,

    /// The shutdown carried by this UDS data contract.
    #[serde(default)]
    pub shutdown: ShutdownConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Network binding and TLS settings shared by the public and admin APIs.
pub struct ListenerConfig {
    /// The bind carried by this UDS data contract.
    pub bind: SocketAddr,

    /// The tls carried by this UDS data contract.
    #[serde(default)]
    pub tls: TlsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Private listener and advertised URL used for node-to-node fleet traffic.
pub struct FleetApiConfig {
    /// The bind carried by this UDS data contract.
    pub bind: SocketAddr,

    /// The fleet base url carried by this UDS data contract.
    pub fleet_base_url: String,

    /// The tls carried by this UDS data contract.
    #[serde(default)]
    pub tls: TlsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Resource limits protecting the server from oversized release uploads.
pub struct UploadConfig {
    /// The max artifact size mb carried by this UDS data contract.
    #[serde(default = "default_max_artifact_size_mb")]
    pub max_artifact_size_mb: u64,

    /// The max total artifact size mb carried by this UDS data contract.
    #[serde(default = "default_max_total_artifact_size_mb")]
    pub max_total_artifact_size_mb: u64,

    /// The max metadata size kb carried by this UDS data contract.
    #[serde(default = "default_max_metadata_size_kb")]
    pub max_metadata_size_kb: u64,

    /// The max platforms carried by this UDS data contract.
    #[serde(default = "default_max_platforms")]
    pub max_platforms: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Queue and rollup tuning for asynchronous usage statistics.
pub struct StatsConfig {
    /// The queue capacity carried by this UDS data contract.
    #[serde(default = "default_stats_queue_capacity")]
    pub queue_capacity: usize,

    /// The max pending events carried by this UDS data contract.
    #[serde(default = "default_stats_max_pending_events")]
    pub max_pending_events: usize,

    /// The rollup trigger events carried by this UDS data contract.
    #[serde(default = "default_stats_rollup_trigger_events")]
    pub rollup_trigger_events: usize,

    /// The rollup interval seconds carried by this UDS data contract.
    #[serde(default = "default_stats_rollup_interval_seconds")]
    pub rollup_interval_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Graceful-shutdown timing applied when listeners begin draining.
pub struct ShutdownConfig {
    /// The grace period seconds carried by this UDS data contract.
    #[serde(default = "default_shutdown_grace_period_seconds")]
    pub grace_period_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Certificate configuration for one HTTPS listener.
pub struct TlsConfig {
    /// The mode carried by this UDS data contract.
    #[serde(default = "default_tls_mode")]
    pub mode: TlsMode,

    /// The cert path carried by this UDS data contract.
    #[serde(default)]
    pub cert_path: Option<PathBuf>,

    /// The key path carried by this UDS data contract.
    #[serde(default)]
    pub key_path: Option<PathBuf>,

    /// The acme domains carried by this UDS data contract.
    #[serde(default)]
    pub acme_domains: Vec<String>,

    /// The acme contact email carried by this UDS data contract.
    #[serde(default)]
    pub acme_contact_email: Option<String>,

    /// The acme use staging carried by this UDS data contract.
    #[serde(default)]
    pub acme_use_staging: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Node identity, discovery, and reconciliation settings for fleet mode.
pub struct ClusterConfig {
    /// The node id path carried by this UDS data contract.
    #[serde(default = "default_node_id_path")]
    pub node_id_path: PathBuf,

    /// The broadcast addr carried by this UDS data contract.
    #[serde(default = "default_broadcast_addr")]
    pub broadcast_addr: SocketAddr,

    /// The broadcast interval seconds carried by this UDS data contract.
    #[serde(default = "default_broadcast_interval_seconds")]
    pub broadcast_interval_seconds: u64,

    /// The reconcile interval seconds carried by this UDS data contract.
    #[serde(default = "default_reconcile_interval_seconds")]
    pub reconcile_interval_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Structured logging destinations and privacy controls.
pub struct LoggingConfig {
    /// The level carried by this UDS data contract.
    #[serde(default = "default_log_level")]
    pub level: String,

    /// The filter carried by this UDS data contract.
    #[serde(default)]
    pub filter: String,

    /// The client ip carried by this UDS data contract.
    #[serde(default)]
    pub client_ip: ClientIpLoggingMode,

    /// The console carried by this UDS data contract.
    #[serde(default)]
    pub console: LoggingConsoleConfig,

    /// The file carried by this UDS data contract.
    #[serde(default)]
    pub file: LoggingFileConfig,

    /// The admin api carried by this UDS data contract.
    #[serde(default)]
    pub admin_api: LoggingAdminApiConfig,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Privacy policy controlling when request logs may contain client IPs.
pub enum ClientIpLoggingMode {
    /// Represents the item case in UDS.
    Never,

    /// Represents the item case in UDS.
    #[default]
    AuditSecurity,

    /// Represents the item case in UDS.
    Always,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Human-readable console logging settings.
pub struct LoggingConsoleConfig {
    /// The enabled carried by this UDS data contract.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// The color carried by this UDS data contract.
    #[serde(default)]
    pub color: LoggingColorMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Rotating NDJSON file logging settings.
pub struct LoggingFileConfig {
    /// The enabled carried by this UDS data contract.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// The path carried by this UDS data contract.
    #[serde(default)]
    pub path: Option<PathBuf>,

    /// The max size mb carried by this UDS data contract.
    #[serde(default = "default_max_log_size_mb")]
    pub max_size_mb: u64,

    /// The max archived files carried by this UDS data contract.
    #[serde(default = "default_max_archived_log_files")]
    pub max_archived_files: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Controls whether authenticated administrators may query stored logs.
pub struct LoggingAdminApiConfig {
    /// The enabled carried by this UDS data contract.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
/// Policy for enabling ANSI colors in human-readable log output.
pub enum LoggingColorMode {
    /// Represents the item case in UDS.
    #[default]
    Auto,

    /// Represents the item case in UDS.
    Always,

    /// Represents the item case in UDS.
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
    /// Provides the policy operation used by UDS callers.
    pub fn policy(&self) -> Result<crate::models::UploadPolicy> {
        let max_artifact_bytes = self
            .max_artifact_size_mb
            .checked_mul(1024 * 1024)
            .ok_or_else(|| UdsError::Config("upload.max_artifact_size_mb is too large".to_string()))?;
        let max_total_artifact_bytes = self
            .max_total_artifact_size_mb
            .checked_mul(1024 * 1024)
            .ok_or_else(|| UdsError::Config("upload.max_total_artifact_size_mb is too large".to_string()))?;
        let max_metadata_bytes = self
            .max_metadata_size_kb
            .checked_mul(1024)
            .ok_or_else(|| UdsError::Config("upload.max_metadata_size_kb is too large".to_string()))?;
        Ok(crate::models::UploadPolicy {
            max_artifact_bytes,
            max_total_artifact_bytes,
            max_metadata_bytes,
            max_platforms: self.max_platforms,
        })
    }
}

impl ServerConfig {
    /// Retrieves the load information required by the caller.
    pub async fn load(args: &ServerArgs) -> Result<Self> {
        let path = args.config.as_ref().ok_or_else(|| {
            UdsError::Config(
                "server configuration is required; pass --config <path> or run 'uds server configure'".to_string(),
            )
        })?;
        let text = tokio::fs::read_to_string(path).await?;
        let mut config = toml::from_str::<ServerConfig>(&text)?;

        if args.single_node_mode {
            config.mode = ServerMode::SingleNode;
        }

        config.validate()?;
        Ok(config)
    }

    /// Performs the single node template operation required by UDS.
    fn single_node_template() -> Self {
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
            public_base_url: "https://updates.example.org".to_string(),
            data_dir: PathBuf::from("/var/lib/uds"),
            owner_token_verifier: String::new(),
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
        let mut config = Self::single_node_template();
        config.cluster.node_id_path = config.data_dir.join("node-id");
        config
    }

    #[cfg(test)]
    pub(crate) fn test_default() -> Self {
        let mut config = Self::single_node_template();
        config.public_base_url = "http://127.0.0.1:8080".to_string();
        config.data_dir = PathBuf::from("./uds-data");
        config.owner_token_verifier = crate::auth::verifier("uds_owner_v1_test-only-owner-token");
        config.cluster.node_id_path = config.data_dir.join("node-id");
        config
    }

    /// Validates the validate input before UDS trusts or persists it.
    pub fn validate(&self) -> Result<()> {
        if self.public_base_url.trim().is_empty() {
            return Err(UdsError::Config(
                "public_base_url must not be empty".to_string(),
            ));
        }

        if !valid_sha512_verifier(&self.owner_token_verifier) {
            return Err(UdsError::Config(
                "owner_token_verifier must use the format sha512:<128 lowercase hex characters>".to_string(),
            ));
        }

        if self.channels.is_empty() {
            return Err(UdsError::Config(
                "at least one channel must be configured".to_string(),
            ));
        }

        if self.mode == ServerMode::Fleet && self.cluster_token.as_deref().unwrap_or_default().len() < 16 {
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
                "upload.max_artifact_size_mb must not exceed upload.max_total_artifact_size_mb".to_string(),
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

    /// Provides the channel is allowed operation used by UDS callers.
    pub fn channel_is_allowed(&self, channel: &str) -> bool {
        self.channels.contains(channel)
    }
}

/// Performs the valid sha512 verifier operation required by UDS.
fn valid_sha512_verifier(value: &str) -> bool {
    value.strip_prefix("sha512:").is_some_and(|digest| {
        digest.len() == 128 // 512 Bit -> 64 Bytes -> 128 Hex Chars
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

/// Performs the validate tls operation required by UDS.
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

/// Performs the validate fleet base url operation required by UDS.
fn validate_fleet_base_url(value: &str) -> Result<()> {
    let url =
        url::Url::parse(value).map_err(|e| UdsError::Config(format!("fleet_api.fleet_base_url is invalid: {e}")))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(UdsError::Config(
            "fleet_api.fleet_base_url must be an absolute HTTP(S) URL without query, fragment, or path".into(),
        ));
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

/// Performs the require existing file operation required by UDS.
fn require_existing_file(path: Option<&Path>, name: &str) -> Result<()> {
    let path = path.ok_or_else(|| UdsError::Config(format!("{name} is required")))?;
    if !path.is_file() {
        return Err(UdsError::Config(format!(
            "{name} must point to an existing file"
        )));
    }
    Ok(())
}

/// Performs the default mode operation required by UDS.
fn default_mode() -> ServerMode {
    ServerMode::Fleet
}

/// Performs the default tls mode operation required by UDS.
fn default_tls_mode() -> TlsMode {
    TlsMode::Off
}

/// Performs the default public bind operation required by UDS.
fn default_public_bind() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080)
}

/// Performs the default admin bind operation required by UDS.
fn default_admin_bind() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8081)
}

/// Performs the default channels operation required by UDS.
fn default_channels() -> BTreeSet<String> {
    ["stable", "beta", "experimental", "mature"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Performs the default node id path operation required by UDS.
fn default_node_id_path() -> PathBuf {
    PathBuf::from("node-id")
}

/// Performs the default broadcast addr operation required by UDS.
fn default_broadcast_addr() -> SocketAddr {
    SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), 44231)
}

/// Performs the default broadcast interval seconds operation required by UDS.
fn default_broadcast_interval_seconds() -> u64 {
    30
}

/// Performs the default reconcile interval seconds operation required by UDS.
fn default_reconcile_interval_seconds() -> u64 {
    300
}

/// Performs the default log level operation required by UDS.
fn default_log_level() -> String {
    "info".to_string()
}

/// Performs the default true operation required by UDS.
fn default_true() -> bool {
    true
}

/// Performs the default max log size mb operation required by UDS.
fn default_max_log_size_mb() -> u64 {
    100
}

/// Performs the default max archived log files operation required by UDS.
fn default_max_archived_log_files() -> usize {
    5
}

/// Performs the default max artifact size mb operation required by UDS.
fn default_max_artifact_size_mb() -> u64 {
    512
}

/// Performs the default max total artifact size mb operation required by UDS.
fn default_max_total_artifact_size_mb() -> u64 {
    2048
}

/// Performs the default max metadata size kb operation required by UDS.
fn default_max_metadata_size_kb() -> u64 {
    1024
}

/// Performs the default max platforms operation required by UDS.
fn default_max_platforms() -> usize {
    32
}

/// Performs the default stats queue capacity operation required by UDS.
fn default_stats_queue_capacity() -> usize {
    8192
}

/// Performs the default stats max pending events operation required by UDS.
fn default_stats_max_pending_events() -> usize {
    100_000
}

/// Performs the default stats rollup trigger events operation required by UDS.
fn default_stats_rollup_trigger_events() -> usize {
    10_000
}

/// Performs the default stats rollup interval seconds operation required by UDS.
fn default_stats_rollup_interval_seconds() -> u64 {
    900
}

/// Performs the default shutdown grace period seconds operation required by UDS.
fn default_shutdown_grace_period_seconds() -> u64 {
    300
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use clap::Parser;

    /// Verifies that server command accepts server options.
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

    /// Verifies that server configure has its own config option.
    #[test]
    fn server_configure_has_its_own_config_option() {
        let cli = Cli::try_parse_from(["uds", "server", "configure", "--config", "/tmp/uds.toml"]).unwrap();

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

    /// Verifies that client subcommands still parse.
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

    /// Verifies that server runtime requires an explicit config file.
    #[tokio::test]
    async fn server_runtime_requires_an_explicit_config_file() {
        let args = ServerArgs {
            config: None,
            single_node_mode: false,
            command: None,
        };
        let error = ServerConfig::load(&args).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "configuration error: server configuration is required; pass --config <path> or run 'uds server configure'"
        );
    }

    /// Verifies that old root level server options are rejected.
    #[test]
    fn old_root_level_server_options_are_rejected() {
        assert!(Cli::try_parse_from(["uds", "--single-node-mode"]).is_err());
        assert!(Cli::try_parse_from(["uds", "--config", "config.toml"]).is_err());
    }

    /// Verifies that root help lists available commands.
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

    /// Verifies that fleet mode requires cluster token.
    #[test]
    fn fleet_mode_requires_cluster_token() {
        let mut config = ServerConfig::test_default();
        config.mode = ServerMode::Fleet;
        config.cluster_token = None;
        let result = config.validate();
        assert!(result.is_err());
    }

    /// Verifies that fleet api matches server mode and rejects wildcard url.
    #[test]
    fn fleet_api_matches_server_mode_and_rejects_wildcard_url() {
        let mut config = ServerConfig::test_default();
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

    /// Verifies that client ip logging modes parse and default.
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
            let parsed: ClientIpLoggingMode = serde_json::from_str(&format!("\"{value}\"")).unwrap();
            assert_eq!(parsed, expected);
        }
        assert!(serde_json::from_str::<ClientIpLoggingMode>("\"sometimes\"").is_err());
    }

    /// Verifies that shutdown defaults to five minutes and rejects zero.
    #[test]
    fn shutdown_defaults_to_five_minutes_and_rejects_zero() {
        let mut config = ServerConfig::test_default();
        assert_eq!(config.shutdown.grace_period_seconds, 300);
        config.shutdown.grace_period_seconds = 0;
        assert!(config.validate().is_err());
    }

    /// Verifies that existing config without shutdown section gets default.
    #[test]
    fn existing_config_without_shutdown_section_gets_default() {
        let config: ServerConfig = toml::from_str(
            r#"
                mode = "single-node"
                public_base_url = "https://updates.example.org"
                data_dir = "/var/lib/uds"
                owner_token_verifier = "sha512:00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"

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
