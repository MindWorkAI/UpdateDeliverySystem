//! Dispatches interactive terminal workflows for UDS administration tasks.
//!
//! User-input handling stays separate from HTTP and import code, so prompts do
//! not obscure the underlying server operations.

mod changelog;
mod configure;
mod copy;
mod logs;
mod stats;
mod tokens;
mod updates;
mod upload;
mod withdraw;

use std::path::PathBuf;

use inquire::{Confirm, Password, Select, Text};

use crate::client::api::{AdminClient, display_path};
use crate::client::config::{ClientConfig, ClientProfile, load_or_default, save};
use crate::client::import::{PreparedUpload, prepare_from_local, prepare_from_remote};
use crate::config::ClientCommand;
use crate::config::TokenCommand;
use crate::errors::{Result, UdsError};
use crate::logging::{color_enabled, render_log_event, should_display_level};
use crate::models::ReleaseListEntry;
use zeroize::Zeroize;

/// Performs the run operation required by UDS.
pub async fn run(command: Option<ClientCommand>) -> Result<()> {
    match command {
        Some(ClientCommand::Configure) => configure::run().await,
        Some(ClientCommand::Upload) => upload::run().await,
        Some(ClientCommand::Withdraw) => withdraw::run().await,
        Some(ClientCommand::Copy) => copy::run().await,
        Some(ClientCommand::Changelog) => changelog::run().await,
        Some(ClientCommand::Stats) => stats::run().await,
        Some(ClientCommand::Updates) => updates::run().await,
        Some(ClientCommand::Tokens { command }) => tokens::run(command).await,
        Some(ClientCommand::Logs {
            follow,
            lines,
            level,
            no_color,
        }) => logs::run(follow, lines, level, no_color).await,
        None => main_menu().await,
    }
}

/// Performs the main menu operation required by UDS.
async fn main_menu() -> Result<()> {
    let action = Select::new(
        "What do you want to do?",
        vec![
            MenuAction::Upload,
            MenuAction::Withdraw,
            MenuAction::Copy,
            MenuAction::Changelog,
            MenuAction::Stats,
            MenuAction::Logs,
            MenuAction::Updates,
            MenuAction::Configure,
        ],
    )
    .prompt()
    .map_err(prompt_error)?;

    match action {
        MenuAction::Configure => configure::run().await,
        MenuAction::Upload => upload::run().await,
        MenuAction::Withdraw => withdraw::run().await,
        MenuAction::Copy => copy::run().await,
        MenuAction::Changelog => changelog::run().await,
        MenuAction::Stats => stats::run().await,
        MenuAction::Logs => logs::run(false, 200, None, false).await,
        MenuAction::Updates => updates::run().await,
    }
}

/// Performs the load profile or configure operation required by UDS.
async fn load_profile_or_configure() -> Result<(ClientConfig, String, ClientProfile)> {
    let config = load_or_default().await?;
    if config.profiles.is_empty() {
        let create = Confirm::new("No UDS client config exists yet. Create one now?")
            .with_default(true)
            .prompt()
            .map_err(prompt_error)?;
        if !create {
            return Err(UdsError::Config(
                "client configuration is required".to_string(),
            ));
        }
        configure::run().await?;
    }

    let config = load_or_default().await?;
    let (name, profile) = config.active_profile()?;
    Ok((config.clone(), name.to_string(), profile.clone()))
}

/// Performs the prompt channel operation required by UDS.
fn prompt_channel(profile: &ClientProfile) -> Result<String> {
    let default = profile.default_channel.as_deref().unwrap_or("stable");
    Text::new("Channel:")
        .with_default(default)
        .prompt()
        .map_err(prompt_error)
}

