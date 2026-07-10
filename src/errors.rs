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
}

impl IntoResponse for UdsError {
    fn into_response(self) -> Response {
        let status = match self {
            UdsError::BadRequest(_) => StatusCode::BAD_REQUEST,
            UdsError::Unauthorized => StatusCode::UNAUTHORIZED,
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

        let body = Json(ErrorBody {
            error: self.to_string(),
        });
        (status, body).into_response()
    }
}
