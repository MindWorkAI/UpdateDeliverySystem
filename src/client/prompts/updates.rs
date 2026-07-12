//! Explicit release selection, confirmation, and polling for manual UDS updates.

use std::fmt;
use std::time::Duration;

use inquire::{Confirm, Select};
use uuid::Uuid;

use crate::client::api::AdminClient;
use crate::errors::{Result, UdsError};
use crate::self_update::{AvailableRelease, OperationStatus, ReleaseKind, StartUpdateRequest};

use super::{load_profile_or_configure, prompt_error};

/// Runs the interactive manual update workflow without unattended behavior.
pub async fn run() -> Result<()> {
    let (_, _, profile) = load_profile_or_configure().await?;
    let client = AdminClient::new(&profile)?;
    let mut kind = ReleaseKind::Regular;
    loop {
        let response = client.update_releases(kind).await?;
        if !response.update_supported {
            return Err(UdsError::BadRequest(
                response
                    .unavailable_reason
                    .unwrap_or_else(|| "updates are unavailable".into()),
            ));
        }
        let mut choices = response
            .releases
            .iter()
            .cloned()
            .map(UpdateChoice::Release)
            .collect::<Vec<_>>();
        choices.push(match kind {
            ReleaseKind::Regular => UpdateChoice::ShowPrereleases,
            ReleaseKind::Prerelease => UpdateChoice::ShowRegular,
        });
        choices.push(UpdateChoice::Cancel);
        let selected = Select::new(
            match kind {
                ReleaseKind::Regular => "Newer regular releases:",
                ReleaseKind::Prerelease => "Newer prereleases:",
            },
            choices,
        )
        .prompt()
        .map_err(prompt_error)?;
        match selected {
            UpdateChoice::ShowPrereleases => kind = ReleaseKind::Prerelease,
            UpdateChoice::ShowRegular => kind = ReleaseKind::Regular,
            UpdateChoice::Cancel => {
                println!("Update cancelled; no changes were made.");
                return Ok(());
            }
            UpdateChoice::Release(release) => {
                println!(
                    "\nUDS update review\nVersion: {}\nBuild: {}\nRelease notes:\n{}\n",
                    release.version, release.build, release.notes
                );
                if !Confirm::new(&format!(
                    "Install exactly UDS v{} (build {})?",
                    release.version, release.build
                ))
                .with_default(false)
                .prompt()
                .map_err(prompt_error)?
                {
                    println!("Update cancelled; no changes were made.");
                    return Ok(());
                }
                let request = StartUpdateRequest {
                    operation_id: Uuid::new_v4(),
                    node_id: response.node_id,
                    version: release.version,
                    allow_prerelease: kind == ReleaseKind::Prerelease,
                };
                let operation = client.start_update(&request).await?;
                return poll(&client, operation.request.operation_id).await;
            }
        }
    }
}

/// Polls durable state while tolerating expected connection loss during restart.
async fn poll(client: &AdminClient, id: Uuid) -> Result<()> {
    let mut last = None;
    loop {
        match client.update_status(id).await {
            Ok(operation) => {
                if last != Some(operation.status) {
                    println!("Update status: {:?}", operation.status);
                    last = Some(operation.status);
                }
                if matches!(operation.status, OperationStatus::Succeeded) {
                    return Ok(());
                }
                if matches!(
                    operation.status,
                    OperationStatus::RolledBack | OperationStatus::Failed
                ) {
                    return Err(UdsError::Storage(operation.error.unwrap_or_else(|| {
                        format!("update ended as {:?}", operation.status)
                    })));
                }
            }
            Err(error) => tracing::debug!(%error, "update polling connection unavailable during restart"),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// One selectable release or navigation action in the update chooser.
#[derive(Debug, Clone)]
enum UpdateChoice {
    /// Exact release offered by the server.
    Release(AvailableRelease),

    /// Switches from the regular list to the explicit prerelease list.
    ShowPrereleases,

    /// Returns from prereleases to the regular list.
    ShowRegular,

    /// Leaves without submitting an operation.
    Cancel,
}

/// Renders concise choices while the review supplies full notes.
impl fmt::Display for UpdateChoice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Release(release) => write!(formatter, "v{} (build {})", release.version, release.build),
            Self::ShowPrereleases => write!(formatter, "Show prereleases"),
            Self::ShowRegular => write!(formatter, "Back to regular releases"),
            Self::Cancel => write!(formatter, "Cancel"),
        }
    }
}
