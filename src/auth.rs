//! Owner and administrator token lifecycle and durable token metadata.
//!
//! UDS stores only token verifiers so a copied data directory cannot reveal
//! credentials that grant administrative access.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use time::OffsetDateTime;
use tokio::fs;
use tokio::sync::RwLock;
use uuid::Uuid;
use zeroize::Zeroize;

use crate::errors::{Result, UdsError};

/// Defines the OWNER PREFIX value exposed by UDS.
pub const OWNER_PREFIX: &str = "uds_owner_v1_";

/// Defines the ADMIN PREFIX value exposed by UDS.
pub const ADMIN_PREFIX: &str = "uds_admin_v1_";

/// Defines the VERIFIER PREFIX value used by UDS.
const VERIFIER_PREFIX: &str = "sha512:";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
/// Kind of authenticated administrator represented in audit events.
pub enum ActorType {
    /// Represents the item case in UDS.
    Owner,

    /// Represents the item case in UDS.
    Admin,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// Authenticated caller identity propagated through protected handlers.
pub struct ActorIdentity {
    /// The actor type carried by this UDS data contract.
    pub actor_type: ActorType,

    /// The token id carried by this UDS data contract.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_id: Option<Uuid>,

    /// The token name carried by this UDS data contract.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_name: Option<String>,
}

impl ActorIdentity {
    /// Provides the owner operation used by UDS callers.
    pub fn owner() -> Self {
        Self {
            actor_type: ActorType::Owner,
            token_id: None,
            token_name: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// Immutable record of one administrator token status change.
pub struct StatusHistoryEntry {
    /// The enabled carried by this UDS data contract.
    pub enabled: bool,

    /// The changed at carried by this UDS data contract.
    #[serde(with = "time::serde::rfc3339")]
    pub changed_at: OffsetDateTime,

    /// The reason carried by this UDS data contract.
    pub reason: String,

    /// The mutation id carried by this UDS data contract.
    pub mutation_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Persisted admin-token record replicated between UDS fleet nodes.
pub struct AdminTokenRecord {
    /// The id carried by this UDS data contract.
    pub id: Uuid,

    /// The verifier carried by this UDS data contract.
    pub verifier: String,

    /// The name carried by this UDS data contract.
    pub name: String,

    /// The created at carried by this UDS data contract.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,

    /// The creation reason carried by this UDS data contract.
    pub creation_reason: String,

    /// The enabled carried by this UDS data contract.
    pub enabled: bool,

    /// The status history carried by this UDS data contract.
    pub status_history: Vec<StatusHistoryEntry>,

    /// The revision carried by this UDS data contract.
    pub revision: u64,

    /// The last mutation id carried by this UDS data contract.
    pub last_mutation_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Safe token metadata returned by APIs without the stored verifier.
pub struct AdminTokenMetadata {
    /// The id carried by this UDS data contract.
    pub id: Uuid,

    /// The name carried by this UDS data contract.
    pub name: String,

    /// The created at carried by this UDS data contract.
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,

    /// The creation reason carried by this UDS data contract.
    pub creation_reason: String,

    /// The enabled carried by this UDS data contract.
    pub enabled: bool,

    /// The status history carried by this UDS data contract.
    pub status_history: Vec<StatusHistoryEntry>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]

    /// The disabled at carried by this UDS data contract.
    pub disabled_at: Option<OffsetDateTime>,

    /// The disabled reason carried by this UDS data contract.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
}

/// A successful response returned when a new admin token is created. Keeping the
/// metadata nested avoids field-name collisions as the response evolves.
#[derive(Serialize, Deserialize)]
pub struct CreatedAdminToken {
    /// The metadata carried by this UDS data contract.
    pub metadata: AdminTokenMetadata,

    /// The token carried by this UDS data contract.
    pub token: String,
}

impl Drop for CreatedAdminToken {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}

impl From<&AdminTokenRecord> for AdminTokenMetadata {
    fn from(value: &AdminTokenRecord) -> Self {
        let disabled = value
            .status_history
            .iter()
            .rev()
            .find(|entry| !entry.enabled);
        Self {
            id: value.id,
            name: value.name.clone(),
            created_at: value.created_at,
            creation_reason: value.creation_reason.clone(),
            enabled: value.enabled,
            status_history: value.status_history.clone(),
            disabled_at: disabled.map(|entry| entry.changed_at),
            disabled_reason: disabled.map(|entry| entry.reason.clone()),
        }
    }
}

#[derive(Debug, Clone)]
/// Concurrent token repository backed by an atomically replaced JSON file.
pub struct AdminTokenStore {
    /// Stores the path value used by this UDS component.
    path: PathBuf,

    /// Stores the records value used by this UDS component.
    records: Arc<RwLock<Vec<AdminTokenRecord>>>,
}

impl AdminTokenStore {
    /// Creates the open state required by UDS.
    pub async fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let auth_dir = data_dir.as_ref().join("auth");
        fs::create_dir_all(&auth_dir).await?;
        harden_dir(&auth_dir).await?;
        let path = auth_dir.join("admin-tokens.json");
        let records = if path.exists() {
            let bytes = fs::read(&path).await?;
            serde_json::from_slice(&bytes)?
        } else {
            Vec::new()
        };
        Ok(Self {
            path,
            records: Arc::new(RwLock::new(records)),
        })
    }

    /// Validates the authenticate input before UDS trusts or persists it.
    pub async fn authenticate(&self, token: &str) -> Option<(ActorIdentity, bool)> {
        let (id, _) = parse_admin_token(token)?;
        let records = self.records.read().await;
        let record = records.iter().find(|item| item.id == id)?;
        if !verify_token(token, &record.verifier) {
            return None;
        }
        Some((
            ActorIdentity {
                actor_type: ActorType::Admin,
                token_id: Some(id),
                token_name: Some(record.name.clone()),
            },
            record.enabled,
        ))
    }

    /// Retrieves the list information required by the caller.
    pub async fn list(&self) -> Vec<AdminTokenMetadata> {
        self.records
            .read()
            .await
            .iter()
            .map(AdminTokenMetadata::from)
            .collect()
    }

    /// Merge fleet state. Higher revisions win; equal-revision conflicts use the
    /// lexicographically ordered mutation UUID as a stable fleet-wide tie-break.
    pub async fn merge(&self, incoming: Vec<AdminTokenRecord>) -> Result<()> {
        let mut records = self.records.write().await;
        let mut changed = false;
        for candidate in incoming {
            match records.iter_mut().find(|record| record.id == candidate.id) {
                Some(current)
                    if candidate.revision > current.revision
                        || (candidate.revision == current.revision
                            && candidate.last_mutation_id > current.last_mutation_id) =>
                {
                    *current = candidate;
                    changed = true;
                }
                None => {
                    records.push(candidate);
                    changed = true;
                }
                _ => {}
            }
        }
        if changed {
            self.persist(&records).await?;
        }
        Ok(())
    }

    /// Retrieves the fleet snapshot information required by the caller.
    pub async fn fleet_snapshot(&self) -> Vec<AdminTokenRecord> {
        self.records.read().await.clone()
    }

    /// Creates the create state required by UDS.
    pub async fn create(&self, name: String, reason: String) -> Result<(AdminTokenMetadata, String)> {
        require_text("name", &name)?;
        require_text("reason", &reason)?;
        let id = Uuid::new_v4();
        let token = generate_admin_token(id)?;
        let mutation_id = Uuid::new_v4();
        let record = AdminTokenRecord {
            id,
            verifier: verifier(&token),
            name: name.trim().to_string(),
            created_at: OffsetDateTime::now_utc(),
            creation_reason: reason.trim().to_string(),
            enabled: true,
            status_history: Vec::new(),
            revision: 1,
            last_mutation_id: mutation_id,
        };
        let metadata = AdminTokenMetadata::from(&record);
        let mut records = self.records.write().await;
        records.push(record);
        self.persist(&records).await?;
        Ok((metadata, token))
    }

    /// Applies the set enabled mutation while preserving UDS consistency guarantees.
    pub async fn set_enabled(&self, id: Uuid, enabled: bool, reason: String) -> Result<AdminTokenMetadata> {
        require_text("reason", &reason)?;
        let mut records = self.records.write().await;
        let record = records
            .iter_mut()
            .find(|item| item.id == id)
            .ok_or_else(|| UdsError::NotFound(format!("admin token {id} was not found")))?;
        if record.enabled != enabled {
            let mutation_id = Uuid::new_v4();
            record.enabled = enabled;
            record.revision += 1;
            record.last_mutation_id = mutation_id;
            record.status_history.push(StatusHistoryEntry {
                enabled,
                changed_at: OffsetDateTime::now_utc(),
                reason: reason.trim().to_string(),
                mutation_id,
            });
            let metadata = AdminTokenMetadata::from(&*record);
            self.persist(&records).await?;
            return Ok(metadata);
        }
        Ok(AdminTokenMetadata::from(&*record))
    }

    /// Performs the persist operation required by UDS.
    async fn persist(&self, records: &[AdminTokenRecord]) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(records)?;
        let tmp = self
            .path
            .with_extension(format!("json.tmp-{}", Uuid::new_v4()));
        fs::write(&tmp, bytes).await?;
        harden_file(&tmp).await?;
        let file = fs::OpenOptions::new().write(true).open(&tmp).await?;
        file.sync_all().await?;
        fs::rename(&tmp, &self.path).await?;
        harden_file(&self.path).await?;
        if let Some(parent) = self.path.parent() {
            fs::File::open(parent).await?.sync_all().await?;
        }
        Ok(())
    }
}

/// Creates the generate owner token state required by UDS.
pub fn generate_owner_token() -> Result<String> {
    Ok(format!("{OWNER_PREFIX}{}", random_secret()?))
}

/// Creates the generate admin token state required by UDS.
pub fn generate_admin_token(id: Uuid) -> Result<String> {
    Ok(format!("{ADMIN_PREFIX}{id}_{}", random_secret()?))
}

/// Generates the high-entropy secret portion of owner and administrator tokens.
fn random_secret() -> Result<String> {
    let mut bytes = [0u8; 64];
    getrandom::fill(&mut bytes)
        .map_err(|error| UdsError::Storage(format!("secure random generation failed: {error}")))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// Provides the verifier operation used by UDS callers.
pub fn verifier(token: &str) -> String {
    format!(
        "{VERIFIER_PREFIX}{}",
        hex::encode(Sha512::digest(token.as_bytes()))
    )
}

/// Validates the verify owner input before UDS trusts or persists it.
pub fn verify_owner(token: &str, expected: &str) -> bool {
    token.starts_with(OWNER_PREFIX) && verify_token(token, expected)
}

/// Compares a supplied token with its persisted digest without leaking timing information.
fn verify_token(token: &str, expected: &str) -> bool {
    let Some(expected_digest) = expected.strip_prefix(VERIFIER_PREFIX) else {
        return false;
    };
    let actual = hex::encode(Sha512::digest(token.as_bytes()));
    constant_time_eq(actual.as_bytes(), expected_digest.as_bytes())
}

/// Extracts the token identifier needed to locate its persisted verifier.
fn parse_admin_token(token: &str) -> Option<(Uuid, &str)> {
    let rest = token.strip_prefix(ADMIN_PREFIX)?;
    let (id, secret) = rest.split_once('_')?;
    if secret.is_empty() {
        return None;
    }
    Some((Uuid::parse_str(id).ok()?, secret))
}

/// Rejects empty audit fields so token mutations remain understandable later.
fn require_text(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(UdsError::BadRequest(format!("{field} is required")))
    } else {
        Ok(())
    }
}

/// Compares digests in constant time to avoid exposing matching prefixes.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut diff = left.len() ^ right.len();
    let length = left.len().max(right.len());
    for index in 0..length {
        diff |= (left.get(index).copied().unwrap_or(0) ^ right.get(index).copied().unwrap_or(0)) as usize;
    }
    diff == 0
}

/// Performs the harden dir operation required by UDS.
#[cfg(unix)]
async fn harden_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).await?;
    Ok(())
}

