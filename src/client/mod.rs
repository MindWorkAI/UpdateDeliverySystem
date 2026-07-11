//! Interactive administration client for operators managing a UDS server.
//!
//! Local profiles and prompts stay separate from HTTP transport and release
//! import preparation.

mod api;
mod config;
mod import;
mod prompts;

use crate::config::ClientCommand;
use crate::errors::Result;

pub async fn run(command: Option<ClientCommand>) -> Result<()> {
    prompts::run(command).await
}
