//! Interactive workflow for tokens administration.

use super::*;

/// Performs the run operation required by UDS.
pub(super) async fn run(command: TokenCommand) -> Result<()> {
    let (_config, _profile_name, profile) = load_profile_or_configure().await?;
    let mut owner_token = Password::new("Owner token:")
        .without_confirmation()
        .prompt()
        .map_err(prompt_error)?;
    let client = AdminClient::with_owner_token(&profile, owner_token.clone())?;
    owner_token.zeroize();
    match command {
        TokenCommand::List => {
            for token in client.admin_tokens().await? {
                println!(
                    "{}  {}  {}",
                    token.id,
                    if token.enabled { "enabled" } else { "disabled" },
                    token.name
                );
                for entry in token.status_history {
                    println!(
                        "  {} {} — {}",
                        entry.changed_at,
                        if entry.enabled { "enabled" } else { "disabled" },
                        entry.reason
                    );
                }
            }
        }
        TokenCommand::Create => {
            let name = Text::new("Token name:").prompt().map_err(prompt_error)?;
            let reason = Text::new("Creation reason:")
                .prompt()
                .map_err(prompt_error)?;
            let mut created = client.create_admin_token(&name, &reason).await?;
            println!(
                "Admin token {} created. This secret is shown exactly once:\n{}",
                created.metadata.id, created.token
            );
            created.token.zeroize();
        }
        TokenCommand::Enable { id } => {
            let enabled = true;
            let reason = Text::new(if enabled {
                "Reactivation reason:"
            } else {
                "Deactivation reason:"
            })
            .prompt()
            .map_err(prompt_error)?;
            let token = client.set_admin_token_enabled(id, enabled, &reason).await?;
            println!(
                "{} is now {}.",
                token.id,
                if token.enabled { "enabled" } else { "disabled" }
            );
        }
        TokenCommand::Disable { id } => {
            let reason = Text::new("Deactivation reason:")
                .prompt()
                .map_err(prompt_error)?;
            let token = client.set_admin_token_enabled(id, false, &reason).await?;
            println!("{} is now disabled.", token.id);
        }
    }
    Ok(())
}
