use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::StatsConfig;
use crate::errors::{Result, UdsError};

#[derive(Debug, Clone)]
pub struct StatsRecorder {
    sender: mpsc::Sender<StatsCommand>,
    dropping: Arc<AtomicBool>,
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

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelStats {
    pub update_checks: u64,
    pub downloads: u64,
    pub traffic_bytes: u64,
    pub by_platform: BTreeMap<String, PlatformStats>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlatformStats {
    pub downloads: u64,
    pub traffic_bytes: u64,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct RollupState {
    #[serde(default)]
    channels: BTreeMap<String, ChannelStats>,
    #[serde(default)]
    included_deltas: BTreeSet<String>,
}

enum StatsCommand {
    Event(StatsEvent),
    ChannelStats {
        channel: String,
        response: oneshot::Sender<Result<ChannelStats>>,
    },
}

struct StatsActor {
    events_dir: PathBuf,
    processing_dir: PathBuf,
    deltas_dir: PathBuf,
    rejected_dir: PathBuf,
    rollup_path: PathBuf,
    config: StatsConfig,
    pending_events: usize,
    dropping: Arc<AtomicBool>,
}

impl StatsRecorder {
    pub async fn new(data_dir: PathBuf, config: StatsConfig) -> Result<Self> {
        let stats_root = data_dir.join("stats");
        reject_legacy_stats(&stats_root).await?;
        let actor = StatsActor::new(stats_root, config.clone()).await?;
        let dropping = actor.dropping.clone();
        let (sender, receiver) = mpsc::channel(config.queue_capacity);
        tokio::spawn(actor.run(receiver));
        Ok(Self { sender, dropping })
    }

    pub fn record(&self, event: StatsEvent) -> bool {
        match self.sender.try_send(StatsCommand::Event(event)) {
            Ok(()) => true,
            Err(error) => {
                if !self.dropping.swap(true, Ordering::Relaxed) {
                    warn!(%error, "dropping statistics events because the recorder is overloaded");
                }
                false
            }
        }
    }

    pub async fn channel_stats(&self, channel: &str) -> Result<ChannelStats> {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(StatsCommand::ChannelStats {
                channel: channel.to_string(),
                response: sender,
            })
            .await
            .map_err(|_| UdsError::Storage("statistics recorder is unavailable".to_string()))?;
        receiver.await.map_err(|_| {
            UdsError::Storage("statistics recorder stopped before answering".to_string())
        })?
    }
}

impl StatsActor {
    async fn new(stats_root: PathBuf, config: StatsConfig) -> Result<Self> {
        let events_dir = stats_root.join("events");
        let processing_dir = stats_root.join("processing");
        let deltas_dir = stats_root.join("rollups/deltas");
        let rejected_dir = stats_root.join("rejected");
        let rollup_path = stats_root.join("rollups/channels.json");
        for directory in [&events_dir, &processing_dir, &deltas_dir, &rejected_dir] {
            fs::create_dir_all(directory).await?;
        }
        if rollup_path.exists() {
            let bytes = fs::read(&rollup_path).await?;
            serde_json::from_slice::<RollupState>(&bytes).map_err(|error| {
                UdsError::Storage(format!(
                    "unsupported legacy statistics file '{}': {error}; remove the unused old data before starting UDS",
                    rollup_path.display()
                ))
            })?;
        }
        let pending_events = count_files(&events_dir).await?;
        let mut actor = Self {
            events_dir,
            processing_dir,
            deltas_dir,
            rejected_dir,
            rollup_path,
            config,
            pending_events,
            dropping: Arc::new(AtomicBool::new(false)),
        };
        actor.recover_processing().await?;
        actor.finish_interrupted_compaction().await?;
        Ok(actor)
    }

    async fn run(mut self, mut receiver: mpsc::Receiver<StatsCommand>) {
        let mut interval =
            tokio::time::interval(Duration::from_secs(self.config.rollup_interval_seconds));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(error) = self.rollup_events().await {
                        warn!(%error, "failed to roll up statistics");
                    }
                }
                command = receiver.recv() => {
                    let Some(command) = command else { break; };
                    match command {
                        StatsCommand::Event(event) => self.persist_event(event).await,
                        StatsCommand::ChannelStats { channel, response } => {
                            let result = async {
                                self.rollup_events().await?;
                                let totals = self.current_totals().await?;
                                Ok(totals.get(&channel).cloned().unwrap_or_default())
                            }.await;
                            let _ = response.send(result);
                        }
                    }
                }
            }
        }
    }

