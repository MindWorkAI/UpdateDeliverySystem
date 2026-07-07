use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::errors::Result;

#[derive(Debug, Clone)]
pub struct StatsRecorder {
    raw_dir: PathBuf,
    rollup_path: PathBuf,
    lock: Arc<Mutex<()>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsEvent {
    pub kind: StatsEventKind,
    pub channel: String,
    pub version: Option<String>,
    pub target: Option<String>,
    pub arch: Option<String>,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatsEventKind {
    UpdateCheck,
    Download,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ChannelStats {
    pub update_checks: u64,
    pub downloads: u64,
    pub traffic_bytes: u64,
    pub by_platform: BTreeMap<String, PlatformStats>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PlatformStats {
    pub downloads: u64,
    pub traffic_bytes: u64,
}

impl StatsRecorder {
    pub async fn new(data_dir: PathBuf) -> Result<Self> {
        let raw_dir = data_dir.join("stats/raw");
        let rollup_path = data_dir.join("stats/rollups/channels.json");
        fs::create_dir_all(&raw_dir).await?;
        if let Some(parent) = rollup_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        Ok(Self {
            raw_dir,
            rollup_path,
            lock: Arc::new(Mutex::new(())),
        })
    }

    pub async fn record(&self, event: StatsEvent) -> Result<()> {
        let path = self.raw_dir.join(format!("{}.json", Uuid::new_v4()));
        let bytes = serde_json::to_vec(&event)?;
        fs::write(path, bytes).await?;
        Ok(())
    }

    pub async fn rollup_now(&self) -> Result<BTreeMap<String, ChannelStats>> {
        let _guard = self.lock.lock().await;
        let mut rollup = self.read_rollup().await?;
        let mut entries = fs::read_dir(&self.raw_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if !entry.file_type().await?.is_file() {
                continue;
            }
            let path = entry.path();
            let bytes = fs::read(&path).await?;
            let event: StatsEvent = serde_json::from_slice(&bytes)?;
            apply_event(&mut rollup, event);
            fs::remove_file(path).await?;
        }

        let bytes = serde_json::to_vec_pretty(&rollup)?;
        fs::write(&self.rollup_path, bytes).await?;
        Ok(rollup)
    }

    pub async fn channel_stats(&self, channel: &str) -> Result<ChannelStats> {
        let rollup = self.rollup_now().await?;
        Ok(rollup.get(channel).cloned().unwrap_or_default())
    }

    async fn read_rollup(&self) -> Result<BTreeMap<String, ChannelStats>> {
        if !self.rollup_path.exists() {
            return Ok(BTreeMap::new());
        }
        let bytes = fs::read(&self.rollup_path).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

fn apply_event(rollup: &mut BTreeMap<String, ChannelStats>, event: StatsEvent) {
    let channel = rollup.entry(event.channel).or_default();
    match event.kind {
        StatsEventKind::UpdateCheck => {
            channel.update_checks += 1;
        }
        StatsEventKind::Download => {
            channel.downloads += 1;
            channel.traffic_bytes += event.bytes;
            let platform_key = match (event.target, event.arch) {
                (Some(target), Some(arch)) => format!("{target}-{arch}"),
                _ => "unknown".to_string(),
            };
            let platform = channel.by_platform.entry(platform_key).or_default();
            platform.downloads += 1;
            platform.traffic_bytes += event.bytes;
        }
    }
}
