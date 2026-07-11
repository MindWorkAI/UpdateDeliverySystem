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
    /// The version carried by this UDS data contract.
    pub version: String,

    /// The pub date carried by this UDS data contract.
    #[serde(default)]
    pub pub_date: Option<String>,

    /// The notes carried by this UDS data contract.
    #[serde(default)]
    pub notes: String,

    /// The platforms carried by this UDS data contract.
    pub platforms: BTreeMap<String, UploadPlatformMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Describes how one uploaded platform artifact maps to multipart form data.
pub struct UploadPlatformMetadata {
    /// The file field carried by this UDS data contract.
    pub file_field: String,

    /// The file name carried by this UDS data contract.
    pub file_name: String,

    /// The signature carried by this UDS data contract.
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Durable description of a release stored in a UDS channel.
///
/// The manifest is the source of truth used for update checks, downloads,
/// catalog reconciliation, and fleet replication.
pub struct ReleaseManifest {
    /// The version carried by this UDS data contract.
    pub version: String,

    /// The pub date carried by this UDS data contract.
    #[serde(default)]
    pub pub_date: Option<String>,

    /// The notes carried by this UDS data contract.
    #[serde(default)]
    pub notes: String,

    /// The withdrawn carried by this UDS data contract.
    #[serde(default)]
    pub withdrawn: bool,

    /// The platforms carried by this UDS data contract.
    pub platforms: BTreeMap<String, PlatformArtifact>,

    /// The updated at carried by this UDS data contract.
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Compact release representation returned by the administrative list API.
pub struct ReleaseListEntry {
    /// The version carried by this UDS data contract.
    pub version: String,

    /// The pub date carried by this UDS data contract.
    #[serde(default)]
    pub pub_date: Option<String>,

    /// The withdrawn carried by this UDS data contract.
    pub withdrawn: bool,

    /// The platforms carried by this UDS data contract.
    pub platforms: Vec<String>,

    /// The updated at carried by this UDS data contract.
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Administrative response containing all releases visible in one channel.
pub struct ReleaseListResponse {
    /// The releases carried by this UDS data contract.
    pub releases: Vec<ReleaseListEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Immutable artifact metadata referenced by a release manifest.
///
/// The digest lets UDS use content-addressed storage and verify that files were
/// not corrupted between upload and download.
pub struct PlatformArtifact {
    /// The file name carried by this UDS data contract.
    pub file_name: String,

    /// The signature carried by this UDS data contract.
    pub signature: String,

    /// The size carried by this UDS data contract.
    pub size: u64,

    /// The sha256 carried by this UDS data contract.
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Update response expected by the Tauri v2 updater.
pub struct TauriUpdateResponse {
    /// The version carried by this UDS data contract.
    pub version: String,

    /// The url carried by this UDS data contract.
    pub url: String,

    /// The signature carried by this UDS data contract.
    pub signature: String,

    /// The pub date carried by this UDS data contract.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pub_date: Option<String>,

    /// The notes carried by this UDS data contract.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Administrative request for replacing a release's human-readable notes.
pub struct ChangelogPatchRequest {
    /// The notes carried by this UDS data contract.
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Administrative request for promoting an existing release between channels.
pub struct CopyReleaseRequest {
    /// The source channel carried by this UDS data contract.
    pub source_channel: String,

    /// The version carried by this UDS data contract.
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Digest-backed catalog item used to compare state between fleet nodes.
pub struct CatalogEntry {
    /// The channel carried by this UDS data contract.
    pub channel: String,

    /// The version carried by this UDS data contract.
    pub version: String,

    /// The withdrawn carried by this UDS data contract.
    pub withdrawn: bool,

    /// The manifest sha256 carried by this UDS data contract.
    pub manifest_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Fleet catalog snapshot containing the releases known by one node.
pub struct CatalogResponse {
    /// The entries carried by this UDS data contract.
    pub entries: Vec<CatalogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Notification that a release mutation must be reconciled across the fleet.
pub struct ReplicationEvent {
    /// The event id carried by this UDS data contract.
    pub event_id: String,

    /// The event type carried by this UDS data contract.
    pub event_type: ReplicationEventType,

    /// The channel carried by this UDS data contract.
    pub channel: String,

    /// The version carried by this UDS data contract.
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Release mutations that can require replication to peer nodes.
pub enum ReplicationEventType {
    /// Represents the item case in UDS.
    ReleaseUploaded,

    /// Represents the item case in UDS.
    ChangelogPatched,

    /// Represents the item case in UDS.
    ReleaseWithdrawn,

    /// Represents the item case in UDS.
    ReleaseCopied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Result of an administrative mutation and its fleet replication attempt.
pub struct MutationResponse {
    /// The channel carried by this UDS data contract.
    pub channel: String,

    /// The version carried by this UDS data contract.
    pub version: String,

    /// The replicated carried by this UDS data contract.
    pub replicated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Server-enforced upload limits advertised to administration clients.
///
/// Publishing these values lets clients reject oversized releases before
/// spending bandwidth on an upload the server cannot accept.
pub struct UploadPolicy {
    /// The max artifact bytes carried by this UDS data contract.
    pub max_artifact_bytes: u64,

    /// The max total artifact bytes carried by this UDS data contract.
    pub max_total_artifact_bytes: u64,

    /// The max metadata bytes carried by this UDS data contract.
    pub max_metadata_bytes: u64,

    /// The max platforms carried by this UDS data contract.
    pub max_platforms: usize,
}