    async fn persist_event(&mut self, event: StatsEvent) {
        if self.pending_events >= self.config.max_pending_events {
            if !self.dropping.swap(true, Ordering::Relaxed) {
                warn!(
                    max_pending_events = self.config.max_pending_events,
                    "dropping statistics events because the pending-event limit was reached"
                );
            }
            return;
        }
        let path = self.events_dir.join(format!("{}.json", Uuid::new_v4()));
        match atomic_write_json(path, &event).await {
            Ok(()) => {
                self.pending_events += 1;
                if self.dropping.swap(false, Ordering::Relaxed) {
                    info!("statistics recording resumed");
                }
                if self.pending_events >= self.config.rollup_trigger_events
                    && let Err(error) = self.rollup_events().await
                {
                    warn!(%error, "failed to roll up statistics after reaching the event threshold");
                }
            }
            Err(error) => {
                if !self.dropping.swap(true, Ordering::Relaxed) {
                    warn!(%error, "dropping statistics events because persistence failed");
                }
            }
        }
    }

    async fn rollup_events(&mut self) -> Result<()> {
        let event_paths = file_paths(&self.events_dir).await?;
        if event_paths.is_empty() {
            return self.compact_if_needed().await;
        }
        let batch_id = Uuid::new_v4().to_string();
        let batch_dir = self.processing_dir.join(&batch_id);
        fs::create_dir(&batch_dir).await?;
        for path in event_paths {
            let target = batch_dir.join(path.file_name().expect("event path has file name"));
            if let Err(error) = fs::rename(&path, target).await {
                warn!(%error, path = %path.display(), "failed to move statistics event into rollup batch");
            }
        }
        let processed = self.process_batch(&batch_id, &batch_dir).await?;
        self.pending_events = self.pending_events.saturating_sub(processed);
        self.compact_if_needed().await
    }

    async fn process_batch(&self, batch_id: &str, batch_dir: &Path) -> Result<usize> {
        let delta_path = self.deltas_dir.join(format!("{batch_id}.json"));
        if delta_path.exists() {
            let count = count_files(batch_dir).await?;
            fs::remove_dir_all(batch_dir).await?;
            return Ok(count);
        }
        let mut delta = BTreeMap::new();
        let paths = file_paths(batch_dir).await?;
        let count = paths.len();
        for path in paths {
            match fs::read(&path).await.and_then(|bytes| {
                serde_json::from_slice::<StatsEvent>(&bytes)
                    .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
            }) {
                Ok(event) => apply_event(&mut delta, event),
                Err(error) => {
                    warn!(%error, path = %path.display(), "quarantining malformed statistics event");
                    let target = self
                        .rejected_dir
                        .join(path.file_name().expect("event path has file name"));
                    fs::rename(&path, target).await?;
                }
            }
        }
        atomic_write_json(delta_path, &delta).await?;
        fs::remove_dir_all(batch_dir).await?;
        Ok(count)
    }

    async fn recover_processing(&mut self) -> Result<()> {
        let mut entries = fs::read_dir(&self.processing_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if !entry.file_type().await?.is_dir() {
                continue;
            }
            let batch_id = entry.file_name().to_string_lossy().to_string();
            let count = self.process_batch(&batch_id, &entry.path()).await?;
            self.pending_events = self.pending_events.saturating_sub(count);
        }
        Ok(())
    }

