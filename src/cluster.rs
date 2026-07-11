use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::fs;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::auth::AdminTokenRecord;
use crate::config::{ServerConfig, ServerMode};
use crate::errors::Result;
use crate::models::ReplicationEvent;

#[derive(Debug, Clone)]
pub struct ClusterState {
    node_id: String,
    enabled: bool,
    peers: Arc<Mutex<BTreeSet<String>>>,
    cluster_token: Option<String>,
}

impl ClusterState {
    pub async fn new(config: &ServerConfig) -> Result<Self> {
        let enabled = config.mode == ServerMode::Fleet;
        let node_id =
            load_or_create_node_id(config.data_dir.join(&config.cluster.node_id_path)).await?;
        Ok(Self {
            node_id,
            enabled,
            peers: Arc::new(Mutex::new(BTreeSet::new())),
            cluster_token: config.cluster_token.clone(),
        })
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    pub async fn replicate_event(&self, event: ReplicationEvent) -> bool {
        if !self.enabled {
            return false;
        }

        let peers = self.peers.lock().await;
        debug!(
            event_id = event.event_id,
            peer_count = peers.len(),
            "queued replication event"
        );
        !peers.is_empty()
    }

    pub async fn peers(&self) -> Vec<String> {
        self.peers.lock().await.iter().cloned().collect()
    }

    pub async fn replicate_auth_snapshot(&self, records: &[AdminTokenRecord]) -> bool {
        if !self.enabled {
            return true;
        }
        let peers = self.peers().await;
        let Some(token) = &self.cluster_token else {
            return false;
        };
        let client = reqwest::Client::new();
        for peer in peers {
            let url = format!("{}/fleet/v1/auth/admin-tokens", peer.trim_end_matches('/'));
            let Ok(response) = client
                .post(url)
                .bearer_auth(token)
                .json(records)
                .timeout(Duration::from_secs(15))
                .send()
                .await
            else {
                return false;
            };
            if !response.status().is_success() {
                return false;
            }
        }
        true
    }
}

pub fn spawn_background_tasks(config: ServerConfig, cluster: ClusterState) {
    if !cluster.enabled() {
        return;
    }

    tokio::spawn(async move {
        let interval = Duration::from_secs(config.cluster.broadcast_interval_seconds);
        loop {
            if let Err(error) = broadcast_presence(&config, &cluster).await {
                warn!(%error, "failed to broadcast cluster presence");
            }
            tokio::time::sleep(interval).await;
        }
    });
}

async fn broadcast_presence(config: &ServerConfig, cluster: &ClusterState) -> Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    socket.set_broadcast(true)?;
    let message = presence_message(config, cluster);
    socket
        .send_to(message.as_bytes(), config.cluster.broadcast_addr)
        .await?;
    Ok(())
}

fn presence_message(config: &ServerConfig, cluster: &ClusterState) -> String {
    let fleet_base_url = &config
        .fleet_api
        .as_ref()
        .expect("validated fleet_api")
        .fleet_base_url;
    format!("uds:{}:{}", cluster.node_id(), fleet_base_url)
}

async fn load_or_create_node_id(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    if path.exists() {
        return Ok(fs::read_to_string(path).await?.trim().to_string());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }

    let node_id = Uuid::new_v4().to_string();
    fs::write(path, &node_id).await?;
    Ok(node_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FleetApiConfig, TlsConfig};

    #[tokio::test]
    async fn discovery_advertises_fleet_base_url() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = ServerConfig::test_default();
        config.data_dir = temp.path().into();
        config.mode = ServerMode::Fleet;
        config.cluster_token = Some("a-long-cluster-token".into());
        config.fleet_api = Some(FleetApiConfig {
            bind: "10.20.0.12:8082".parse().unwrap(),
            fleet_base_url: "http://10.20.0.12:8082".into(),
            tls: TlsConfig::default(),
        });
        let cluster = ClusterState::new(&config).await.unwrap();
        assert!(presence_message(&config, &cluster).ends_with(":http://10.20.0.12:8082"));
        assert!(!presence_message(&config, &cluster).contains(&config.public_base_url));
    }
}
