use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, UdsError>;

#[derive(Debug, Error)]
pub enum UdsError {
    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("forbidden")]
    Forbidden,

    #[error("fleet confirmation unavailable")]
    FleetUnavailable,

    #[error("payload too large: {0}")]
    PayloadTooLarge(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    TomlDe(#[from] toml::de::Error),

    #[error(transparent)]
    Semver(#[from] semver::Error),
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
    code: &'static str,
    error_id: uuid::Uuid,
}

#[derive(Debug, Clone)]
pub struct ErrorResponseMetadata {
    pub error_id: uuid::Uuid,
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
