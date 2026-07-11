use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, header};

use crate::config::LogLevel;
use crate::errors::UdsError;
use crate::logging::{LogEventKind, RequestMetadata};
use crate::routes::AppState;

#[derive(Debug, Clone, Copy)]
pub struct AdminAuth;

#[derive(Debug, Clone, Copy)]
pub struct ClusterAuth;

impl FromRequestParts<AppState> for AdminAuth {
    type Rejection = UdsError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> std::result::Result<Self, Self::Rejection> {
        if let Err(reason) = require_bearer(&parts.headers, &state.config.admin_token) {
            security_failure(parts, state, "admin", reason);
            return Err(UdsError::Unauthorized);
        }
        Ok(Self)
    }
}

impl FromRequestParts<AppState> for ClusterAuth {
    type Rejection = UdsError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> std::result::Result<Self, Self::Rejection> {
        let Some(token) = &state.config.cluster_token else {
            security_failure(parts, state, "cluster", "invalid");
            return Err(UdsError::Unauthorized);
        };
        if let Err(reason) = require_bearer(&parts.headers, token) {
            security_failure(parts, state, "cluster", reason);
            return Err(UdsError::Unauthorized);
        }
        Ok(Self)
    }
}

fn require_bearer(
    headers: &HeaderMap,
    expected_token: &str,
) -> std::result::Result<(), &'static str> {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return Err("missing");
    };

    let Ok(value) = value.to_str() else {
        return Err("malformed");
    };

    let Some(token) = value.strip_prefix("Bearer ") else {
        return Err("malformed");
    };

    if constant_time_eq(token.as_bytes(), expected_token.as_bytes()) {
        Ok(())
    } else {
        Err("invalid")
    }
}

fn security_failure(parts: &Parts, state: &AppState, scope: &str, reason: &str) {
    let request = parts.extensions.get::<RequestMetadata>();
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "security_action".into(),
        serde_json::Value::from("authentication_failed"),
    );
    fields.insert("auth_scope".into(), serde_json::Value::from(scope));
    fields.insert("reason".into(), serde_json::Value::from(reason));
    if let Some(request) = request {
        fields.insert(
            "method".into(),
            serde_json::Value::from(request.method.clone()),
        );
        if let Some(route) = &request.route {
            fields.insert("route".into(), serde_json::Value::from(route.clone()));
        }
    }
    let event = state.logging.event(
        LogLevel::Warn,
        LogEventKind::Security,
        "uds::security",
        request,
        fields,
        "authentication failed",
    );
    state.logging.emit(&event);
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
