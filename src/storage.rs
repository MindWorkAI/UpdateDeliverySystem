//! Release manifests and content-addressed artifact storage.
//!
//! UDS publishes manifests only after artifacts are verified and durably moved
//! into blob storage, preventing clients from observing partial releases.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use semver::Version;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
use url::Url;
use uuid::Uuid;

use crate::errors::{Result, UdsError};
use crate::models::{
    CatalogEntry, CatalogResponse, PlatformArtifact, ReleaseListEntry, ReleaseListResponse, ReleaseManifest,
    ReleaseUploadMetadata, TauriUpdateResponse, UploadPolicy,
};

#[derive(Debug, Clone)]
/// Artifact streamed to a temporary file while an upload is being validated.
pub struct StagedArtifact {
    pub field_name: String,
    pub path: PathBuf,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone)]
/// File-system repository for release metadata and immutable artifact blobs.
pub struct Storage {
    data_dir: PathBuf,
    public_base_url: Url,
    mutation_lock: Arc<Mutex<()>>,
}

impl Storage {
    pub async fn new(data_dir: PathBuf, public_base_url: String) -> Result<Self> {
        let public_base_url = Url::parse(&public_base_url)
            .map_err(|error| UdsError::Config(format!("public_base_url is not a valid URL: {error}")))?;
        let storage = Self {
            data_dir,
            public_base_url,
            mutation_lock: Arc::new(Mutex::new(())),
        };
        storage.ensure_layout().await?;
        storage.ensure_no_legacy_releases().await?;
        storage.cleanup_staging().await?;
        storage.prune_unreferenced_blobs().await?;
        Ok(storage)
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn upload_staging_root(&self) -> PathBuf {
        self.data_dir.join("staging/uploads")
    }

    pub async fn put_release(
        &self,
        channel: &str,
        metadata: ReleaseUploadMetadata,
        staged_files: BTreeMap<String, StagedArtifact>,
        policy: &UploadPolicy,
    ) -> Result<ReleaseManifest> {
        validate_path_segment(channel, "channel")?;
        validate_upload_metadata(&metadata, &staged_files, policy)?;
        let version = normalize_version(&metadata.version)?;
        let _guard = self.mutation_lock.lock().await;
        let release_dir = self.release_dir(channel, &version);

        if release_dir.exists() {
            return Err(UdsError::Conflict(format!(
                "release {channel}/{version} already exists"
            )));
        }

        let mut platforms = BTreeMap::new();
        for (platform_key, upload_platform) in metadata.platforms {
            let staged = staged_files
                .get(&upload_platform.file_field)
                .ok_or_else(|| {
                    UdsError::BadRequest(format!(
                        "missing multipart file field '{}'",
                        upload_platform.file_field
                    ))
                })?;
            self.publish_blob(staged).await?;
            platforms.insert(
                platform_key,
                PlatformArtifact {
                    file_name: upload_platform.file_name,
                    signature: upload_platform.signature,
                    size: staged.size,
                    sha256: staged.sha256.clone(),
                },
            );
        }

        let manifest = ReleaseManifest {
            version: version.clone(),
            pub_date: metadata.pub_date,
            notes: metadata.notes,
            withdrawn: false,
            platforms,
            updated_at: OffsetDateTime::now_utc(),
        };

        let staging_dir = self
            .data_dir
            .join("staging/releases")
            .join(Uuid::new_v4().to_string());
        fs::create_dir_all(&staging_dir).await?;
        atomic_write_json(staging_dir.join("manifest.json"), &manifest).await?;
        if let Some(parent) = release_dir.parent() {
            fs::create_dir_all(parent).await?;
        }
        match fs::rename(&staging_dir, &release_dir).await {
            Ok(()) => Ok(manifest),
            Err(_error) if release_dir.exists() => {
                let _ = fs::remove_dir_all(&staging_dir).await;
                Err(UdsError::Conflict(format!(
                    "release {channel}/{version} already exists"
                )))
            }
            Err(error) => {
                let _ = fs::remove_dir_all(&staging_dir).await;
                Err(error.into())
            }
        }
    }

    pub async fn patch_changelog(&self, channel: &str, version: &str, notes: String) -> Result<ReleaseManifest> {
        let version = normalize_version(version)?;
        let _guard = self.mutation_lock.lock().await;
        let mut manifest = self.load_manifest(channel, &version).await?;
        manifest.notes = notes;
        manifest.updated_at = OffsetDateTime::now_utc();
        self.save_manifest(channel, &version, &manifest).await?;
        Ok(manifest)
    }

    pub async fn withdraw_release(&self, channel: &str, version: &str) -> Result<ReleaseManifest> {
        let version = normalize_version(version)?;
        let _guard = self.mutation_lock.lock().await;
        let mut manifest = self.load_manifest(channel, &version).await?;
        manifest.withdrawn = true;
        manifest.updated_at = OffsetDateTime::now_utc();
        self.save_manifest(channel, &version, &manifest).await?;
        Ok(manifest)
    }

    pub async fn copy_release(
        &self,
        source_channel: &str,
        target_channel: &str,
        version: &str,
    ) -> Result<ReleaseManifest> {
        validate_path_segment(source_channel, "source_channel")?;
        validate_path_segment(target_channel, "target_channel")?;
        let version = normalize_version(version)?;
        let _guard = self.mutation_lock.lock().await;
        let target = self.release_dir(target_channel, &version);
        if target.exists() {
            return Err(UdsError::Conflict(format!(
                "release {target_channel}/{version} already exists"
            )));
        }

        let mut manifest = self.load_manifest(source_channel, &version).await?;
        for artifact in manifest.platforms.values() {
            self.verify_blob(artifact).await?;
        }
        manifest.updated_at = OffsetDateTime::now_utc();
        let staging_dir = self
            .data_dir
            .join("staging/releases")
            .join(Uuid::new_v4().to_string());
        fs::create_dir_all(&staging_dir).await?;
        atomic_write_json(staging_dir.join("manifest.json"), &manifest).await?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::rename(staging_dir, target).await?;
        Ok(manifest)
    }

    pub async fn update_for(
        &self,
        channel: &str,
        target: &str,
        arch: &str,
        current_version: &str,
    ) -> Result<Option<TauriUpdateResponse>> {
        validate_path_segment(channel, "channel")?;
        validate_path_segment(target, "target")?;
        validate_path_segment(arch, "arch")?;
        let current = parse_version(current_version)?;
        let platform_key = format!("{target}-{arch}");
        let releases = self.list_releases(channel).await?;
        let mut candidates = Vec::new();
        for manifest in releases {
            if manifest.withdrawn || !manifest.platforms.contains_key(&platform_key) {
                continue;
            }
            let parsed = parse_version(&manifest.version)?;
            if parsed > current {
                candidates.push((parsed, manifest));
            }
        }
        candidates.sort_by(|left, right| left.0.cmp(&right.0));
        let Some((offered_version, offered_manifest)) = candidates.last() else {
            return Ok(None);
        };
        let artifact = offered_manifest
            .platforms
            .get(&platform_key)
            .ok_or_else(|| UdsError::Storage("selected release is missing its platform artifact".to_string()))?;

        let mut notes = String::new();
        for (_, manifest) in candidates
            .iter()
            .filter(|(version, _)| version <= offered_version)
        {
            if !notes.is_empty() {
                notes.push_str("\n\n");
            }
            notes.push_str("## ");
            notes.push_str(&manifest.version);
            notes.push_str("\n\n");
            notes.push_str(manifest.notes.trim());
        }

        let mut url = self.public_base_url.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| UdsError::Config("public_base_url cannot be used as a hierarchical URL".to_string()))?;
            segments.pop_if_empty();
            segments.extend([
                "api",
                "v1",
                "downloads",
                channel,
                &offered_manifest.version,
                &platform_key,
                &artifact.file_name,
            ]);
        }