    async fn compact_if_needed(&self) -> Result<()> {
        if count_files(&self.deltas_dir).await? < 96 {
            return Ok(());
        }
        self.compact().await
    }

    async fn compact(&self) -> Result<()> {
        let mut state = self.read_rollup_state().await?;
        let delta_paths = file_paths(&self.deltas_dir).await?;
        let mut included = BTreeSet::new();
        for path in delta_paths {
            let id = path
                .file_stem()
                .expect("delta path has stem")
                .to_string_lossy()
                .to_string();
            if state.included_deltas.contains(&id) {
                continue;
            }
            let delta: BTreeMap<String, ChannelStats> =
                serde_json::from_slice(&fs::read(&path).await?)?;
            merge_stats(&mut state.channels, delta);
            included.insert(id);
        }
        if included.is_empty() {
            return Ok(());
        }
        state.included_deltas.extend(included);
        atomic_write_json(self.rollup_path.clone(), &state).await?;
        self.finish_interrupted_compaction().await
    }

    async fn finish_interrupted_compaction(&self) -> Result<()> {
        let mut state = self.read_rollup_state().await?;
        if state.included_deltas.is_empty() {
            return Ok(());
        }
        for id in &state.included_deltas {
            let path = self.deltas_dir.join(format!("{id}.json"));
            if path.exists() {
                fs::remove_file(path).await?;
            }
        }
        state.included_deltas.clear();
        atomic_write_json(self.rollup_path.clone(), &state).await
    }

    async fn current_totals(&self) -> Result<BTreeMap<String, ChannelStats>> {
        let state = self.read_rollup_state().await?;
        let mut totals = state.channels;
        for path in file_paths(&self.deltas_dir).await? {
            let id = path
                .file_stem()
                .expect("delta path has stem")
                .to_string_lossy()
                .to_string();
            if state.included_deltas.contains(&id) {
                continue;
            }
            let delta: BTreeMap<String, ChannelStats> =
                serde_json::from_slice(&fs::read(path).await?)?;
            merge_stats(&mut totals, delta);
        }
        Ok(totals)
    }

    async fn read_rollup_state(&self) -> Result<RollupState> {
        if !self.rollup_path.exists() {
            return Ok(RollupState::default());
        }
        Ok(serde_json::from_slice(&fs::read(&self.rollup_path).await?)?)
    }
}

async fn reject_legacy_stats(stats_root: &Path) -> Result<()> {
    let legacy_raw = stats_root.join("raw");
    if legacy_raw.exists() {
        if count_files(&legacy_raw).await? > 0 {
            return Err(UdsError::Storage(format!(
                "legacy statistics events detected at '{}'; migration is intentionally unsupported",
                legacy_raw.display()
            )));
        }
        fs::remove_dir_all(legacy_raw).await?;
    }
    Ok(())
}

fn apply_event(rollup: &mut BTreeMap<String, ChannelStats>, event: StatsEvent) {
    let channel = rollup.entry(event.channel).or_default();
    match event.kind {
        StatsEventKind::UpdateCheck => {
            channel.update_checks = channel.update_checks.saturating_add(1)
        }
        StatsEventKind::Download => {
            channel.downloads = channel.downloads.saturating_add(1);
            channel.traffic_bytes = channel.traffic_bytes.saturating_add(event.bytes);
            let platform_key = match (event.target, event.arch) {
                (Some(target), Some(arch)) => format!("{target}-{arch}"),
                _ => "unknown".to_string(),
            };
            let platform = channel.by_platform.entry(platform_key).or_default();
            platform.downloads = platform.downloads.saturating_add(1);
            platform.traffic_bytes = platform.traffic_bytes.saturating_add(event.bytes);
        }
    }
}

