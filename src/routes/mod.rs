//! HTTP routing, middleware, and endpoint handlers for the three UDS APIs.
//!
//! Public, administrative, and fleet traffic use separate routers so a
//! deployment cannot accidentally expose privileged handlers on the public
//! listener.

mod routers;

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::{Body, HttpBody};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Extension, MatchedPath, Multipart, Path, Query, State};
use axum::http::{HeaderValue, Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use uuid::Uuid;
use zeroize::Zeroize;

use crate::auth::{AdminTokenMetadata, AdminTokenStore, CreatedAdminToken};
use crate::cluster::ClusterState;
use crate::config::LogLevel;
use crate::config::ServerConfig;
use crate::errors::{ErrorResponseMetadata, Result, UdsError};
use crate::logging::{
    LogEventKind, LoggingRuntime, RequestMetadata, events_to_ndjson, read_recent_events, stream_events,
};
use crate::models::{
    CatalogResponse, ChangelogPatchRequest, CopyReleaseRequest, MutationResponse, ReleaseUploadMetadata,
    ReplicationEvent, ReplicationEventType, UploadPolicy,
};
use crate::security::{AdminAuth, ClusterAuth, OwnerAuth};
use crate::shutdown::{ShutdownState, TransferKind};
use crate::stats::{ChannelStats, StatsEvent, StatsEventKind, StatsRecorder};
use crate::storage::{StagedArtifact, Storage};

pub use routers::{build_admin_router, build_fleet_router, build_public_router};

#[derive(Clone)]
/// Shared services required by UDS request handlers.
///
/// Axum clones this lightweight container for requests while the underlying
/// state remains shared through reference-counted services.
pub struct AppState {
    pub config: Arc<ServerConfig>,
    pub storage: Arc<Storage>,
    pub stats: Arc<StatsRecorder>,
    pub cluster: ClusterState,
    pub logging: Arc<LoggingRuntime>,
    pub shutdown: Arc<ShutdownState>,
    pub auth: Arc<AdminTokenStore>,
}

#[derive(serde::Deserialize)]
/// Owner-authorized input for creating one purpose-bound admin token.
struct CreateAdminTokenRequest {
    name: String,
    reason: String,
}

#[derive(serde::Deserialize)]
/// Owner-authorized input for changing an admin token's enabled state.
struct SetAdminTokenStatusRequest {
    enabled: bool,
    reason: String,
}

async fn list_admin_tokens(State(state): State<AppState>, _auth: OwnerAuth) -> Result<Response> {
    no_store(Json(state.auth.list().await).into_response())
}

async fn create_admin_token(
    State(state): State<AppState>,
    _auth: OwnerAuth,
    Extension(request_metadata): Extension<RequestMetadata>,
    Json(request): Json<CreateAdminTokenRequest>,
) -> Result<Response> {
    let (metadata, mut token) = state.auth.create(request.name, request.reason).await?;

    // Do not reveal a newly created credential unless every known peer has the
    // verifier required to authenticate it consistently.
    if !state
        .cluster
        .replicate_auth_snapshot(&state.auth.fleet_snapshot().await)
        .await
    {
        token.zeroize();
        return Err(UdsError::FleetUnavailable);
    }
    emit_token_audit(
        &state,
        &request_metadata,
        "admin_token_created",
        &metadata,
        None,
    );
    no_store(Json(CreatedAdminToken { metadata, token }).into_response())
}

async fn set_admin_token_status(
    State(state): State<AppState>,
    _auth: OwnerAuth,
    Extension(request_metadata): Extension<RequestMetadata>,
    Path(id): Path<Uuid>,
    Json(request): Json<SetAdminTokenStatusRequest>,
) -> Result<Response> {
    let reason = request.reason.clone();
    let metadata = state
        .auth
        .set_enabled(id, request.enabled, request.reason)
        .await?;
    if !state
        .cluster
        .replicate_auth_snapshot(&state.auth.fleet_snapshot().await)
        .await
    {
        return Err(UdsError::FleetUnavailable);
    }
    emit_token_audit(
        &state,
        &request_metadata,
        "admin_token_status_changed",
        &metadata,
        Some(&reason),
    );
    no_store(Json(metadata).into_response())
}

fn no_store(mut response: Response) -> Result<Response> {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn no_store_token_responses(request: Request<Body>, next: Next) -> Response {
    let token_management = request.uri().path().starts_with("/admin/v1/admin-tokens");
    let mut response = next.run(request).await;
    if token_management {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

fn emit_token_audit(
    state: &AppState,
    request: &RequestMetadata,
    action: &str,
    token: &AdminTokenMetadata,
    reason: Option<&str>,
) {
    let mut fields = BTreeMap::new();
    fields.insert("audit_action".into(), serde_json::Value::from(action));
    fields.insert("actor_type".into(), serde_json::Value::from("owner"));
    fields.insert(
        "target_token_id".into(),
        serde_json::Value::from(token.id.to_string()),
    );
    fields.insert(
        "target_token_name".into(),
        serde_json::Value::from(token.name.clone()),
    );
    fields.insert("enabled".into(), serde_json::Value::from(token.enabled));
    if let Some(reason) = reason {
        fields.insert("reason".into(), serde_json::Value::from(reason));
    }
    let event = state.logging.event(
        LogLevel::Info,
        LogEventKind::Audit,
        "uds::audit",
        Some(request),
        fields,
        "admin token lifecycle changed",
    );
    state.logging.emit(&event);
}

fn apply_common_layers(router: Router<AppState>, state: AppState) -> Router {
    router
        .layer(tower_http::catch_panic::CatchPanicLayer::custom(|_| {
            UdsError::Storage("request handler panicked".into()).into_response()
        }))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            request_logging,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            reject_during_shutdown,
        ))
        .with_state(state)
}

async fn reject_during_shutdown(State(state): State<AppState>, request: Request<Body>, next: Next) -> Response {
    if !state.shutdown.is_draining() {
        return next.run(request).await;
    }

    let status = if request.uri().path() == "/health" {
        "draining"
    } else {
        "service_unavailable"
    };
    let mut response = (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({ "status": status })),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
    response
}

async fn request_logging(State(state): State<AppState>, mut request: Request<Body>, next: Next) -> Response {
    //
    // Attach one request ID and an actor slot before authentication extractors
    // and handlers run, allowing every later event to share the same context.
    //
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|v| {
            !v.is_empty()
                && v.len() <= 128
                && v.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let socket_ip = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|v| v.0.ip());
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string());
    let metadata = RequestMetadata {
        request_id: request_id.clone(),
        socket_ip,
        method: request.method().to_string(),
        route,
        actor: Default::default(),
    };
    request.extensions_mut().insert(metadata.clone());
    let started = std::time::Instant::now();

    // Run the next middleware or handler and capture the response:
    let mut response = next.run(request).await;
    let status = response.status();
    if let Some(error) = response.extensions().get::<ErrorResponseMetadata>()
        && error.internal
    {
        let mut error_fields = BTreeMap::new();
        error_fields.insert(
            "error_id".into(),
            serde_json::Value::from(error.error_id.to_string()),
        );
        let event = state.logging.event(
            LogLevel::Error,
            LogEventKind::System,
            "uds::error",
            Some(&metadata),
            error_fields,
            "request failed internally",
        );
        state.logging.emit(&event);
    }
    let route = metadata
        .route
        .clone()
        .unwrap_or_else(|| "<unmatched>".into());
    let mut fields = BTreeMap::new();
    fields.insert(
        "method".into(),
        serde_json::Value::from(metadata.method.clone()),
    );
    fields.insert("route".into(), serde_json::Value::from(route));
    fields.insert("status".into(), serde_json::Value::from(status.as_u16()));
    fields.insert(
        "latency_ms".into(),
        serde_json::Value::from(started.elapsed().as_millis() as u64),
    );
    if let Ok(actor) = metadata.actor.lock()
        && let Some(actor) = actor.as_ref()
    {
        fields.insert(
            "actor_type".into(),
            serde_json::Value::from(match actor.actor_type {
                crate::auth::ActorType::Owner => "owner",
                crate::auth::ActorType::Admin => "admin",
            }),
        );
        if let Some(id) = actor.token_id {
            fields.insert(
                "actor_token_id".into(),
                serde_json::Value::from(id.to_string()),
            );
        }
        if let Some(name) = &actor.token_name {
            fields.insert(
                "actor_token_name".into(),
                serde_json::Value::from(name.clone()),
            );
        }
    }
    if let Some(size) = response.body().size_hint().exact() {
        fields.insert("response_size".into(), serde_json::Value::from(size));
    }
    let level = if status.is_server_error() {
        LogLevel::Error
    } else if status.is_client_error() {
        LogLevel::Warn
    } else if metadata.route.as_deref() == Some("/health") {
        LogLevel::Debug
    } else {
        LogLevel::Info
    };
    let event = state.logging.event(
        level,
        LogEventKind::Http,
        "uds::http",
        Some(&metadata),
        fields,
        "request completed",
    );
    state.logging.emit(&event);
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}

async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let _ = state;
    Json(serde_json::json!({ "status": "ok" }))
}

