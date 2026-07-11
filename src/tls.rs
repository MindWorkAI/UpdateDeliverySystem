//! HTTP listener startup with optional file-based TLS.
//!
//! Centralizing listener setup ensures all public, admin, and fleet APIs apply
//! the same transport behavior.

use axum::Router;
use axum_server::Handle;
use axum_server::tls_rustls::RustlsConfig;

use crate::config::{TlsConfig, TlsMode};
use crate::errors::{Result, UdsError};

/// Runs the serve workflow for UDS.
pub async fn serve(
    name: &'static str,
    bind: std::net::SocketAddr,
    tls: TlsConfig,
    router: Router,
    handle: Handle<std::net::SocketAddr>,
) -> Result<()> {
    match tls.mode {
        TlsMode::Off => {
            tracing::warn!(listener = name, %bind, "starting HTTP server without TLS");
            axum_server::bind(bind)
                .handle(handle)
                .serve(router.into_make_service_with_connect_info::<std::net::SocketAddr>())
                .await
                .map_err(|error| UdsError::Storage(format!("server failed: {error}")))?;
        }
        TlsMode::Files => {
            let cert_path = tls.cert_path.as_ref().expect("validated cert_path");
            let key_path = tls.key_path.as_ref().expect("validated key_path");
            let tls_config = RustlsConfig::from_pem_file(cert_path, key_path).await?;
            tracing::info!(listener = name, %bind, "starting HTTPS server with file-based TLS");
            axum_server::bind_rustls(bind, tls_config)
                .handle(handle)
                .serve(router.into_make_service_with_connect_info::<std::net::SocketAddr>())
                .await
                .map_err(|error| UdsError::Storage(format!("server failed: {error}")))?;
        }
        TlsMode::Acme => {
            return Err(UdsError::Config(
                "ACME mode is validated in configuration but not wired into the server runtime yet; use tls.mode = \"files\" or terminate TLS at a load balancer for now"
                    .to_string(),
            ));
        }
    }

    Ok(())
}
