use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseUploadMetadata {
    pub version: String,

    #[serde(default)]
    pub pub_date: Option<String>,

    #[serde(default)]
    pub notes: String,

    pub platforms: BTreeMap<String, UploadPlatformMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadPlatformMetadata {
    pub file_field: String,
    pub file_name: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseManifest {
    pub version: String,

    #[serde(default)]
    pub pub_date: Option<String>,

    #[serde(default)]
    pub notes: String,

    #[serde(default)]
    pub withdrawn: bool,

    pub platforms: BTreeMap<String, PlatformArtifact>,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformArtifact {
    pub file_name: String,
    pub signature: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TauriUpdateResponse {
    pub version: String,
    pub url: String,
    pub signature: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub pub_date: Option<String>,

    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangelogPatchRequest {
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyReleaseRequest {
    pub source_channel: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub channel: String,
    pub version: String,
    pub withdrawn: bool,
    pub manifest_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogResponse {
    pub entries: Vec<CatalogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationEvent {
    pub event_id: String,
    pub event_type: ReplicationEventType,
    pub channel: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplicationEventType {
    ReleaseUploaded,
    ChangelogPatched,
    ReleaseWithdrawn,
    ReleaseCopied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationResponse {
    pub channel: String,
    pub version: String,
    pub replicated: bool,
}
