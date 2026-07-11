//! UDS binary entry point.
//!
//! The binary deliberately contains only the minimum startup wiring. Command
//! dispatch and the server lifecycle live in the application module so the
//! entry point stays easy to scan.

mod application;

use clap::Parser;
use update_delivery_system::config::Cli;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    update_delivery_system::build_info::print_banner_if_interactive()?;
    application::run(Cli::parse()).await
}
