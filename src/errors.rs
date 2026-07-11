//! Shared UDS error taxonomy and safe HTTP error conversion.
//!
//! Internal failures are mapped to stable status codes without leaking secrets
//! or file-system details to remote callers.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use thiserror::Error;

/// Shared Result type used for consistent UDS error handling.
pub type Result<T> = std::result::Result<T, UdsError>;

#[derive(Debug, Error)]
/// Failures that can propagate across UDS service and route boundaries.
pub enum UdsError {
    /// Represents the item case in UDS.
    #[error("bad request: {0}")]
    BadRequest(String),

    /// Represents the item case in UDS.
    #[error("unauthorized")]
    Unauthorized,

    /// Represents the item case in UDS.
    #[error("forbidden")]
    Forbidden,

    /// Represents the item case in UDS.
    #[error("fleet confirmation unavailable")]
    FleetUnavailable,

    /// Represents the item case in UDS.
    #[error("payload too large: {0}")]
    PayloadTooLarge(String),

    /// Represents the item case in UDS.
    #[error("not found: {0}")]
    NotFound(String),

    /// Represents the item case in UDS.
    #[error("conflict: {0}")]
    Conflict(String),

    /// Represents the item case in UDS.
    #[error("configuration error: {0}")]
    Config(String),

    /// Represents the item case in UDS.
    #[error("storage error: {0}")]
    Storage(String),

    /// Represents the std case in UDS.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// Represents the serde json case in UDS.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// Represents the toml case in UDS.
    #[error(transparent)]
    TomlDe(#[from] toml::de::Error),

    /// Represents the semver case in UDS.
    #[error(transparent)]
    Semver(#[from] semver::Error),
}

#[derive(Serialize)]
/// Private wire wrapper used to serialize a safe HTTP error response.
struct ErrorBody {
    /// Stores the error value used by this UDS component.
    error: String,

    /// Defines the str value used by UDS.
    code: &'static str,

    /// Stores the error id value used by this UDS component.
    error_id: uuid::Uuid,
}

#[derive(Debug, Clone)]
/// Context retained after converting an internal error into an HTTP response.
pub struct ErrorResponseMetadata {
    /// The error id carried by this UDS data contract.
    pub error_id: uuid::Uuid,

    /// The internal carried by this UDS data contract.
    pub internal: bool,
}

impl IntoResponse for UdsError {
    fn into_response(self) -> Response {
        let status = match &self {
            UdsError::BadRequest(_) => StatusCode::BAD_REQUEST,
            UdsError::Unauthorized => StatusCode::UNAUTHORIZED,
            UdsError::Forbidden => StatusCode::FORBIDDEN,
            UdsError::FleetUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            UdsError::PayloadTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            UdsError::NotFound(_) => StatusCode::NOT_FOUND,
            UdsError::Conflict(_) => StatusCode::CONFLICT,
            UdsError::Config(_)
            | UdsError::Storage(_)
            | UdsError::Io(_)
            | UdsError::Json(_)
            | UdsError::TomlDe(_)
            | UdsError::Semver(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let (error, code) = match &self {
            UdsError::BadRequest(message) => (message.clone(), "bad_request"),
            UdsError::Unauthorized => ("unauthorized".into(), "unauthorized"),
            UdsError::Forbidden => ("forbidden".into(), "forbidden"),
            UdsError::FleetUnavailable => (
                "fleet confirmation unavailable".into(),
                "fleet_confirmation_unavailable",
            ),
            UdsError::PayloadTooLarge(message) => (message.clone(), "payload_too_large"),
            UdsError::NotFound(message) => (message.clone(), "not_found"),
            UdsError::Conflict(message) => (message.clone(), "conflict"),
            _ => ("internal server error".into(), "internal_error"),
        };
        let error_id = uuid::Uuid::new_v4();
        if status.is_server_error() {
            tracing::error!(error_id=%error_id, error=%self, "request failed");
        }
        let body = Json(ErrorBody {
            error,
            code,
            error_id,
        });
        let mut response = (status, body).into_response();
        response.extensions_mut().insert(ErrorResponseMetadata {
            error_id,
            internal: status.is_server_error(),
        });
        response
    }
}