/// Performs the select release operation required by UDS.
async fn select_release(client: &AdminClient, channel: &str) -> Result<ReleaseListEntry> {
    let response = client.list_releases(channel).await?;
    if response.releases.is_empty() {
        return Err(UdsError::NotFound(format!(
            "channel '{channel}' has no releases"
        )));
    }

    let choices = response
        .releases
        .iter()
        .map(format_release_choice)
        .collect::<Vec<_>>();
    let selected = Select::new("Release:", choices)
        .prompt()
        .map_err(prompt_error)?;
    response
        .releases
        .into_iter()
        .find(|release| format_release_choice(release) == selected)
        .ok_or_else(|| UdsError::Storage("selected release was not found".to_string()))
}

/// Performs the print upload review operation required by UDS.
fn print_upload_review(channel: &str, upload: &PreparedUpload) {
    println!("\nUpload review");
    println!("Channel: {channel}");
    println!("Version: {}", upload.metadata.version);
    if let Some(pub_date) = &upload.metadata.pub_date {
        println!("Publication date: {pub_date}");
    }
    println!("Notes preview:");
    println!(
        "{}",
        upload
            .metadata
            .notes
            .lines()
            .take(12)
            .collect::<Vec<_>>()
            .join("\n")
    );
    println!("\nArtifacts:");
    for artifact in &upload.artifacts {
        println!("- {}", artifact.platform);
        println!("  File: {}", artifact.file_name);
        println!("  Path: {}", display_path(&artifact.path));
        println!("  Source: {}", artifact.source_url);
        println!("  Size: {} bytes", artifact.size);
        println!("  SHA-256: {}", artifact.sha256);
    }
    println!();
}

/// Performs the format release choice operation required by UDS.
fn format_release_choice(release: &ReleaseListEntry) -> String {
    let withdrawn = if release.withdrawn { " withdrawn" } else { "" };
    let pub_date = release.pub_date.as_deref().unwrap_or("no pub_date");
    format!(
        "{} ({pub_date}, {} platforms{withdrawn})",
        release.version,
        release.platforms.len()
    )
}

/// Performs the non empty operation required by UDS.
fn non_empty(value: String) -> Option<String> {
    if value.trim().is_empty() { None } else { Some(value) }
}

/// Performs the prompt error operation required by UDS.
fn prompt_error(error: inquire::InquireError) -> UdsError {
    UdsError::Config(format!("prompt failed: {error}"))
}

#[derive(Debug, Clone, Copy)]
/// Top-level operation selected from the interactive client menu.
enum MenuAction {
    /// Represents the item concept used by UDS.
    Configure,

    /// Represents the item concept used by UDS.
    Upload,

    /// Represents the item concept used by UDS.
    Withdraw,

    /// Represents the item concept used by UDS.
    Copy,

    /// Represents the item concept used by UDS.
    Changelog,

    /// Represents the item concept used by UDS.
    Stats,

    /// Represents the item concept used by UDS.
    Logs,

    /// Opens the explicit manual UDS update selection workflow.
    Updates,
}

impl std::fmt::Display for MenuAction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MenuAction::Configure => write!(formatter, "Configure client"),
            MenuAction::Upload => write!(formatter, "Upload release"),
            MenuAction::Withdraw => write!(formatter, "Withdraw release"),
            MenuAction::Copy => write!(formatter, "Copy release"),
            MenuAction::Changelog => write!(formatter, "Correct changelog"),
            MenuAction::Stats => write!(formatter, "Show statistics"),
            MenuAction::Logs => write!(formatter, "Show logs"),
            MenuAction::Updates => write!(formatter, "Update UDS manually"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
/// Source from which the client prepares a release upload.
enum UploadSource {
    /// Represents the item concept used by UDS.
    GitHubOrUrl,

    /// Represents the item concept used by UDS.
    LocalFiles,
}

impl std::fmt::Display for UploadSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UploadSource::GitHubOrUrl => write!(formatter, "GitHub release or latest.json URL"),
            UploadSource::LocalFiles => write!(formatter, "Local latest.json and artifact files"),
        }
    }
}
