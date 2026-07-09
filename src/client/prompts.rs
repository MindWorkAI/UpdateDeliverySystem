use std::path::PathBuf;

use inquire::{Confirm, Password, Select, Text};

use crate::client::api::{AdminClient, display_path};
use crate::client::config::{ClientConfig, ClientProfile, load_or_default, save};
use crate::client::import::{PreparedUpload, prepare_from_local, prepare_from_remote};
use crate::config::ClientCommand;
use crate::errors::{Result, UdsError};
use crate::logging::{color_enabled, render_log_event, should_display_level};
use crate::models::ReleaseListEntry;

pub async fn run(command: Option<ClientCommand>) -> Result<()> {
    match command {
        Some(ClientCommand::Configure) => configure().await,
        Some(ClientCommand::Upload) => upload().await,
        Some(ClientCommand::Withdraw) => withdraw().await,
        Some(ClientCommand::Copy) => copy().await,
        Some(ClientCommand::Changelog) => changelog().await,
        Some(ClientCommand::Stats) => stats().await,
        Some(ClientCommand::Logs { follow, lines, level, no_color }) => logs(follow, lines, level, no_color).await,
        None => main_menu().await,
    }
}

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
            MenuAction::Configure,
        ],
    )
    .prompt()
    .map_err(prompt_error)?;

    match action {
        MenuAction::Configure => configure().await,
        MenuAction::Upload => upload().await,
        MenuAction::Withdraw => withdraw().await,
        MenuAction::Copy => copy().await,
        MenuAction::Changelog => changelog().await,
        MenuAction::Stats => stats().await,
        MenuAction::Logs => logs(false, 200, None, false).await,
    }
}

async fn configure() -> Result<()> {
    let mut config = load_or_default().await?;
    let default_name = config.active_profile.clone().unwrap_or_else(|| "default".to_string());
    let profile_name = Text::new("Profile name:")
        .with_default(&default_name)
        .prompt()
        .map_err(prompt_error)?;
    let existing = config.profiles.get(&profile_name);
    let default_url = existing.map(|profile| profile.base_url.as_str()).unwrap_or("https://updates.example.org");
    let base_url = Text::new("UDS base URL:")
        .with_default(default_url)
        .prompt()
        .map_err(prompt_error)?;
    let admin_token = Password::new("Admin token:")
        .without_confirmation()
        .prompt()
        .map_err(prompt_error)?;
    let default_channel = Text::new("Default channel:")
        .with_default(existing.and_then(|profile| profile.default_channel.as_deref()).unwrap_or("stable"))
        .prompt()
        .map_err(prompt_error)?;

    config.profiles.insert(
        profile_name.clone(),
        ClientProfile {
            base_url,
            admin_token,
            default_channel: non_empty(default_channel),
        },
    );
    config.active_profile = Some(profile_name);
    let path = save(&config).await?;
    println!("Saved client configuration to {}", path.display());
    Ok(())
}

async fn upload() -> Result<()> {
    let (_config, _profile_name, profile) = load_profile_or_configure().await?;
    let client = AdminClient::new(&profile)?;
    let channel = prompt_channel(&profile)?;
    let source = Select::new("Upload source:", vec![UploadSource::GitHubOrUrl, UploadSource::LocalFiles])
        .prompt()
        .map_err(prompt_error)?;

    let upload = match source {
        UploadSource::GitHubOrUrl => {
            let url = Text::new("GitHub release URL or latest.json URL:").prompt().map_err(prompt_error)?;
            prepare_from_remote(&url).await?
        }
        UploadSource::LocalFiles => {
            let latest_json = Text::new("Path to local latest.json:").prompt().map_err(prompt_error)?;
            let artifact_dir = Text::new("Directory containing referenced artifacts:")
                .with_default(".")
                .prompt()
                .map_err(prompt_error)?;
            prepare_from_local(&PathBuf::from(latest_json), &PathBuf::from(artifact_dir)).await?
        }
    };

    print_upload_review(&channel, &upload);
    let confirmed = Confirm::new("Upload this release to UDS?")
        .with_default(false)
        .prompt()
        .map_err(prompt_error)?;
    if !confirmed {
        println!("Upload cancelled.");
        return Ok(());
    }

    let response = client.upload_release(&channel, &upload).await?;
    println!("Uploaded {} to channel '{}'. Replicated: {}", response.version, response.channel, response.replicated);
    Ok(())
}

async fn withdraw() -> Result<()> {
    let (_config, _profile_name, profile) = load_profile_or_configure().await?;
    let client = AdminClient::new(&profile)?;
    let channel = prompt_channel(&profile)?;
    let release = select_release(&client, &channel).await?;
    let confirmed = Confirm::new(&format!("Withdraw release {} from channel '{}'?", release.version, channel))
        .with_default(false)
        .prompt()
        .map_err(prompt_error)?;
    if confirmed {
        let response = client.withdraw_release(&channel, &release.version).await?;
        println!("Withdrew {} from channel '{}'. Replicated: {}", response.version, response.channel, response.replicated);
    }
    Ok(())
}

