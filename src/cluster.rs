use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::fs;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::config::{ServerConfig, ServerMode};
use crate::errors::Result;
use crate::models::ReplicationEvent;

#[derive(Debug, Clone)]
pub struct ClusterState {
    node_id: String,
    enabled: bool,
    peers: Arc<Mutex<BTreeSet<String>>>,
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
    let message = format!("uds:{}:{}", cluster.node_id(), config.public_base_url);
    socket
        .send_to(message.as_bytes(), config.cluster.broadcast_addr)
        .await?;
    Ok(())
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