/// Verifies that harden dir.
#[cfg(not(unix))]
async fn harden_dir(_path: &Path) -> Result<()> {
    Ok(())
}

/// Performs the harden file operation required by UDS.
#[cfg(unix)]
async fn harden_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
    Ok(())
}

/// Verifies that harden file.
#[cfg(not(unix))]
async fn harden_file(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that secrets are not persisted and history survives restart.
    #[tokio::test]
    async fn secrets_are_not_persisted_and_history_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = AdminTokenStore::open(dir.path()).await.unwrap();
        let (created, token) = store
            .create("Thorsten".into(), "daily work".into())
            .await
            .unwrap();
        store
            .set_enabled(created.id, false, "lost".into())
            .await
            .unwrap();
        let disk = fs::read_to_string(dir.path().join("auth/admin-tokens.json"))
            .await
            .unwrap();
        assert!(!disk.contains(&token));
        assert!(disk.contains("sha512:"));
        let reopened = AdminTokenStore::open(dir.path()).await.unwrap();
        let item = reopened.list().await.pop().unwrap();
        assert_eq!(item.disabled_reason.as_deref(), Some("lost"));
        assert!(!item.enabled);
    }

    /// Verifies that authentication and idempotent status changes work.
    #[tokio::test]
    async fn authentication_and_idempotent_status_changes_work() {
        let dir = tempfile::tempdir().unwrap();
        let store = AdminTokenStore::open(dir.path()).await.unwrap();
        let (created, token) = store
            .create("automation".into(), "publishing".into())
            .await
            .unwrap();
        assert!(store.authenticate(&token).await.unwrap().1);
        assert!(store.authenticate(&(token.clone() + "x")).await.is_none());
        let first = store
            .set_enabled(created.id, false, "retired".into())
            .await
            .unwrap();
        let second = store
            .set_enabled(created.id, false, "duplicate request".into())
            .await
            .unwrap();
        assert_eq!(first.status_history.len(), 1);
        assert_eq!(second.status_history.len(), 1);
        assert!(!store.authenticate(&token).await.unwrap().1);
        let enabled = store
            .set_enabled(created.id, true, "needed again".into())
            .await
            .unwrap();
        assert_eq!(enabled.status_history.len(), 2);
        assert_eq!(enabled.disabled_reason.as_deref(), Some("retired"));
    }

    /// Verifies that fleet merge resolves same revision by mutation id.
    #[tokio::test]
    async fn fleet_merge_resolves_same_revision_by_mutation_id() {
        let left_dir = tempfile::tempdir().unwrap();
        let right_dir = tempfile::tempdir().unwrap();
        let left = AdminTokenStore::open(left_dir.path()).await.unwrap();
        let (created, _) = left
            .create("deploy".into(), "automation".into())
            .await
            .unwrap();
        let mut low = left.fleet_snapshot().await.pop().unwrap();
        let mut high = low.clone();
        low.enabled = false;
        low.last_mutation_id = Uuid::from_u128(1);
        high.enabled = true;
        high.last_mutation_id = Uuid::from_u128(2);
        let right = AdminTokenStore::open(right_dir.path()).await.unwrap();
        right.merge(vec![high.clone()]).await.unwrap();
        right.merge(vec![low]).await.unwrap();
        assert!(
            right
                .list()
                .await
                .into_iter()
                .find(|v| v.id == created.id)
                .unwrap()
                .enabled
        );
    }
}
