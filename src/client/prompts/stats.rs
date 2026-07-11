//! Interactive workflow for stats administration.

use super::*;

/// Performs the run operation required by UDS.
pub(super) async fn run() -> Result<()> {
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
