//! Interactive workflow for logs administration.

use super::*;

/// Performs the run operation required by UDS.
pub(super) async fn run(
    follow: bool,
    lines: usize,
    level: Option<crate::config::LogLevel>,
    no_color: bool,
) -> Result<()> {
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