async fn check_update(
    State(state): State<AppState>,
    Path((channel, target, arch, current_version)): Path<(String, String, String, String)>,
) -> Result<Response> {
    require_allowed_channel(&state, &channel)?;
    let update = state
        .storage
        .update_for(&channel, &target, &arch, &current_version)
        .await?;
    state.stats.record(StatsEvent {
        kind: StatsEventKind::UpdateCheck,
        channel: channel.clone(),
        version: None,
        target: Some(target.clone()),
        arch: Some(arch.clone()),
        bytes: 0,
    });

    match update {
        Some(update) => Ok(Json(update).into_response()),
        None => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

async fn download_artifact(
    State(state): State<AppState>,
    Extension(request): Extension<RequestMetadata>,
    Path((channel, version, platform, file_name)): Path<(String, String, String, String)>,
) -> Result<Response> {
    require_allowed_channel(&state, &channel)?;
    let (path, artifact_size) = state
        .storage
        .artifact_path(&channel, &version, &platform, &file_name)
        .await?;
    let file = File::open(path).await?;
    let mut file_stream = ReaderStream::new(file);
    let mut transfer_fields = BTreeMap::new();
    transfer_fields.insert("channel".into(), serde_json::Value::from(channel.clone()));
    transfer_fields.insert("version".into(), serde_json::Value::from(version.clone()));
    transfer_fields.insert("platform".into(), serde_json::Value::from(platform.clone()));
    transfer_fields.insert(
        "file_name".into(),
        serde_json::Value::from(file_name.clone()),
    );
    transfer_fields.insert("size".into(), serde_json::Value::from(artifact_size));
    let transfer = state
        .shutdown
        .start_transfer(TransferKind::Download, request.request_id, transfer_fields);
    let (target, arch) = platform
        .split_once('-')
        .map(|(target, arch)| (Some(target.to_string()), Some(arch.to_string())))
        .unwrap_or((None, None));

    let stats = state.stats.clone();
    let event = StatsEvent {
        kind: StatsEventKind::Download,
        channel,
        version: Some(version),
        target,
        arch,
        bytes: artifact_size,
    };
    let stream = async_stream::stream! {
        let _transfer = transfer;
        while let Some(chunk) = file_stream.next().await {
            match chunk {
                Ok(bytes) => yield Ok::<_, std::io::Error>(bytes),
                Err(error) => {
                    yield Err(error);
                    return;
                }
            }
        }
        stats.record(event);
    };
    let body = Body::from_stream(stream);

    let mut response = (StatusCode::OK, body).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment"),
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&artifact_size.to_string())
            .map_err(|error| UdsError::Storage(format!("invalid artifact size header: {error}")))?,
    );
    Ok(response)
}

