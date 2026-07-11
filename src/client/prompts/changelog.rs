//! Interactive workflow for changelog administration.

use super::*;

/// Performs the run operation required by UDS.
pub(super) async fn run() -> Result<()> {
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
        let response = client
            .patch_changelog(&channel, &release.version, notes)
            .await?;
        println!(
            "Updated changelog for {} in channel '{}'. Replicated: {}",
            response.version, response.channel, response.replicated
        );
    }
    Ok(())
}
