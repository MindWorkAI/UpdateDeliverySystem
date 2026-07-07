use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use clap::Parser;
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
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.public_base_url.trim().is_empty() {
            return Err(UdsError::Config("public_base_url must not be empty".to_string()));
        }

        if self.admin_token.len() < 16 {
            return Err(UdsError::Config("admin_token must contain at least 16 characters".to_string()));
        }

        if self.channels.is_empty() {
            return Err(UdsError::Config("at least one channel must be configured".to_string()));
        }

        if self.mode == ServerMode::Fleet && self.cluster_token.as_deref().unwrap_or_default().len() < 16 {
            return Err(UdsError::Config("cluster_token must contain at least 16 characters in fleet mode".to_string()));
        }

        match self.tls.mode {
            TlsMode::Off => {}
            TlsMode::Files => {
                require_existing_file(self.tls.cert_path.as_deref(), "tls.cert_path")?;
                require_existing_file(self.tls.key_path.as_deref(), "tls.key_path")?;
            }
            TlsMode::Acme => {
                if self.tls.acme_domains.is_empty() {
                    return Err(UdsError::Config("tls.acme_domains must contain at least one domain in ACME mode".to_string()));
                }
                if self.tls.acme_contact_email.is_none() {
                    return Err(UdsError::Config("tls.acme_contact_email is required in ACME mode".to_string()));
                }
            }
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
        return Err(UdsError::Config(format!("{name} must point to an existing file")));
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
    ["stable", "beta", "experimental", "lts"]
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
