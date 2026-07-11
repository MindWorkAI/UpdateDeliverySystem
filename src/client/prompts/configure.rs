//! Interactive workflow for configure administration.

use super::*;

/// Performs the run operation required by UDS.
pub(super) async fn run() -> Result<()> {
    let mut config = load_or_default().await?;
    let default_name = config
        .active_profile
        .clone()
        .unwrap_or_else(|| "default".to_string());
    let profile_name = Text::new("Profile name:")
        .with_default(&default_name)
        .prompt()
        .map_err(prompt_error)?;
    let existing = config.profiles.get(&profile_name);
    let default_url = existing
        .map(|profile| profile.base_url.as_str())
        .unwrap_or("https://updates.example.org");
    let base_url = Text::new("UDS base URL:")
        .with_default(default_url)
        .prompt()
        .map_err(prompt_error)?;
    let admin_token = Password::new("Admin token:")
        .without_confirmation()
        .prompt()
        .map_err(prompt_error)?;
    let default_channel = Text::new("Default channel:")
        .with_default(
            existing
                .and_then(|profile| profile.default_channel.as_deref())
                .unwrap_or("stable"),
        )
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
