//! Interactive workflow for upload administration.

use super::*;

/// Performs the run operation required by UDS.
pub(super) async fn run() -> Result<()> {
    let (_config, _profile_name, profile) = load_profile_or_configure().await?;
    let client = AdminClient::new(&profile)?;
    let policy = client.upload_policy().await?;
    let channel = prompt_channel(&profile)?;
    let source = Select::new(
        "Upload source:",
        vec![UploadSource::GitHubOrUrl, UploadSource::LocalFiles],
    )
    .prompt()
    .map_err(prompt_error)?;

    let upload = match source {
        UploadSource::GitHubOrUrl => {
            let url = Text::new("GitHub release URL or latest.json URL:")
                .prompt()
                .map_err(prompt_error)?;
            prepare_from_remote(&url, &policy).await?
        }
        UploadSource::LocalFiles => {
            let latest_json = Text::new("Path to local latest.json:")
                .prompt()
                .map_err(prompt_error)?;
            let artifact_dir = Text::new("Directory containing referenced artifacts:")
                .with_default(".")
                .prompt()
                .map_err(prompt_error)?;
            prepare_from_local(
                &PathBuf::from(latest_json),
                &PathBuf::from(artifact_dir),
                &policy,
            )
            .await?
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
    println!(
        "Uploaded {} to channel '{}'. Replicated: {}",
        response.version, response.channel, response.replicated
    );
    Ok(())
}
