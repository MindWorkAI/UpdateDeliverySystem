use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bytes::Bytes;
use semver::Version;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tokio::fs;

use crate::errors::{Result, UdsError};
use crate::models::{
    CatalogEntry, CatalogResponse, PlatformArtifact, ReleaseListEntry, ReleaseListResponse, ReleaseManifest, ReleaseUploadMetadata,
    TauriUpdateResponse,
};

#[derive(Debug, Clone)]
pub struct Storage {
    data_dir: PathBuf,
    public_base_url: String,
}

impl Storage {
    pub async fn new(data_dir: PathBuf, public_base_url: String) -> Result<Self> {
        let storage = Self { data_dir, public_base_url };
        storage.ensure_layout().await?;
        Ok(storage)
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub async fn put_release(
        &self,
        channel: &str,
        metadata: ReleaseUploadMetadata,
        uploaded_files: BTreeMap<String, Bytes>,
    ) -> Result<ReleaseManifest> {
        validate_path_segment(channel, "channel")?;
        let version = normalize_version(&metadata.version)?;
        let release_dir = self.release_dir(channel, &version);

        if release_dir.exists() {
            return Err(UdsError::Conflict(format!("release {channel}/{version} already exists")));
        }

        fs::create_dir_all(&release_dir).await?;

        let mut platforms = BTreeMap::new();
        for (platform_key, upload_platform) in metadata.platforms {
            validate_platform_key(&platform_key)?;
            let bytes = uploaded_files.get(&upload_platform.file_field).ok_or_else(|| {
                UdsError::BadRequest(format!("missing multipart file field '{}'", upload_platform.file_field))
            })?;

            validate_path_segment(&upload_platform.file_name, "file_name")?;
            let artifact_path = release_dir.join(&upload_platform.file_name);
            fs::write(&artifact_path, bytes).await?;

            platforms.insert(
                platform_key,
                PlatformArtifact {
                    file_name: upload_platform.file_name,
                    signature: upload_platform.signature,
                    size: bytes.len() as u64,
                    sha256: sha256_hex(bytes),
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

        self.save_manifest(channel, &version, &manifest).await?;
        Ok(manifest)
    }

    pub async fn patch_changelog(&self, channel: &str, version: &str, notes: String) -> Result<ReleaseManifest> {
        let version = normalize_version(version)?;
        let mut manifest = self.load_manifest(channel, &version).await?;
        manifest.notes = notes;
        manifest.updated_at = OffsetDateTime::now_utc();
        self.save_manifest(channel, &version, &manifest).await?;
        Ok(manifest)
    }

    pub async fn withdraw_release(&self, channel: &str, version: &str) -> Result<ReleaseManifest> {
        let version = normalize_version(version)?;
        let mut manifest = self.load_manifest(channel, &version).await?;
        manifest.withdrawn = true;
        manifest.updated_at = OffsetDateTime::now_utc();
        self.save_manifest(channel, &version, &manifest).await?;
        Ok(manifest)
    }

    pub async fn copy_release(&self, source_channel: &str, target_channel: &str, version: &str) -> Result<ReleaseManifest> {
        validate_path_segment(source_channel, "source_channel")?;
        validate_path_segment(target_channel, "target_channel")?;
        let version = normalize_version(version)?;
        let source = self.release_dir(source_channel, &version);
        let target = self.release_dir(target_channel, &version);

        if !source.exists() {
            return Err(UdsError::NotFound(format!("release {source_channel}/{version} not found")));
        }
        if target.exists() {
            return Err(UdsError::Conflict(format!("release {target_channel}/{version} already exists")));
        }

        copy_dir_all(&source, &target).await?;
        let mut manifest = self.load_manifest(target_channel, &version).await?;
        manifest.updated_at = OffsetDateTime::now_utc();
        self.save_manifest(target_channel, &version, &manifest).await?;
        Ok(manifest)
    }

    pub async fn update_for(&self, channel: &str, target: &str, arch: &str, current_version: &str) -> Result<Option<TauriUpdateResponse>> {
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
        for (_, manifest) in candidates.iter().filter(|(version, _)| version <= offered_version) {
            if !notes.is_empty() {
                notes.push_str("\n\n");
            }
            notes.push_str("## ");
            notes.push_str(&manifest.version);
            notes.push_str("\n\n");
            notes.push_str(manifest.notes.trim());
        }

        let url = format!(
            "{}/api/v1/downloads/{}/{}/{}/{}",
            self.public_base_url.trim_end_matches('/'),
            channel,
            offered_manifest.version,
            platform_key,
            artifact.file_name
        );

        Ok(Some(TauriUpdateResponse {
            version: offered_manifest.version.clone(),
            url,
            signature: artifact.signature.clone(),
            pub_date: offered_manifest.pub_date.clone(),
            notes,
        }))
    }

    pub async fn artifact_path(&self, channel: &str, version: &str, platform: &str, file_name: &str) -> Result<PathBuf> {
        validate_path_segment(channel, "channel")?;
        validate_platform_key(platform)?;
        validate_path_segment(file_name, "file_name")?;
        let version = normalize_version(version)?;
        let manifest = self.load_manifest(channel, &version).await?;
        let artifact = manifest
            .platforms
            .get(platform)
            .ok_or_else(|| UdsError::NotFound(format!("platform {platform} not found for release {channel}/{version}")))?;
        if artifact.file_name != file_name {
            return Err(UdsError::NotFound(format!("artifact {file_name} not found")));
        }
        Ok(self.release_dir(channel, &version).join(file_name))
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

        releases.sort_by(|left, right| {
            let left_version = parse_version(&left.version);
            let right_version = parse_version(&right.version);
            match (left_version, right_version) {
                (Ok(left_version), Ok(right_version)) => right_version.cmp(&left_version),
                _ => right.version.cmp(&left.version),
            }
        });

        Ok(ReleaseListResponse { releases })
    }

    pub async fn catalog(&self) -> Result<CatalogResponse> {
        let mut entries = Vec::new();
        let releases_root = self.data_dir.join("releases");
        if !releases_root.exists() {
            return Ok(CatalogResponse { entries });
        }

        let mut channels = fs::read_dir(releases_root).await?;
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

        entries.sort_by(|left, right| left.channel.cmp(&right.channel).then(left.version.cmp(&right.version)));
        Ok(CatalogResponse { entries })
    }

    async fn ensure_layout(&self) -> Result<()> {
        fs::create_dir_all(self.data_dir.join("releases")).await?;
        fs::create_dir_all(self.data_dir.join("stats/raw")).await?;
        fs::create_dir_all(self.data_dir.join("stats/rollups")).await?;
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
            return Err(UdsError::NotFound(format!("release {channel}/{version} not found")));
        }
        let text = fs::read_to_string(path).await?;
        Ok(serde_json::from_str(&text)?)
    }

    async fn save_manifest(&self, channel: &str, version: &str, manifest: &ReleaseManifest) -> Result<()> {
        let release_dir = self.release_dir(channel, version);
        fs::create_dir_all(&release_dir).await?;
        let tmp_path = release_dir.join("manifest.json.tmp");
        let final_path = release_dir.join("manifest.json");
        let bytes = serde_json::to_vec_pretty(manifest)?;
        fs::write(&tmp_path, bytes).await?;
        fs::rename(&tmp_path, &final_path).await?;
        Ok(())
    }

    fn release_dir(&self, channel: &str, version: &str) -> PathBuf {
        self.data_dir.join("releases").join(channel).join(version)
    }

    fn manifest_path(&self, channel: &str, version: &str) -> PathBuf {
        self.release_dir(channel, version).join("manifest.json")
    }
}

pub fn parse_version(version: &str) -> Result<Version> {
    let normalized = version.trim().trim_start_matches('v');
    Ok(Version::parse(normalized)?)
}

fn normalize_version(version: &str) -> Result<String> {
    let parsed = parse_version(version)?;
    Ok(parsed.to_string())
}

fn validate_path_segment(value: &str, name: &str) -> Result<()> {
    if value.is_empty() || value.contains('/') || value.contains('\\') || value == "." || value == ".." {
        return Err(UdsError::BadRequest(format!("{name} is not a safe path segment")));
    }
    Ok(())
}

fn validate_platform_key(value: &str) -> Result<()> {
    validate_path_segment(value, "platform")?;
    if !value.contains('-') {
        return Err(UdsError::BadRequest("platform must use the target-arch form".to_string()));
    }
    Ok(())
}

fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    let digest = Sha256::digest(bytes.as_ref());
    hex::encode(digest)
}

async fn copy_dir_all(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target).await?;
    let mut entries = fs::read_dir(source).await?;
    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        let target_path = target.join(entry.file_name());
        if file_type.is_dir() {
            Box::pin(copy_dir_all(&entry.path(), &target_path)).await?;
        } else {
            fs::copy(entry.path(), target_path).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::UploadPlatformMetadata;

    #[tokio::test]
    async fn update_notes_include_all_versions_after_current_version() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().to_path_buf(), "https://updates.example.test".to_string()).await.unwrap();

        for version in ["26.6.0", "26.7.0", "26.7.2"] {
            let mut platforms = BTreeMap::new();
            platforms.insert(
                "windows-x86_64".to_string(),
                UploadPlatformMetadata {
                    file_field: "bundle".to_string(),
                    file_name: format!("ai-studio-{version}.zip"),
                    signature: format!("signature-{version}"),
                },
            );
            let mut files = BTreeMap::new();
            files.insert("bundle".to_string(), Bytes::from_static(b"bundle"));
            storage
                .put_release(
                    "stable",
                    ReleaseUploadMetadata {
                        version: version.to_string(),
                        pub_date: None,
                        notes: format!("Changed in {version}"),
                        platforms,
                    },
                    files,
                )
                .await
                .unwrap();
        }

        let update = storage.update_for("stable", "windows", "x86_64", "26.5.5").await.unwrap().unwrap();
        assert_eq!(update.version, "26.7.2");
        assert!(update.notes.contains("Changed in 26.6.0"));
        assert!(update.notes.contains("Changed in 26.7.0"));
        assert!(update.notes.contains("Changed in 26.7.2"));
    }

    #[tokio::test]
    async fn release_list_is_sorted_newest_first() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::new(temp.path().to_path_buf(), "https://updates.example.test".to_string()).await.unwrap();

        for version in ["26.6.0", "26.7.2", "26.7.0"] {
            let mut platforms = BTreeMap::new();
            platforms.insert(
                "linux-x86_64".to_string(),
                UploadPlatformMetadata {
                    file_field: "bundle".to_string(),
                    file_name: format!("ai-studio-{version}.tar.gz"),
                    signature: format!("signature-{version}"),
                },
            );
            let mut files = BTreeMap::new();
            files.insert("bundle".to_string(), Bytes::from_static(b"bundle"));
            storage
                .put_release(
                    "stable",
                    ReleaseUploadMetadata {
                        version: version.to_string(),
                        pub_date: None,
                        notes: String::new(),
                        platforms,
                    },
                    files,
                )
                .await
                .unwrap();
        }

        let response = storage.release_list("stable").await.unwrap();
        let versions = response.releases.into_iter().map(|release| release.version).collect::<Vec<_>>();
        assert_eq!(versions, vec!["26.7.2", "26.7.0", "26.6.0"]);
    }
}
