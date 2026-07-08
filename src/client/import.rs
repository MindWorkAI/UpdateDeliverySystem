use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use reqwest::Client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::fs;
use url::Url;

use crate::errors::{Result, UdsError};
use crate::models::{ReleaseUploadMetadata, UploadPlatformMetadata};

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

#[derive(Debug, Clone)]
pub struct PreparedUpload {
    pub metadata: ReleaseUploadMetadata,
    pub artifacts: Vec<PreparedArtifact>,
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

pub async fn prepare_from_remote(input_url: &str) -> Result<PreparedUpload> {
    let client = Client::builder()
        .user_agent("uds-client")
        .build()
        .map_err(|error| UdsError::Config(format!("failed to create HTTP client: {error}")))?;
    let latest_json_url = normalize_github_release_url(input_url)?;
    let release = client
        .get(latest_json_url.clone())
        .send()
        .await
        .map_err(|error| UdsError::Storage(format!("failed to fetch latest.json: {error}")))?
        .error_for_status()
        .map_err(|error| UdsError::Storage(format!("failed to fetch latest.json: {error}")))?
        .json::<TauriStaticRelease>()
        .await
        .map_err(|error| UdsError::BadRequest(format!("latest.json is not a Tauri updater JSON file: {error}")))?;

    let temp_dir = std::env::temp_dir().join(format!("uds-client-upload-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir).await?;

    let mut artifacts = Vec::new();
    let mut platforms = BTreeMap::new();

    for (index, (platform, platform_release)) in release.platforms.iter().enumerate() {
        let artifact_url = Url::parse(&platform_release.url)
            .or_else(|_| latest_json_url.join(&platform_release.url))
            .map_err(|error| UdsError::BadRequest(format!("invalid artifact URL for {platform}: {error}")))?;
        let file_name = artifact_file_name(&artifact_url)?;
        let field_name = format!("artifact_{index}");
        let bytes = client
            .get(artifact_url.clone())
            .send()
            .await
            .map_err(|error| UdsError::Storage(format!("failed to download artifact for {platform}: {error}")))?
            .error_for_status()
            .map_err(|error| UdsError::Storage(format!("failed to download artifact for {platform}: {error}")))?
            .bytes()
            .await
            .map_err(|error| UdsError::Storage(format!("failed to read artifact for {platform}: {error}")))?;
        let path = temp_dir.join(&file_name);
        fs::write(&path, &bytes).await?;
        let sha256 = hex::encode(Sha256::digest(&bytes));

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
            size: bytes.len() as u64,
            sha256,
        });
    }

    Ok(PreparedUpload {
        metadata: ReleaseUploadMetadata {
            version: release.version,
            pub_date: release.pub_date,
            notes: release.notes,
            platforms,
        },
        artifacts,
    })
}

pub async fn prepare_from_local(latest_json_path: &Path, artifact_dir: &Path) -> Result<PreparedUpload> {
    let text = fs::read_to_string(latest_json_path).await?;
    let release = serde_json::from_str::<TauriStaticRelease>(&text)
        .map_err(|error| UdsError::BadRequest(format!("latest.json is not a Tauri updater JSON file: {error}")))?;

    let mut artifacts = Vec::new();
    let mut platforms = BTreeMap::new();
    for (index, (platform, platform_release)) in release.platforms.iter().enumerate() {
        let file_name = Url::parse(&platform_release.url)
            .ok()
            .and_then(|url| artifact_file_name(&url).ok())
            .or_else(|| Path::new(&platform_release.url).file_name().map(|value| value.to_string_lossy().to_string()))
            .ok_or_else(|| UdsError::BadRequest(format!("could not determine artifact file name for {platform}")))?;
        let path = artifact_dir.join(&file_name);
        let bytes = fs::read(&path).await?;
        let field_name = format!("artifact_{index}");
        let sha256 = hex::encode(Sha256::digest(&bytes));

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
            size: bytes.len() as u64,
            sha256,
        });
    }

    Ok(PreparedUpload {
        metadata: ReleaseUploadMetadata {
            version: release.version,
            pub_date: release.pub_date,
            notes: release.notes,
            platforms,
        },
        artifacts,
    })
}

pub fn normalize_github_release_url(input: &str) -> Result<Url> {
    let url = Url::parse(input).map_err(|error| UdsError::BadRequest(format!("invalid URL: {error}")))?;
    if url.path().ends_with("/latest.json") {
        return Ok(url);
    }

    let segments = url.path_segments().map(|segments| segments.collect::<Vec<_>>()).unwrap_or_default();
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
        return Url::parse(&normalized).map_err(|error| UdsError::BadRequest(format!("invalid normalized GitHub URL: {error}")));
    }

    Ok(url)
}

fn artifact_file_name(url: &Url) -> Result<String> {
    url.path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .ok_or_else(|| UdsError::BadRequest(format!("could not determine artifact file name from {url}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_github_latest_release_url() {
        let url = normalize_github_release_url("https://github.com/MindWorkAI/AI-Studio/releases/latest").unwrap();
        assert_eq!(url.as_str(), "https://github.com/MindWorkAI/AI-Studio/releases/latest/download/latest.json");
    }

    #[test]
    fn normalizes_github_tag_url() {
        let url = normalize_github_release_url("https://github.com/MindWorkAI/AI-Studio/releases/tag/v26.7.2").unwrap();
        assert_eq!(url.as_str(), "https://github.com/MindWorkAI/AI-Studio/releases/download/v26.7.2/latest.json");
    }
}
