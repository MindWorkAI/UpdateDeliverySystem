//! Interactive workflow for copy administration.

use super::*;

/// Performs the run operation required by UDS.
pub(super) async fn run() -> Result<()> {
    let (_config, _profile_name, profile) = load_profile_or_configure().await?;
    let client = AdminClient::new(&profile)?;
    let source_channel = prompt_channel(&profile)?;
    let release = select_release(&client, &source_channel).await?;
    let target_channel = Text::new("Target channel:")
        .prompt()
        .map_err(prompt_error)?;
    let confirmed = Confirm::new(&format!(
        "Copy release {} from '{}' to '{}'?",
        release.version, source_channel, target_channel
    ))
    .with_default(false)
    .prompt()
    .map_err(prompt_error)?;
    if confirmed {
        let response = client
            .copy_release(&source_channel, &target_channel, &release.version)
            .await?;
        println!(
            "Copied {} to channel '{}'. Replicated: {}",
            response.version, response.channel, response.replicated
        );
    }
    Ok(())
}