async fn upload_release(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Extension(request): Extension<RequestMetadata>,
    Path(channel): Path<String>,
    multipart: Multipart,
) -> Result<Json<MutationResponse>> {
    require_allowed_channel(&state, &channel)?;
    let policy = state.config.upload.policy()?;
    let mut transfer_fields = BTreeMap::new();
    transfer_fields.insert("channel".into(), serde_json::Value::from(channel.clone()));
    let transfer = state.shutdown.start_transfer(
        TransferKind::Upload,
        request.request_id.clone(),
        transfer_fields,
    );
    let upload = read_release_multipart(multipart, state.storage.upload_staging_root(), &policy).await?;
    transfer.set_field("version", upload.metadata.version.clone());
    let manifest = state
        .storage
        .put_release(&channel, upload.metadata, upload.files, &policy)
        .await?;
    let replicated = state
        .cluster
        .replicate_event(replication_event_model(
            &channel,
            &manifest.version,
            ReplicationEventType::ReleaseUploaded,
        ))
        .await;

    let mut fields = BTreeMap::new();
    fields.insert(
        "audit_action".into(),
        serde_json::Value::from("release_uploaded"),
    );
    fields.insert("channel".into(), serde_json::Value::from(channel.clone()));
    fields.insert(
        "version".into(),
        serde_json::Value::from(manifest.version.clone()),
    );
    fields.insert(
        "platform_count".into(),
        serde_json::Value::from(manifest.platforms.len()),
    );
    fields.insert(
        "total_size".into(),
        serde_json::Value::from(
            manifest
                .platforms
                .values()
                .map(|artifact| artifact.size)
                .sum::<u64>(),
        ),
    );
    let event = state.logging.event(
        LogLevel::Info,
        LogEventKind::Audit,
        "uds::audit",
        Some(&request),
        fields,
        "release uploaded",
    );
    state.logging.emit(&event);
    Ok(Json(MutationResponse {
        channel,
        version: manifest.version,
        replicated,
    }))
}

