use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

use crate::errors::{Result, UdsError};
use crate::models::{ReleaseUploadMetadata, UploadPlatformMetadata, UploadPolicy};

#[derive(Debug, Deserialize)]
pub struct TauriStaticRelease {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub pub_date: Option<String>,
    pub platforms: BTreeMap<String, TauriStaticPlatform>,
}

#[derive(Debug, Deserialize)]
pub struct TauriStaticPlatform {
    pub url: String,
    pub signature: String,
}

#[derive(Debug)]
pub struct PreparedUpload {
    pub metadata: ReleaseUploadMetadata,
    pub artifacts: Vec<PreparedArtifact>,
    _temp_dir: Option<tempfile::TempDir>,
}

#[derive(Debug, Clone)]
pub struct PreparedArtifact {
    pub field_name: String,
    pub platform: String,
    pub file_name: String,
    pub source_url: String,
    pub path: PathBuf,
    pub size: u64,
    pub sha256: String,
}

pub async fn prepare_from_remote(input_url: &str, policy: &UploadPolicy) -> Result<PreparedUpload> {
    let client = Client::builder()
        .user_agent("uds-client")
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| UdsError::Config(format!("failed to create HTTP client: {error}")))?;
    let latest_json_url = normalize_github_release_url(input_url)?;
    let release =
        fetch_release_metadata(&client, latest_json_url.clone(), policy.max_metadata_bytes).await?;
    validate_platform_count(&release, policy)?;
    let temp_dir = tempfile::Builder::new()
        .prefix("uds-client-upload-")
        .tempdir()?;
    let mut artifacts = Vec::new();
    let mut platforms = BTreeMap::new();
    let mut total_size = 0u64;

    for (index, (platform, platform_release)) in release.platforms.iter().enumerate() {
        let artifact_url = Url::parse(&platform_release.url)
            .or_else(|_| latest_json_url.join(&platform_release.url))
            .map_err(|error| {
                UdsError::BadRequest(format!("invalid artifact URL for {platform}: {error}"))
            })?;
        let file_name = artifact_file_name(&artifact_url)?;
        let field_name = format!("artifact_{index}");
        let path = temp_dir.path().join(&field_name);
        let (size, sha256) = download_artifact(
            &client,
            artifact_url.clone(),
            &path,
            policy.max_artifact_bytes,
        )
        .await?;
        total_size = total_size.saturating_add(size);
        if total_size > policy.max_total_artifact_bytes {
            return Err(UdsError::PayloadTooLarge(
                "release artifacts exceed the server's total upload limit".to_string(),
            ));
        }
        platforms.insert(
            platform.clone(),
            UploadPlatformMetadata {
                file_field: field_name.clone(),
                file_name: file_name.clone(),
                signature: platform_release.signature.clone(),
            },
        );
        artifacts.push(PreparedArtifact {
            field_name,
            platform: platform.clone(),
            file_name,
            source_url: artifact_url.to_string(),
            path,
            size,
            sha256,
        });
    }

    let metadata = ReleaseUploadMetadata {
        version: release.version,
        pub_date: release.pub_date,
        notes: release.notes,
        platforms,
    };
    validate_serialized_metadata(&metadata, policy)?;
    Ok(PreparedUpload {
        metadata,
        artifacts,
        _temp_dir: Some(temp_dir),
    })
}

pub async fn prepare_from_local(
    latest_json_path: &Path,
    artifact_dir: &Path,
    policy: &UploadPolicy,
) -> Result<PreparedUpload> {
    let metadata_size = fs::metadata(latest_json_path).await?.len();
    if metadata_size > policy.max_metadata_bytes {
        return Err(UdsError::PayloadTooLarge(
            "latest.json exceeds the server's metadata limit".to_string(),
        ));
    }
    let text = fs::read_to_string(latest_json_path).await?;
    let release = serde_json::from_str::<TauriStaticRelease>(&text).map_err(|error| {
        UdsError::BadRequest(format!(
            "latest.json is not a Tauri updater JSON file: {error}"
        ))
    })?;
    validate_platform_count(&release, policy)?;
    let mut artifacts = Vec::new();
    let mut platforms = BTreeMap::new();
    let mut total_size = 0u64;
    for (index, (platform, platform_release)) in release.platforms.iter().enumerate() {
        let file_name = Url::parse(&platform_release.url)
            .ok()
            .and_then(|url| artifact_file_name(&url).ok())
            .or_else(|| {
                Path::new(&platform_release.url)
                    .file_name()
                    .map(|value| value.to_string_lossy().to_string())
            })
            .ok_or_else(|| {
                UdsError::BadRequest(format!(
                    "could not determine artifact file name for {platform}"
                ))
            })?;
        let path = artifact_dir.join(&file_name);
        let (size, sha256) = hash_local_artifact(&path, policy.max_artifact_bytes).await?;
        total_size = total_size.saturating_add(size);
        if total_size > policy.max_total_artifact_bytes {
            return Err(UdsError::PayloadTooLarge(
                "release artifacts exceed the server's total upload limit".to_string(),
            ));
        }
        let field_name = format!("artifact_{index}");
        platforms.insert(
            platform.clone(),
            UploadPlatformMetadata {
                file_field: field_name.clone(),
                file_name: file_name.clone(),
                signature: platform_release.signature.clone(),
            },
        );
        artifacts.push(PreparedArtifact {
            field_name,
            platform: platform.clone(),
            file_name,
            source_url: path.display().to_string(),
            path,
            size,
            sha256,
        });
    }
    let metadata = ReleaseUploadMetadata {
        version: release.version,
        pub_date: release.pub_date,
        notes: release.notes,
        platforms,
    };
    validate_serialized_metadata(&metadata, policy)?;
    Ok(PreparedUpload {
        metadata,
        artifacts,
        _temp_dir: None,
    })
}

