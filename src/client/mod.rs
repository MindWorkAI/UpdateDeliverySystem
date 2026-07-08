mod api;
mod config;
mod import;
mod prompts;

use crate::config::ClientCommand;
use crate::errors::Result;

pub async fn run(command: Option<ClientCommand>) -> Result<()> {
    prompts::run(command).await
}