async fn get_upload_policy(State(state): State<AppState>, _auth: AdminAuth) -> Result<Json<UploadPolicy>> {
    Ok(Json(state.config.upload.policy()?))
}

async fn list_releases(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Path(channel): Path<String>,
) -> Result<Json<crate::models::ReleaseListResponse>> {
    require_allowed_channel(&state, &channel)?;
    Ok(Json(state.storage.release_list(&channel).await?))
}

async fn patch_changelog(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Extension(request_metadata): Extension<RequestMetadata>,
    Path((channel, version)): Path<(String, String)>,
    Json(request): Json<ChangelogPatchRequest>,
) -> Result<Json<MutationResponse>> {
    require_allowed_channel(&state, &channel)?;
    let manifest = state
        .storage
        .patch_changelog(&channel, &version, request.notes)
        .await?;
    let replicated = state
        .cluster
        .replicate_event(replication_event_model(
            &channel,
            &manifest.version,
            ReplicationEventType::ChangelogPatched,
        ))
        .await;

    emit_audit(
        &state,
        &request_metadata,
        "changelog_updated",
        &channel,
        &manifest.version,
        None,
    );
    Ok(Json(MutationResponse {
        channel,
        version: manifest.version,
        replicated,
    }))
}

async fn withdraw_release(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Extension(request_metadata): Extension<RequestMetadata>,
    Path((channel, version)): Path<(String, String)>,
) -> Result<Json<MutationResponse>> {
    require_allowed_channel(&state, &channel)?;
    let manifest = state.storage.withdraw_release(&channel, &version).await?;
    let replicated = state
        .cluster
        .replicate_event(replication_event_model(
            &channel,
            &manifest.version,
            ReplicationEventType::ReleaseWithdrawn,
        ))
        .await;

    emit_audit(
        &state,
        &request_metadata,
        "release_withdrawn",
        &channel,
        &manifest.version,
        None,
    );
    Ok(Json(MutationResponse {
        channel,
        version: manifest.version,
        replicated,
    }))
}