async fn copy() -> Result<()> {
    let (_config, _profile_name, profile) = load_profile_or_configure().await?;
    let client = AdminClient::new(&profile)?;
    let source_channel = prompt_channel(&profile)?;
    let release = select_release(&client, &source_channel).await?;
    let target_channel = Text::new("Target channel:").prompt().map_err(prompt_error)?;
    let confirmed = Confirm::new(&format!(
        "Copy release {} from '{}' to '{}'?",
        release.version, source_channel, target_channel
    ))
    .with_default(false)
    .prompt()
    .map_err(prompt_error)?;
    if confirmed {
        let response = client.copy_release(&source_channel, &target_channel, &release.version).await?;
        println!("Copied {} to channel '{}'. Replicated: {}", response.version, response.channel, response.replicated);
    }
    Ok(())
}

async fn changelog() -> Result<()> {
    let (_config, _profile_name, profile) = load_profile_or_configure().await?;
    let client = AdminClient::new(&profile)?;
    let channel = prompt_channel(&profile)?;
    let release = select_release(&client, &channel).await?;
    println!("Enter the new changelog. Finish input with an empty line.");
    let mut lines = Vec::new();
    loop {
        let line = Text::new(">").prompt().map_err(prompt_error)?;
        if line.is_empty() {
            break;
        }
        lines.push(line);
    }
    let notes = lines.join("\n");
    println!("\nNew changelog for {}:\n{}\n", release.version, notes);
    let confirmed = Confirm::new("Apply this changelog?")
        .with_default(false)
        .prompt()
        .map_err(prompt_error)?;
    if confirmed {
        let response = client.patch_changelog(&channel, &release.version, notes).await?;
        println!("Updated changelog for {} in channel '{}'. Replicated: {}", response.version, response.channel, response.replicated);
    }
    Ok(())
}

async fn stats() -> Result<()> {
    let (_config, _profile_name, profile) = load_profile_or_configure().await?;
    let client = AdminClient::new(&profile)?;
    let channel = prompt_channel(&profile)?;
    let stats = client.channel_stats(&channel).await?;
    println!("Statistics for channel '{channel}'");
    println!("Update checks: {}", stats.update_checks);
    println!("Downloads: {}", stats.downloads);
    println!("Traffic bytes: {}", stats.traffic_bytes);
    for (platform, platform_stats) in stats.by_platform {
        println!(
            "- {platform}: {} downloads, {} bytes",
            platform_stats.downloads, platform_stats.traffic_bytes
        );
    }
    Ok(())
}

async fn logs(follow: bool, lines: usize, level: Option<crate::config::LogLevel>, no_color: bool) -> Result<()> {
    let (_config, _profile_name, profile) = load_profile_or_configure().await?;
    let client = AdminClient::new(&profile)?;
    let color = color_enabled(no_color);

    if follow {
        client
            .stream_logs(lines, |event| {
                if should_display_level(event.level, level) {
                    println!("{}", render_log_event(&event, color));
                }
            })
            .await
    } else {
        for event in client.recent_logs(lines).await? {
            if should_display_level(event.level, level) {
                println!("{}", render_log_event(&event, color));
            }
        }
        Ok(())
    }
}

async fn load_profile_or_configure() -> Result<(ClientConfig, String, ClientProfile)> {
    let config = load_or_default().await?;
    if config.profiles.is_empty() {
        let create = Confirm::new("No UDS client config exists yet. Create one now?")
            .with_default(true)
            .prompt()
            .map_err(prompt_error)?;
        if !create {
            return Err(UdsError::Config("client configuration is required".to_string()));
        }
        configure().await?;
    }

    let config = load_or_default().await?;
    let (name, profile) = config.active_profile()?;
    Ok((config.clone(), name.to_string(), profile.clone()))
}

fn prompt_channel(profile: &ClientProfile) -> Result<String> {
    let default = profile.default_channel.as_deref().unwrap_or("stable");
    Text::new("Channel:").with_default(default).prompt().map_err(prompt_error)
}

async fn select_release(client: &AdminClient, channel: &str) -> Result<ReleaseListEntry> {
    let response = client.list_releases(channel).await?;
    if response.releases.is_empty() {
        return Err(UdsError::NotFound(format!("channel '{channel}' has no releases")));
    }

    let choices = response
        .releases
        .iter()
        .map(format_release_choice)
        .collect::<Vec<_>>();
    let selected = Select::new("Release:", choices).prompt().map_err(prompt_error)?;
    response
        .releases
        .into_iter()
        .find(|release| format_release_choice(release) == selected)
        .ok_or_else(|| UdsError::Storage("selected release was not found".to_string()))
}

fn print_upload_review(channel: &str, upload: &PreparedUpload) {
    println!("\nUpload review");
    println!("Channel: {channel}");
    println!("Version: {}", upload.metadata.version);
    if let Some(pub_date) = &upload.metadata.pub_date {
        println!("Publication date: {pub_date}");
    }
    println!("Notes preview:");
    println!("{}", upload.metadata.notes.lines().take(12).collect::<Vec<_>>().join("\n"));
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

fn format_release_choice(release: &ReleaseListEntry) -> String {
    let withdrawn = if release.withdrawn { " withdrawn" } else { "" };
    let pub_date = release.pub_date.as_deref().unwrap_or("no pub_date");
    format!("{} ({pub_date}, {} platforms{withdrawn})", release.version, release.platforms.len())
}

fn non_empty(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

fn prompt_error(error: inquire::InquireError) -> UdsError {
    UdsError::Config(format!("prompt failed: {error}"))
}

#[derive(Debug, Clone, Copy)]
enum MenuAction {
    Configure,
    Upload,
    Withdraw,
    Copy,
    Changelog,
    Stats,
    Logs,
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
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum UploadSource {
    GitHubOrUrl,
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