        Ok(Some(TauriUpdateResponse {
            version: offered_manifest.version.clone(),
            url: url.to_string(),
            signature: artifact.signature.clone(),
            pub_date: offered_manifest.pub_date.clone(),
            notes,
        }))
    }

    pub async fn artifact_path(
        &self,
        channel: &str,
        version: &str,
        platform: &str,
        file_name: &str,
    ) -> Result<(PathBuf, u64)> {
        validate_path_segment(channel, "channel")?;
        validate_platform_key(platform)?;
        validate_path_segment(file_name, "file_name")?;
        let version = normalize_version(version)?;
        let manifest = self.load_manifest(channel, &version).await?;
        let artifact = manifest.platforms.get(platform).ok_or_else(|| {
            UdsError::NotFound(format!(
                "platform {platform} not found for release {channel}/{version}"
            ))
        })?;
        if artifact.file_name != file_name {
            return Err(UdsError::NotFound(format!(
                "artifact {file_name} not found"
            )));
        }
        self.verify_blob(artifact).await?;
        Ok((self.blob_data_path(&artifact.sha256), artifact.size))
    }

    pub async fn release_list(&self, channel: &str) -> Result<ReleaseListResponse> {
        let mut releases = self
            .list_releases(channel)
            .await?
            .into_iter()
            .map(|manifest| ReleaseListEntry {
                version: manifest.version,
                pub_date: manifest.pub_date,
                withdrawn: manifest.withdrawn,
                platforms: manifest.platforms.keys().cloned().collect(),
                updated_at: manifest.updated_at,
            })
            .collect::<Vec<_>>();
        releases.sort_by_key(|release| std::cmp::Reverse(parse_version(&release.version).ok()));
        Ok(ReleaseListResponse { releases })
    }

    pub async fn catalog(&self) -> Result<CatalogResponse> {
        let mut entries = Vec::new();
        let mut channels = fs::read_dir(self.data_dir.join("releases")).await?;
        while let Some(channel_entry) = channels.next_entry().await? {
            if !channel_entry.file_type().await?.is_dir() {
                continue;
            }
            let channel = channel_entry.file_name().to_string_lossy().to_string();
            for manifest in self.list_releases(&channel).await? {
                let manifest_bytes = serde_json::to_vec(&manifest)?;
                entries.push(CatalogEntry {
                    channel: channel.clone(),
                    version: manifest.version.clone(),
                    withdrawn: manifest.withdrawn,
                    manifest_sha256: sha256_hex(&manifest_bytes),
                });
            }
        }
        entries.sort_by(|left, right| {
            left.channel
                .cmp(&right.channel)
                .then(left.version.cmp(&right.version))
        });
        Ok(CatalogResponse { entries })
    }

    async fn ensure_layout(&self) -> Result<()> {
        for path in [
            "releases",
            "blobs/sha256",
            "staging/uploads",
            "staging/releases",
        ] {
            fs::create_dir_all(self.data_dir.join(path)).await?;
        }
        Ok(())
    }

    async fn cleanup_staging(&self) -> Result<()> {
        for path in ["staging/uploads", "staging/releases"] {
            let directory = self.data_dir.join(path);
            if directory.exists() {
                fs::remove_dir_all(&directory).await?;
            }
            fs::create_dir_all(directory).await?;
        }
        Ok(())
    }

    async fn ensure_no_legacy_releases(&self) -> Result<()> {
        let root = self.data_dir.join("releases");
        let mut channels = fs::read_dir(root).await?;
        while let Some(channel) = channels.next_entry().await? {
            if !channel.file_type().await?.is_dir() {
                continue;
            }
            let mut releases = fs::read_dir(channel.path()).await?;
            while let Some(release) = releases.next_entry().await? {
                if !release.file_type().await?.is_dir() {
                    continue;
                }
                let mut files = fs::read_dir(release.path()).await?;
                while let Some(file) = files.next_entry().await? {
                    if file.file_name() != "manifest.json" {
                        return Err(UdsError::Storage(format!(
                            "legacy release storage detected at '{}'; migration is intentionally unsupported",
                            release.path().display()
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    async fn publish_blob(&self, staged: &StagedArtifact) -> Result<()> {
        validate_sha256(&staged.sha256)?;
        let target_dir = self.blob_dir(&staged.sha256);
        if target_dir.exists() {
            self.verify_staged_against_existing(staged).await?;
            return Ok(());
        }
        let shard = target_dir.parent().expect("blob digest has shard parent");
        fs::create_dir_all(shard).await?;
        let temp_dir = shard.join(format!(".tmp-{}", Uuid::new_v4()));
        fs::create_dir(&temp_dir).await?;
        fs::rename(&staged.path, temp_dir.join("data")).await?;
        match fs::rename(&temp_dir, &target_dir).await {
            Ok(()) => Ok(()),
            Err(_error) if target_dir.exists() => {
                let _ = fs::remove_dir_all(&temp_dir).await;
                self.verify_staged_against_existing(staged).await
            }
            Err(error) => {
                let _ = fs::remove_dir_all(&temp_dir).await;
                Err(error.into())
            }
        }
    }

    async fn verify_staged_against_existing(&self, staged: &StagedArtifact) -> Result<()> {
        let path = self.blob_data_path(&staged.sha256);
        let metadata = fs::metadata(&path)
            .await
            .map_err(|error| UdsError::Storage(format!("blob {} is incomplete: {error}", staged.sha256)))?;
        if metadata.len() != staged.size || sha256_file(&path).await? != staged.sha256 {
            return Err(UdsError::Storage(format!(
                "blob {} failed integrity validation",
                staged.sha256
            )));
        }
        Ok(())
    }

    async fn verify_blob(&self, artifact: &PlatformArtifact) -> Result<()> {
        validate_sha256(&artifact.sha256)?;
        let path = self.blob_data_path(&artifact.sha256);
        let metadata = fs::metadata(&path).await.map_err(|error| {
            UdsError::Storage(format!(
                "artifact blob {} is unavailable: {error}",
                artifact.sha256
            ))
        })?;
        if !metadata.is_file() || metadata.len() != artifact.size {
            return Err(UdsError::Storage(format!(
                "artifact blob {} has an unexpected size",
                artifact.sha256
            )));
        }
        Ok(())
    }

    async fn prune_unreferenced_blobs(&self) -> Result<()> {
        let mut referenced = BTreeSet::new();
        let releases_root = self.data_dir.join("releases");
        let mut channels = fs::read_dir(&releases_root).await?;
        while let Some(channel) = channels.next_entry().await? {
            if !channel.file_type().await?.is_dir() {
                continue;
            }
            let name = channel.file_name().to_string_lossy().to_string();
            for manifest in self.list_releases(&name).await? {
                referenced.extend(
                    manifest
                        .platforms
                        .values()
                        .map(|artifact| artifact.sha256.clone()),
                );
            }
        }
        let blobs_root = self.data_dir.join("blobs/sha256");
        let mut shards = fs::read_dir(&blobs_root).await?;
        while let Some(shard) = shards.next_entry().await? {
            if !shard.file_type().await?.is_dir() {
                continue;
            }
            let mut digests = fs::read_dir(shard.path()).await?;
            while let Some(digest) = digests.next_entry().await? {
                let name = digest.file_name().to_string_lossy().to_string();
                if name.starts_with(".tmp-") || !referenced.contains(&name) {
                    fs::remove_dir_all(digest.path()).await?;
                }
            }
        }
        Ok(())
    }

    async fn list_releases(&self, channel: &str) -> Result<Vec<ReleaseManifest>> {
        validate_path_segment(channel, "channel")?;
        let channel_dir = self.data_dir.join("releases").join(channel);
        if !channel_dir.exists() {
            return Ok(Vec::new());
        }
        let mut releases = Vec::new();
        let mut entries = fs::read_dir(channel_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                let version = entry.file_name().to_string_lossy().to_string();
                releases.push(self.load_manifest(channel, &version).await?);
            }
        }
        Ok(releases)
    }

    async fn load_manifest(&self, channel: &str, version: &str) -> Result<ReleaseManifest> {
        validate_path_segment(channel, "channel")?;
        let version = normalize_version(version)?;
        let path = self.manifest_path(channel, &version);
        if !path.exists() {
            return Err(UdsError::NotFound(format!(
                "release {channel}/{version} not found"
            )));
        }
        Ok(serde_json::from_slice(&fs::read(path).await?)?)
    }

    async fn save_manifest(&self, channel: &str, version: &str, manifest: &ReleaseManifest) -> Result<()> {
        atomic_write_json(self.manifest_path(channel, version), manifest).await
    }

    fn release_dir(&self, channel: &str, version: &str) -> PathBuf {
        self.data_dir.join("releases").join(channel).join(version)
    }

    fn manifest_path(&self, channel: &str, version: &str) -> PathBuf {
        self.release_dir(channel, version).join("manifest.json")
    }

    fn blob_dir(&self, sha256: &str) -> PathBuf {
        self.data_dir
            .join("blobs/sha256")
            .join(&sha256[..2])
            .join(sha256)
    }

    fn blob_data_path(&self, sha256: &str) -> PathBuf {
        self.blob_dir(sha256).join("data")
    }
}

pub fn parse_version(version: &str) -> Result<Version> {
    let normalized = version.trim().trim_start_matches('v');
    Version::parse(normalized)
        .map_err(|error| UdsError::BadRequest(format!("invalid semantic version '{version}': {error}")))
}

fn normalize_version(version: &str) -> Result<String> {
    Ok(parse_version(version)?.to_string())
}

fn validate_upload_metadata(
    metadata: &ReleaseUploadMetadata,
    files: &BTreeMap<String, StagedArtifact>,
    policy: &UploadPolicy,
) -> Result<()> {
    let _ = normalize_version(&metadata.version)?;
    if metadata.platforms.is_empty() || metadata.platforms.len() > policy.max_platforms {
        return Err(UdsError::BadRequest(format!(
            "release must contain between 1 and {} platforms",
            policy.max_platforms
        )));
    }
    if let Some(pub_date) = &metadata.pub_date {
        OffsetDateTime::parse(pub_date, &Rfc3339)
            .map_err(|error| UdsError::BadRequest(format!("pub_date must be RFC 3339: {error}")))?;
    }
    let referenced = metadata
        .platforms
        .values()
        .map(|platform| platform.file_field.as_str())
        .collect::<BTreeSet<_>>();
    let supplied = files.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if referenced != supplied {
        return Err(UdsError::BadRequest(
            "multipart file fields must exactly match metadata references".to_string(),
        ));
    }
    for (platform_key, platform) in &metadata.platforms {
        validate_platform_key(platform_key)?;
        validate_path_segment(&platform.file_name, "file_name")?;
        if platform.signature.trim().is_empty() {
            return Err(UdsError::BadRequest(format!(
                "signature for platform {platform_key} must not be empty"
            )));
        }
    }
    let mut total = 0u64;
    for staged in files.values() {
        if staged.size > policy.max_artifact_bytes {
            return Err(UdsError::PayloadTooLarge(format!(
                "multipart file field '{}' exceeds the configured limit",
                staged.field_name
            )));
        }
        total = total.saturating_add(staged.size);
    }
    if total > policy.max_total_artifact_bytes {
        return Err(UdsError::PayloadTooLarge(
            "release artifacts exceed the configured total limit".to_string(),
        ));
    }
    Ok(())
}

fn validate_path_segment(value: &str, name: &str) -> Result<()> {
    if value.is_empty()
        || value.contains('/')
        || value.contains('\\')
        || value == "."
        || value == ".."
        || value.chars().any(char::is_control)
    {
        return Err(UdsError::BadRequest(format!(
            "{name} is not a safe path segment"
        )));
    }
    Ok(())
}

fn validate_platform_key(value: &str) -> Result<()> {
    validate_path_segment(value, "platform")?;
    if !value.contains('-') {
        return Err(UdsError::BadRequest(
            "platform must use the target-arch form".to_string(),
        ));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<()> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(UdsError::Storage(
            "manifest contains an invalid SHA-256 digest".to_string(),
        ));
    }
    Ok(())
}

fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(bytes.as_ref()))
}

async fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

async fn atomic_write_json(path: PathBuf, value: &impl serde::Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    tokio::task::spawn_blocking(move || -> Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| UdsError::Storage("atomic write path has no parent".to_string()))?;
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(&path)
            .map_err(|error| UdsError::Io(error.error))?;
        Ok(())
    })
    .await
    .map_err(|error| UdsError::Storage(format!("atomic writer task failed: {error}")))??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::UploadPlatformMetadata;

    fn policy() -> UploadPolicy {
        UploadPolicy {
            max_artifact_bytes: 1024,
            max_total_artifact_bytes: 4096,
            max_metadata_bytes: 1024,
            max_platforms: 8,
        }
    }

    async fn staged(root: &Path, field: &str, bytes: &[u8]) -> StagedArtifact {
        let path = root.join(field);
        fs::write(&path, bytes).await.unwrap();
        StagedArtifact {
            field_name: field.to_string(),
            path,
            size: bytes.len() as u64,
            sha256: sha256_hex(bytes),
        }
    }

    #[tokio::test]
    async fn copy_reuses_content_addressed_blob() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(
            temp.path().to_path_buf(),
            "https://updates.example.test".to_string(),
        )
        .await
        .unwrap();
        let artifact = staged(temp.path(), "bundle", b"bundle").await;
        let digest = artifact.sha256.clone();
        let metadata = ReleaseUploadMetadata {
            version: "26.7.2".to_string(),
            pub_date: Some("2026-07-06T18:35:11Z".to_string()),
            notes: "notes".to_string(),
            platforms: BTreeMap::from([(
                "linux-x86_64".to_string(),
                UploadPlatformMetadata {
                    file_field: "bundle".to_string(),
                    file_name: "studio.tar.gz".to_string(),
                    signature: "signature".to_string(),
                },
            )]),
        };
        storage
            .put_release(
                "beta",
                metadata,
                BTreeMap::from([("bundle".to_string(), artifact)]),
                &policy(),
            )
            .await
            .unwrap();
        storage
            .copy_release("beta", "stable", "26.7.2")
            .await
            .unwrap();
        assert!(storage.blob_data_path(&digest).is_file());
        assert_eq!(
            storage.release_list("stable").await.unwrap().releases.len(),
            1
        );
    }
}