async fn copy_release(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Extension(request_metadata): Extension<RequestMetadata>,
    Path(target_channel): Path<String>,
    Json(request): Json<CopyReleaseRequest>,
) -> Result<Json<MutationResponse>> {
    require_allowed_channel(&state, &target_channel)?;
    require_allowed_channel(&state, &request.source_channel)?;
    let manifest = state
        .storage
        .copy_release(&request.source_channel, &target_channel, &request.version)
        .await?;
    let replicated = state
        .cluster
        .replicate_event(replication_event_model(
            &target_channel,
            &manifest.version,
            ReplicationEventType::ReleaseCopied,
        ))
        .await;

    emit_audit(
        &state,
        &request_metadata,
        "release_copied",
        &target_channel,
        &manifest.version,
        Some(&request.source_channel),
    );
    Ok(Json(MutationResponse {
        channel: target_channel,
        version: manifest.version,
        replicated,
    }))
}

async fn channel_stats(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Path(channel): Path<String>,
) -> Result<Json<ChannelStats>> {
    require_allowed_channel(&state, &channel)?;
    Ok(Json(state.stats.channel_stats(&channel).await?))
}

#[derive(Debug, serde::Deserialize)]
/// Optional filters accepted by the administrative log endpoints.
struct LogQuery {
    lines: Option<usize>,
}

async fn recent_logs(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Query(query): Query<LogQuery>,
) -> Result<Response> {
    let path = state
        .logging
        .active_file_path()
        .ok_or_else(|| UdsError::Config("file logging is disabled".to_string()))?;
    let events = read_recent_events(path, query.lines.unwrap_or(200).min(10_000)).await?;
    let body = events_to_ndjson(&events)?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/x-ndjson"),
            (header::CACHE_CONTROL, "no-store, no-transform"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        body,
    )
        .into_response())
}

async fn stream_logs(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Query(query): Query<LogQuery>,
) -> Result<Response> {
    state
        .logging
        .active_file_path()
        .ok_or_else(|| UdsError::Config("file logging is disabled".to_string()))?;
    let stream = stream_events(
        state.logging.clone(),
        query.lines.unwrap_or(100).min(10_000),
        state.shutdown.clone(),
    );
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/x-ndjson"),
            (header::CACHE_CONTROL, "no-store, no-transform"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        Body::from_stream(stream),
    )
        .into_response())
}

fn emit_audit(
    state: &AppState,
    request: &RequestMetadata,
    action: &str,
    channel: &str,
    version: &str,
    source_channel: Option<&str>,
) {
    let mut fields = BTreeMap::new();
    fields.insert("audit_action".into(), serde_json::Value::from(action));
    fields.insert("channel".into(), serde_json::Value::from(channel));
    fields.insert("version".into(), serde_json::Value::from(version));
    if let Some(source) = source_channel {
        fields.insert("source_channel".into(), serde_json::Value::from(source));
    }
    let event = state.logging.event(
        LogLevel::Info,
        LogEventKind::Audit,
        "uds::audit",
        Some(request),
        fields,
        action,
    );
    state.logging.emit(&event);
}

async fn catalog(State(state): State<AppState>, _auth: ClusterAuth) -> Result<Json<CatalogResponse>> {
    Ok(Json(state.storage.catalog().await?))
}

async fn local_stats(
    State(state): State<AppState>,
    _auth: ClusterAuth,
    Path(channel): Path<String>,
) -> Result<Json<ChannelStats>> {
    require_allowed_channel(&state, &channel)?;
    Ok(Json(state.stats.channel_stats(&channel).await?))
}

