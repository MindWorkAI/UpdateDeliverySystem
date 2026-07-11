//! Interactive workflow for withdraw administration.

use super::*;

/// Performs the run operation required by UDS.
pub(super) async fn run() -> Result<()> {
    let (_config, _profile_name, profile) = load_profile_or_configure().await?;
    let client = AdminClient::new(&profile)?;
    let channel = prompt_channel(&profile)?;
    let release = select_release(&client, &channel).await?;
    let confirmed = Confirm::new(&format!(
        "Withdraw release {} from channel '{}'?",
        release.version, channel
    ))
    .with_default(false)
    .prompt()
    .map_err(prompt_error)?;
    if confirmed {
        let response = client.withdraw_release(&channel, &release.version).await?;
        println!(
            "Withdrew {} from channel '{}'. Replicated: {}",
            response.version, response.channel, response.replicated
        );
    }
    Ok(())
}
