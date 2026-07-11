//! Update Delivery System library.
//!
//! The binary crate wires these modules together. Tests use the library entry
//! points directly so route behavior can be verified without binding sockets.

pub mod build_info;
pub mod changelog;
pub mod client;
pub mod cluster;
pub mod config;
pub mod errors;
pub mod logging;
pub mod models;
pub mod routes;
pub mod security;
pub mod server_configure;
pub mod shutdown;
pub mod stats;
pub mod storage;
pub mod tls;

pub use config::{Cli, ServerConfig, ServerMode};
pub use errors::{Result, UdsError};
pub use routes::{AppState, build_router};
