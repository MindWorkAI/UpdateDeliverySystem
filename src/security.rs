use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderMap, header};

use crate::auth::{ActorIdentity, verify_owner};
use crate::config::LogLevel;
use crate::errors::UdsError;
use crate::logging::{LogEventKind, RequestMetadata};
use crate::routes::AppState;

#[derive(Debug, Clone)]
pub struct AdminAuth(pub ActorIdentity);

#[derive(Debug, Clone)]
pub struct OwnerAuth(pub ActorIdentity);

#[derive(Debug, Clone, Copy)]
pub struct ClusterAuth;

impl FromRequestParts<AppState> for AdminAuth {
    type Rejection = UdsError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Some(token) = bearer(&parts.headers) else {
            security_failure(parts, state, "admin", "missing");
            return Err(UdsError::Unauthorized);
        };
        if verify_owner(token, &state.config.owner_token_verifier) {
            let actor = ActorIdentity::owner();
            record_actor(parts, &actor);
            return Ok(Self(actor));
        }
        if let Some((actor, enabled)) = state.auth.authenticate(token).await {
            if enabled {
                record_actor(parts, &actor);
                return Ok(Self(actor));
            }
            disabled_token(parts, state, actor.token_id.expect("admin actor has id"));
            security_failure(parts, state, "admin", "disabled");
            return Err(UdsError::Unauthorized);
        }
        security_failure(parts, state, "admin", "invalid");
        Err(UdsError::Unauthorized)
    }
}

impl FromRequestParts<AppState> for OwnerAuth {
    type Rejection = UdsError;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Some(token) = bearer(&parts.headers) else {
            security_failure(parts, state, "owner", "missing");
            return Err(UdsError::Unauthorized);
        };
        if verify_owner(token, &state.config.owner_token_verifier) {
            let actor = ActorIdentity::owner();
            record_actor(parts, &actor);
            return Ok(Self(actor));
        }
        if state.auth.authenticate(token).await.is_some() {
            return Err(UdsError::Forbidden);
        }
        security_failure(parts, state, "owner", "invalid");
        Err(UdsError::Unauthorized)
    }
}

fn record_actor(parts: &Parts, actor: &ActorIdentity) {
    if let Some(request) = parts.extensions.get::<RequestMetadata>()
        && let Ok(mut slot) = request.actor.lock()
    {
        *slot = Some(actor.clone());
    }
}

impl FromRequestParts<AppState> for ClusterAuth {
    type Rejection = UdsError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
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

fn require_bearer(headers: &HeaderMap, expected_token: &str) -> Result<(), &'static str> {
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

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn disabled_token(parts: &Parts, state: &AppState, id: uuid::Uuid) {
    let request = parts.extensions.get::<RequestMetadata>();
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "security_action".into(),
        serde_json::Value::from("disabled_token_used"),
    );
    fields.insert("token_id".into(), serde_json::Value::from(id.to_string()));
    let event = state.logging.event(
        LogLevel::Warn,
        LogEventKind::Security,
        "uds::security",
        request,
        fields,
        "disabled admin token used",
    );
    state.logging.emit(&event);
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