fn merge_stats(
    target: &mut BTreeMap<String, ChannelStats>,
    source: BTreeMap<String, ChannelStats>,
) {
    for (channel, source_stats) in source {
        let target_stats = target.entry(channel).or_default();
        target_stats.update_checks = target_stats
            .update_checks
            .saturating_add(source_stats.update_checks);
        target_stats.downloads = target_stats
            .downloads
            .saturating_add(source_stats.downloads);
        target_stats.traffic_bytes = target_stats
            .traffic_bytes
            .saturating_add(source_stats.traffic_bytes);
        for (platform, source_platform) in source_stats.by_platform {
            let target_platform = target_stats.by_platform.entry(platform).or_default();
            target_platform.downloads = target_platform
                .downloads
                .saturating_add(source_platform.downloads);
            target_platform.traffic_bytes = target_platform
                .traffic_bytes
                .saturating_add(source_platform.traffic_bytes);
        }
    }
}

async fn file_paths(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut entries = fs::read_dir(directory).await?;
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_file() {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

async fn count_files(directory: &Path) -> Result<usize> {
    Ok(file_paths(directory).await?.len())
}

async fn atomic_write_json(path: PathBuf, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    tokio::task::spawn_blocking(move || -> Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| UdsError::Storage("statistics path has no parent".to_string()))?;
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(path)
            .map_err(|error| UdsError::Io(error.error))?;
        Ok(())
    })
    .await
    .map_err(|error| UdsError::Storage(format!("statistics writer task failed: {error}")))??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> StatsConfig {
        StatsConfig {
            queue_capacity: 16,
            max_pending_events: 100,
            rollup_trigger_events: 10,
            rollup_interval_seconds: 3600,
        }
    }

    #[tokio::test]
    async fn channel_query_flushes_queued_events() {
        let temp = tempfile::tempdir().unwrap();
        let recorder = StatsRecorder::new(temp.path().to_path_buf(), config())
            .await
            .unwrap();
        assert!(recorder.record(StatsEvent {
            kind: StatsEventKind::UpdateCheck,
            channel: "stable".to_string(),
            version: None,
            target: Some("linux".to_string()),
            arch: Some("x86_64".to_string()),
            bytes: 0,
        }));
        assert_eq!(
            recorder
                .channel_stats("stable")
                .await
                .unwrap()
                .update_checks,
            1
        );
    }

    #[tokio::test]
    async fn download_rollup_is_saturating_and_platform_specific() {
        let mut stats = BTreeMap::new();
        apply_event(
            &mut stats,
            StatsEvent {
                kind: StatsEventKind::Download,
                channel: "stable".to_string(),
                version: Some("1.0.0".to_string()),
                target: Some("windows".to_string()),
                arch: Some("x86_64".to_string()),
                bytes: 42,
            },
        );
        assert_eq!(stats["stable"].downloads, 1);
        assert_eq!(
            stats["stable"].by_platform["windows-x86_64"].traffic_bytes,
            42
        );
    }

    #[tokio::test]
    async fn recovers_processing_batch_without_double_counting() {
        let temp = tempfile::tempdir().unwrap();
        let batch = temp.path().join("stats/processing/batch-1");
        fs::create_dir_all(&batch).await.unwrap();
        atomic_write_json(
            batch.join("event.json"),
            &StatsEvent {
                kind: StatsEventKind::UpdateCheck,
                channel: "stable".to_string(),
                version: None,
                target: None,
                arch: None,
                bytes: 0,
            },
        )
        .await
        .unwrap();
        let recorder = StatsRecorder::new(temp.path().to_path_buf(), config())
            .await
            .unwrap();
        assert_eq!(
            recorder
                .channel_stats("stable")
                .await
                .unwrap()
                .update_checks,
            1
        );
    }

    #[tokio::test]
    async fn rejects_non_empty_legacy_event_directory() {
        let temp = tempfile::tempdir().unwrap();
        let legacy = temp.path().join("stats/raw");
        fs::create_dir_all(&legacy).await.unwrap();
        fs::write(legacy.join("event.json"), b"{}").await.unwrap();
        let error = StatsRecorder::new(temp.path().to_path_buf(), config())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("legacy statistics"));
    }
}