async fn replication_event(_auth: ClusterAuth, Json(_event): Json<ReplicationEvent>) -> StatusCode {
    StatusCode::ACCEPTED
}

async fn fleet_admin_tokens(
    State(state): State<AppState>,
    _auth: ClusterAuth,
) -> Json<Vec<crate::auth::AdminTokenRecord>> {
    Json(state.auth.fleet_snapshot().await)
}

async fn merge_fleet_admin_tokens(
    State(state): State<AppState>,
    _auth: ClusterAuth,
    Json(records): Json<Vec<crate::auth::AdminTokenRecord>>,
) -> Result<StatusCode> {
    state.auth.merge(records).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Fully streamed multipart upload ready for semantic validation and publishing.
struct StagedMultipart {
    _temp_dir: tempfile::TempDir,
    metadata: ReleaseUploadMetadata,
    files: BTreeMap<String, StagedArtifact>,
}

async fn read_release_multipart(
    mut multipart: Multipart,
    staging_root: std::path::PathBuf,
    policy: &UploadPolicy,
) -> Result<StagedMultipart> {
    //
    // Stream multipart fields to a temporary directory while enforcing limits.
    // No artifact is published until metadata and all digests are validated.
    //
    std::fs::create_dir_all(&staging_root)?;
    let temp_dir = tempfile::Builder::new()
        .prefix("upload-")
        .tempdir_in(staging_root)?;
    let mut metadata = None;
    let mut files = BTreeMap::new();
    let mut total_artifact_bytes = 0u64;

    while let Some(mut field) = multipart.next_field().await.map_err(map_multipart_error)? {
        let name = field
            .name()
            .map(str::to_string)
            .ok_or_else(|| UdsError::BadRequest("all multipart fields must have a name".to_string()))?;

        if name == "metadata" {
            if metadata.is_some() {
                return Err(UdsError::BadRequest(
                    "multipart field 'metadata' must occur exactly once".to_string(),
                ));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = field.chunk().await.map_err(map_multipart_error)? {
                if bytes.len().saturating_add(chunk.len()) as u64 > policy.max_metadata_bytes {
                    return Err(UdsError::PayloadTooLarge(
                        "release metadata exceeds the configured limit".to_string(),
                    ));
                }
                bytes.extend_from_slice(&chunk);
            }
            metadata = Some(
                serde_json::from_slice::<ReleaseUploadMetadata>(&bytes)
                    .map_err(|error| UdsError::BadRequest(format!("invalid release metadata: {error}")))?,
            );
        } else {
            if files.contains_key(&name) {
                return Err(UdsError::BadRequest(format!(
                    "duplicate multipart file field '{name}'"
                )));
            }
            if files.len() >= policy.max_platforms {
                return Err(UdsError::BadRequest(
                    "multipart body contains too many artifact fields".to_string(),
                ));
            }
            let path = temp_dir.path().join(Uuid::new_v4().to_string());
            let mut file = File::create(&path).await?;
            let mut size = 0u64;
            let mut hasher = Sha256::new();
            while let Some(chunk) = field.chunk().await.map_err(map_multipart_error)? {
                size = size.saturating_add(chunk.len() as u64);
                total_artifact_bytes = total_artifact_bytes.saturating_add(chunk.len() as u64);
                if size > policy.max_artifact_bytes {
                    return Err(UdsError::PayloadTooLarge(format!(
                        "multipart file field '{name}' exceeds the configured limit"
                    )));
                }
                if total_artifact_bytes > policy.max_total_artifact_bytes {
                    return Err(UdsError::PayloadTooLarge(
                        "release artifacts exceed the configured total limit".to_string(),
                    ));
                }
                file.write_all(&chunk).await?;
                hasher.update(&chunk);
            }
            file.flush().await?;
            file.sync_all().await?;
            files.insert(
                name.clone(),
                StagedArtifact {
                    field_name: name,
                    path,
                    size,
                    sha256: hex::encode(hasher.finalize()),
                },
            );
        }
    }

    let metadata =
        metadata.ok_or_else(|| UdsError::BadRequest("multipart field 'metadata' is required".to_string()))?;
    Ok(StagedMultipart {
        _temp_dir: temp_dir,
        metadata,
        files,
    })
}

fn map_multipart_error(error: axum::extract::multipart::MultipartError) -> UdsError {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        UdsError::PayloadTooLarge("multipart request exceeds the configured limit".to_string())
    } else {
        UdsError::BadRequest(format!("invalid multipart body: {error}"))
    }
}

fn require_allowed_channel(state: &AppState, channel: &str) -> Result<()> {
    if state.config.channel_is_allowed(channel) {
        Ok(())
    } else {
        Err(UdsError::NotFound(format!(
            "channel {channel} is not configured"
        )))
    }
}

fn replication_event_model(channel: &str, version: &str, event_type: ReplicationEventType) -> ReplicationEvent {
    ReplicationEvent {
        event_id: Uuid::new_v4().to_string(),
        event_type,
        channel: channel.to_string(),
        version: version.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn test_app() -> (
        Router,
        Router,
        Arc<StatsRecorder>,
        Arc<ShutdownState>,
        tempfile::TempDir,
        AppState,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let mut config = ServerConfig::test_default();
        config.data_dir = temp.path().to_path_buf();
        config.logging.file.enabled = false;
        config.logging.admin_api.enabled = false;
        config.upload.max_artifact_size_mb = 1;
        config.upload.max_total_artifact_size_mb = 1;
        config.stats.queue_capacity = 32;
        config.stats.max_pending_events = 100;
        config.stats.rollup_trigger_events = 10;
        config.stats.rollup_interval_seconds = 3600;
        let storage = Storage::new(config.data_dir.clone(), config.public_base_url.clone())
            .await
            .unwrap();
        let stats = Arc::new(
            StatsRecorder::new(config.data_dir.clone(), config.stats.clone())
                .await
                .unwrap(),
        );
        let cluster = ClusterState::new(&config).await.unwrap();
        let shutdown = Arc::new(ShutdownState::default());
        let auth = Arc::new(AdminTokenStore::open(&config.data_dir).await.unwrap());
        let state = AppState {
            config: Arc::new(config),
            storage: Arc::new(storage),
            stats: stats.clone(),
            cluster,
            logging: Arc::new(LoggingRuntime::disabled()),
            shutdown: shutdown.clone(),
            auth,
        };
        let public = build_public_router(state.clone());
        let admin = build_admin_router(state.clone());
        (public, admin, stats, shutdown, temp, state)
    }

    fn multipart_body(artifact: &[u8]) -> (String, Vec<u8>) {
        let boundary = "uds-test-boundary";
        let metadata = serde_json::json!({
            "version": "1.2.3",
            "pub_date": "2026-07-06T18:35:11Z",
            "notes": "notes",
            "platforms": {
                "linux-x86_64": {
                    "file_field": "artifact_0",
                    "file_name": "studio.tar.gz",
                    "signature": "signature"
                }
            }
        });
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"artifact_0\"; filename=\"studio.tar.gz\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        ).into_bytes();
        body.extend_from_slice(artifact);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        (boundary.to_string(), body)
    }

    async fn upload(router: Router, artifact: &[u8]) -> Response {
        let (boundary, body) = multipart_body(artifact);
        router
            .oneshot(
                Request::post("/admin/v1/channels/stable/releases")
                    .header(
                        header::AUTHORIZATION,
                        "Bearer uds_owner_v1_test-only-owner-token",
                    )
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn upload_streams_into_blob_storage_and_download_counts_on_eof() {
        let (public, admin, stats, shutdown, _temp, _state) = test_app().await;
        let response = upload(admin, b"artifact bytes").await;
        assert_eq!(response.status(), StatusCode::OK);

        let response = public
            .oneshot(
                Request::get("/api/v1/downloads/stable/1.2.3/linux-x86_64/studio.tar.gz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "14");
        assert_eq!(shutdown.active_count(), 1);
        assert_eq!(stats.channel_stats("stable").await.unwrap().downloads, 0);
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap(),
            "artifact bytes"
        );
        assert_eq!(shutdown.active_count(), 0);
        assert_eq!(stats.channel_stats("stable").await.unwrap().downloads, 1);
    }

    #[tokio::test]
    async fn aborted_download_is_untracked_without_recording_stats() {
        let (public, admin, stats, shutdown, _temp, _state) = test_app().await;
        assert_eq!(
            upload(admin, b"artifact bytes").await.status(),
            StatusCode::OK
        );

        let response = public
            .oneshot(
                Request::get("/api/v1/downloads/stable/1.2.3/linux-x86_64/studio.tar.gz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(shutdown.active_count(), 1);
        drop(response);
        assert_eq!(shutdown.active_count(), 0);
        assert_eq!(stats.channel_stats("stable").await.unwrap().downloads, 0);
    }

    #[tokio::test]
    async fn upload_rejects_artifact_above_policy_limit() {
        let (_public, admin, _stats, _shutdown, _temp, _state) = test_app().await;
        let artifact = vec![0u8; 1024 * 1024 + 1];
        let response = upload(admin, &artifact).await;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn upload_policy_requires_admin_authentication() {
        let (_public, admin, _stats, _shutdown, _temp, _state) = test_app().await;
        let response = admin
            .oneshot(
                Request::get("/admin/v1/upload-policy")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn owner_manages_tokens_and_admin_cannot_use_owner_api() {
        let (_public, admin, _stats, _shutdown, _temp, _state) = test_app().await;
        let create = admin
            .clone()
            .oneshot(
                Request::post("/admin/v1/admin-tokens")
                    .header(
                        header::AUTHORIZATION,
                        "Bearer uds_owner_v1_test-only-owner-token",
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"name":"Thorsten","reason":"daily administration"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::OK);
        assert_eq!(create.headers()[header::CACHE_CONTROL], "no-store");
        let body = to_bytes(create.into_body(), 8192).await.unwrap();
        let created: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let token = created["token"].as_str().unwrap();
        assert!(token.starts_with("uds_admin_v1_"));
        assert_eq!(created.as_object().unwrap().len(), 2);
        assert_eq!(created["metadata"]["name"], "Thorsten");
        assert!(created.get("id").is_none());
        assert!(created.get("verifier").is_none());
        assert!(created["metadata"].get("verifier").is_none());

        let forbidden = admin
            .clone()
            .oneshot(
                Request::get("/admin/v1/admin-tokens")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

        let normal = admin
            .oneshot(
                Request::get("/admin/v1/upload-policy")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(normal.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn health_returns_service_unavailable_while_draining() {
        let (public, _admin, _stats, shutdown, _temp, _state) = test_app().await;
        let healthy = public
            .clone()
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(healthy.status(), StatusCode::OK);

        assert!(shutdown.begin_draining());
        let draining = public
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(draining.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(draining.into_body(), 1024).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"status": "draining"})
        );
    }

    #[tokio::test]
    async fn listeners_expose_only_their_own_routes_and_no_internal_aliases() {
        let (_public, _admin, _stats, _shutdown, _temp, state) = test_app().await;
        let public = build_public_router(state.clone());
        let admin = build_admin_router(state.clone());
        let fleet = build_fleet_router(state);
        for (router, foreign) in [
            (public, "/admin/v1/upload-policy"),
            (admin, "/api/v1/updates/stable/linux/x86/1.0.0"),
            (fleet, "/internal/v1/catalog"),
        ] {
            let response = router
                .oneshot(Request::get(foreign).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
    }
}
