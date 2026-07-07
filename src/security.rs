use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, header};

use crate::errors::{Result, UdsError};
use crate::routes::AppState;

#[derive(Debug, Clone, Copy)]
pub struct AdminAuth;

#[derive(Debug, Clone, Copy)]
pub struct ClusterAuth;

impl FromRequestParts<AppState> for AdminAuth {
    type Rejection = UdsError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> std::result::Result<Self, Self::Rejection> {
        require_bearer(parts.headers.clone(), &state.config.admin_token)?;
        Ok(Self)
    }
}

impl FromRequestParts<AppState> for ClusterAuth {
    type Rejection = UdsError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> std::result::Result<Self, Self::Rejection> {
        let Some(token) = &state.config.cluster_token else {
            return Err(UdsError::Unauthorized);
        };
        require_bearer(parts.headers.clone(), token)?;
        Ok(Self)
    }
}

fn require_bearer(headers: HeaderMap, expected_token: &str) -> Result<()> {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return Err(UdsError::Unauthorized);
    };

    let Ok(value) = value.to_str() else {
        return Err(UdsError::Unauthorized);
    };

    let Some(token) = value.strip_prefix("Bearer ") else {
        return Err(UdsError::Unauthorized);
    };

    if constant_time_eq(token.as_bytes(), expected_token.as_bytes()) {
        Ok(())
    } else {
        Err(UdsError::Unauthorized)
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }

    let mut diff = 0u8;
    for (left, right) in left.iter().zip(right.iter()) {
        diff |= left ^ right;
    }
    diff == 0
}
