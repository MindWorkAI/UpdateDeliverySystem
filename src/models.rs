//! Wire and persistence models shared by UDS routes and storage.
//!
//! These types make the update protocol explicit and keep serialized data
//! independent from the services that read or write it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Metadata supplied by an administrator when publishing a release.
///
/// UDS combines this trusted description with the independently streamed
/// artifacts before it creates the persisted release manifest.
pub struct ReleaseUploadMetadata {
    pub version: String,

    #[serde(default)]
    pub pub_date: Option<String>,

    #[serde(default)]
    pub notes: String,

    pub platforms: BTreeMap<String, UploadPlatformMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Describes how one uploaded platform artifact maps to multipart form data.
pub struct UploadPlatformMetadata {
    pub file_field: String,
    pub file_name: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Durable description of a release stored in a UDS channel.
///
/// The manifest is the source of truth used for update checks, downloads,
/// catalog reconciliation, and fleet replication.
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
/// Compact release representation returned by the administrative list API.
pub struct ReleaseListEntry {
    pub version: String,

    #[serde(default)]
    pub pub_date: Option<String>,

    pub withdrawn: bool,
    pub platforms: Vec<String>,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Administrative response containing all releases visible in one channel.
pub struct ReleaseListResponse {
    pub releases: Vec<ReleaseListEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Immutable artifact metadata referenced by a release manifest.
///
/// The digest lets UDS use content-addressed storage and verify that files were
/// not corrupted between upload and download.
pub struct PlatformArtifact {
    pub file_name: String,
    pub signature: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Update response expected by the Tauri v2 updater.
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
/// Administrative request for replacing a release's human-readable notes.
pub struct ChangelogPatchRequest {
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Administrative request for promoting an existing release between channels.
pub struct CopyReleaseRequest {
    pub source_channel: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Digest-backed catalog item used to compare state between fleet nodes.
pub struct CatalogEntry {
    pub channel: String,
    pub version: String,
    pub withdrawn: bool,
    pub manifest_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Fleet catalog snapshot containing the releases known by one node.
pub struct CatalogResponse {
    pub entries: Vec<CatalogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Notification that a release mutation must be reconciled across the fleet.
pub struct ReplicationEvent {
    pub event_id: String,
    pub event_type: ReplicationEventType,
    pub channel: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Release mutations that can require replication to peer nodes.
pub enum ReplicationEventType {
    ReleaseUploaded,
    ChangelogPatched,
    ReleaseWithdrawn,
    ReleaseCopied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Result of an administrative mutation and its fleet replication attempt.
pub struct MutationResponse {
    pub channel: String,
    pub version: String,
    pub replicated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Server-enforced upload limits advertised to administration clients.
///
/// Publishing these values lets clients reject oversized releases before
/// spending bandwidth on an upload the server cannot accept.
pub struct UploadPolicy {
    pub max_artifact_bytes: u64,
    pub max_total_artifact_bytes: u64,
    pub max_metadata_bytes: u64,
    pub max_platforms: usize,
}
