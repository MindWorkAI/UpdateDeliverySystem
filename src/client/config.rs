//! Local administration-client profiles and persisted configuration.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::errors::{Result, UdsError};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
/// Complete local client configuration containing named server profiles.
pub struct ClientConfig {
    /// Stores the active profile value used by this UDS component.
    #[serde(default)]
    pub active_profile: Option<String>,

    /// Stores the profiles value used by this UDS component.
    #[serde(default)]
    pub profiles: BTreeMap<String, ClientProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Connection information and credential for one UDS installation.
pub struct ClientProfile {
    /// Stores the base url value used by this UDS component.
    pub base_url: String,

    /// Stores the admin token value used by this UDS component.
    pub admin_token: String,

    /// Stores the default channel value used by this UDS component.
    #[serde(default)]
    pub default_channel: Option<String>,
}

impl ClientConfig {
    /// Performs the active profile operation required by UDS.
    pub fn active_profile(&self) -> Result<(&str, &ClientProfile)> {
        let name = self
            .active_profile
            .as_deref()
            .ok_or_else(|| UdsError::Config("no active client profile is configured".to_string()))?;
        let profile = self
            .profiles
            .get(name)
            .ok_or_else(|| UdsError::Config(format!("active client profile '{name}' does not exist")))?;
        Ok((name, profile))
    }
}

/// Performs the config path operation required by UDS.
pub fn config_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("org", "MindWork AI", "UDS")
        .ok_or_else(|| UdsError::Config("could not determine the user configuration directory".to_string()))?;
    Ok(dirs.config_dir().join("client.toml"))
}

/// Performs the load or default operation required by UDS.
pub async fn load_or_default() -> Result<ClientConfig> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(ClientConfig::default());
    }
    verify_private_permissions(&path).await?;
    let text = fs::read_to_string(path).await?;
    let config: ClientConfig = toml::from_str(&text)?;
    validate_profiles(&config)?;
    Ok(config)
}

/// Performs the save operation required by UDS.
pub async fn save(config: &ClientConfig) -> Result<PathBuf> {
    validate_profiles(config)?;
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
        harden_directory(parent).await?;
    }

    let text = toml::to_string_pretty(config)
        .map_err(|error| UdsError::Config(format!("failed to serialize client config: {error}")))?;
    fs::write(&path, text).await?;
    harden_file(&path).await?;
    verify_private_permissions(&path).await?;
    Ok(path)
}

/// Performs the validate profiles operation required by UDS.
fn validate_profiles(config: &ClientConfig) -> Result<()> {
    for (name, profile) in &config.profiles {
        if profile.admin_token.starts_with(crate::auth::OWNER_PREFIX) {
            return Err(UdsError::Config(format!(
                "profile '{name}' must not store an owner token"
            )));
        }
        if !profile.admin_token.starts_with(crate::auth::ADMIN_PREFIX) {
            return Err(UdsError::Config(format!(
                "profile '{name}' must contain a uds_admin_v1 token"
            )));
        }
    }
    Ok(())
}

/// Performs the harden directory operation required by UDS.
#[cfg(unix)]
async fn harden_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = std::fs::Permissions::from_mode(0o700);
    fs::set_permissions(path, permissions).await?;
    Ok(())
}

/// Performs the harden file operation required by UDS.
#[cfg(unix)]
async fn harden_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = std::fs::Permissions::from_mode(0o600);
    fs::set_permissions(path, permissions).await?;
    Ok(())
}

/// Performs the verify private permissions operation required by UDS.
#[cfg(unix)]
async fn verify_private_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = fs::metadata(path).await?;
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(UdsError::Config(format!(
            "client config '{}' is readable or writable by group/others; set permissions to 0600",
            path.display()
        )));
    }

    let current_uid = unsafe { libc::geteuid() };
    if metadata.uid() != current_uid {
        return Err(UdsError::Config(format!(
            "client config '{}' is not owned by the current user",
            path.display()
        )));
    }

    Ok(())
}

/// Verifies that harden directory.
#[cfg(windows)]
async fn harden_directory(path: &Path) -> Result<()> {
    harden_windows_path(path)
}

/// Verifies that harden file.
#[cfg(windows)]
async fn harden_file(path: &Path) -> Result<()> {
    harden_windows_path(path)
}

/// Verifies that verify private permissions.
#[cfg(windows)]
async fn verify_private_permissions(path: &Path) -> Result<()> {
    if !path.exists() {
        return Err(UdsError::Config(format!(
            "client config '{}' does not exist",
            path.display()
        )));
    }
    Ok(())
}

/// Verifies that harden windows path.
#[cfg(windows)]
fn harden_windows_path(path: &Path) -> Result<()> {
    let username = std::env::var("USERNAME")
        .map_err(|_| UdsError::Config("could not determine the current Windows user name".to_string()))?;
    let status = std::process::Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(format!("{username}:F"))
        .status()
        .map_err(|error| {
            UdsError::Config(format!(
                "failed to run icacls for '{}': {error}",
                path.display()
            ))
        })?;

    if status.success() {
        Ok(())
    } else {
        Err(UdsError::Config(format!(
            "failed to harden Windows ACLs for '{}'; icacls exited with {status}",
            path.display()
        )))
    }
}