async fn fetch_release_metadata(
    client: &Client,
    url: Url,
    limit: u64,
) -> Result<TauriStaticRelease> {
    let response = client
        .get(url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|error| UdsError::Storage(format!("failed to fetch latest.json: {error}")))?
        .error_for_status()
        .map_err(|error| UdsError::Storage(format!("failed to fetch latest.json: {error}")))?;
    if response
        .content_length()
        .is_some_and(|length| length > limit)
    {
        return Err(UdsError::PayloadTooLarge(
            "latest.json exceeds the server's metadata limit".to_string(),
        ));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|error| UdsError::Storage(format!("failed to read latest.json: {error}")))?;
        if bytes.len().saturating_add(chunk.len()) as u64 > limit {
            return Err(UdsError::PayloadTooLarge(
                "latest.json exceeds the server's metadata limit".to_string(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        UdsError::BadRequest(format!(
            "latest.json is not a Tauri updater JSON file: {error}"
        ))
    })
}

async fn download_artifact(
    client: &Client,
    url: Url,
    path: &Path,
    limit: u64,
) -> Result<(u64, String)> {
    let response = client
        .get(url)
        .timeout(Duration::from_secs(30 * 60))
        .send()
        .await
        .map_err(|error| UdsError::Storage(format!("failed to download artifact: {error}")))?
        .error_for_status()
        .map_err(|error| UdsError::Storage(format!("failed to download artifact: {error}")))?;
    if response
        .content_length()
        .is_some_and(|length| length > limit)
    {
        return Err(UdsError::PayloadTooLarge(
            "remote artifact exceeds the server's per-artifact limit".to_string(),
        ));
    }
    let mut file = fs::File::create(path).await?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|error| UdsError::Storage(format!("failed to read artifact: {error}")))?;
        size = size.saturating_add(chunk.len() as u64);
        if size > limit {
            return Err(UdsError::PayloadTooLarge(
                "remote artifact exceeds the server's per-artifact limit".to_string(),
            ));
        }
        file.write_all(&chunk).await?;
        hasher.update(&chunk);
    }
    file.flush().await?;
    Ok((size, hex::encode(hasher.finalize())))
}

async fn hash_local_artifact(path: &Path, limit: u64) -> Result<(u64, String)> {
    let size = fs::metadata(path).await?.len();
    if size > limit {
        return Err(UdsError::PayloadTooLarge(format!(
            "local artifact '{}' exceeds the server's per-artifact limit",
            path.display()
        )));
    }
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
    Ok((size, hex::encode(hasher.finalize())))
}

fn validate_platform_count(release: &TauriStaticRelease, policy: &UploadPolicy) -> Result<()> {
    if release.platforms.is_empty() || release.platforms.len() > policy.max_platforms {
        return Err(UdsError::BadRequest(format!(
            "release must contain between 1 and {} platforms",
            policy.max_platforms
        )));
    }
    Ok(())
}

fn validate_serialized_metadata(
    metadata: &ReleaseUploadMetadata,
    policy: &UploadPolicy,
) -> Result<()> {
    if serde_json::to_vec(metadata)?.len() as u64 > policy.max_metadata_bytes {
        return Err(UdsError::PayloadTooLarge(
            "release metadata exceeds the server's metadata limit".to_string(),
        ));
    }
    Ok(())
}

pub fn normalize_github_release_url(input: &str) -> Result<Url> {
    let url =
        Url::parse(input).map_err(|error| UdsError::BadRequest(format!("invalid URL: {error}")))?;
    if url.path().ends_with("/latest.json") {
        return Ok(url);
    }
    let segments = url
        .path_segments()
        .map(|segments| segments.collect::<Vec<_>>())
        .unwrap_or_default();
    if url.domain() == Some("github.com") && segments.len() >= 4 && segments[2] == "releases" {
        let owner = segments[0];
        let repo = segments[1];
        let release_selector = match segments[3] {
            "latest" => "latest".to_string(),
            "tag" if segments.len() >= 5 => format!("download/{}/latest.json", segments[4]),
            "download" if segments.len() >= 6 => return Ok(url),
            _ => "latest".to_string(),
        };
        let normalized = if release_selector == "latest" {
            format!("https://github.com/{owner}/{repo}/releases/latest/download/latest.json")
        } else {
            format!("https://github.com/{owner}/{repo}/releases/{release_selector}")
        };
        return Url::parse(&normalized).map_err(|error| {
            UdsError::BadRequest(format!("invalid normalized GitHub URL: {error}"))
        });
    }
    Ok(url)
}

fn artifact_file_name(url: &Url) -> Result<String> {
    url.path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            UdsError::BadRequest(format!("could not determine artifact file name from {url}"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_github_latest_release_url() {
        let url =
            normalize_github_release_url("https://github.com/MindWorkAI/AI-Studio/releases/latest")
                .unwrap();
        assert_eq!(
            url.as_str(),
            "https://github.com/MindWorkAI/AI-Studio/releases/latest/download/latest.json"
        );
    }

    #[test]
    fn normalizes_github_tag_url() {
        let url = normalize_github_release_url(
            "https://github.com/MindWorkAI/AI-Studio/releases/tag/v26.7.2",
        )
        .unwrap();
        assert_eq!(
            url.as_str(),
            "https://github.com/MindWorkAI/AI-Studio/releases/download/v26.7.2/latest.json"
        );
    }
}
