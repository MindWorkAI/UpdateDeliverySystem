use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use bytes::Bytes;
use tokio::fs::File;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::cluster::ClusterState;
use crate::config::{ServerConfig, ServerMode};
use crate::errors::{Result, UdsError};
use crate::logging::{LoggingRuntime, events_to_ndjson, read_recent_events, stream_events_from_file};
use crate::models::{
    CatalogResponse, ChangelogPatchRequest, CopyReleaseRequest, MutationResponse, ReleaseUploadMetadata, ReplicationEvent,
    ReplicationEventType,
};
use crate::security::{AdminAuth, ClusterAuth};
use crate::stats::{ChannelStats, StatsEvent, StatsEventKind, StatsRecorder};
use crate::storage::Storage;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<ServerConfig>,
    pub storage: Arc<Storage>,
    pub stats: Arc<StatsRecorder>,
    pub cluster: ClusterState,
    pub logging: Arc<LoggingRuntime>,
}

pub fn build_router(state: AppState) -> Router {
    let mut router = Router::new()
        .route("/health", get(health))
        .route("/api/v1/updates/{channel}/{target}/{arch}/{current_version}", get(check_update))
        .route("/api/v1/downloads/{channel}/{version}/{platform}/{file_name}", get(download_artifact))
        .route("/admin/v1/channels/{channel}/releases", get(list_releases).post(upload_release))
        .route("/admin/v1/channels/{channel}/releases/{version}/changelog", patch(patch_changelog))
        .route("/admin/v1/channels/{channel}/releases/{version}", delete(withdraw_release))
        .route("/admin/v1/channels/{target_channel}/copy", post(copy_release))
        .route("/admin/v1/channels/{channel}/stats", get(channel_stats));

    if state.config.logging.admin_api.enabled && state.config.logging.file.enabled {
        router = router
            .route("/admin/v1/logs/recent", get(recent_logs))
            .route("/admin/v1/logs/stream", get(stream_logs));
    }

    if state.config.mode == ServerMode::Fleet {
        router = router
            .route("/internal/v1/replication/events", post(replication_event))
            .route("/internal/v1/catalog", get(catalog))
            .route("/internal/v1/stats/local/{channel}", get(local_stats));
    }

    router.with_state(state)
}

async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "mode": state.config.mode,
        "node_id": state.cluster.node_id(),
    }))
}

async fn check_update(
    State(state): State<AppState>,
    Path((channel, target, arch, current_version)): Path<(String, String, String, String)>,
) -> Result<Response> {
    require_allowed_channel(&state, &channel)?;
    state
        .stats
        .record(StatsEvent {
            kind: StatsEventKind::UpdateCheck,
            channel: channel.clone(),
            version: None,
            target: Some(target.clone()),
            arch: Some(arch.clone()),
            bytes: 0,
        })
        .await?;

    match state.storage.update_for(&channel, &target, &arch, &current_version).await? {
        Some(update) => Ok(Json(update).into_response()),
        None => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

async fn download_artifact(
    State(state): State<AppState>,
    Path((channel, version, platform, file_name)): Path<(String, String, String, String)>,
) -> Result<Response> {
    require_allowed_channel(&state, &channel)?;
    let path = state.storage.artifact_path(&channel, &version, &platform, &file_name).await?;
    let file = File::open(path).await?;
    let metadata = file.metadata().await?;
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);
    let (target, arch) = platform
        .split_once('-')
        .map(|(target, arch)| (Some(target.to_string()), Some(arch.to_string())))
        .unwrap_or((None, None));

    state
        .stats
        .record(StatsEvent {
            kind: StatsEventKind::Download,
            channel,
            version: Some(version),
            target,
            arch,
            bytes: metadata.len(),
        })
        .await?;

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CONTENT_DISPOSITION, "attachment"),
        ],
        body,
    )
        .into_response())
}

async fn upload_release(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Path(channel): Path<String>,
    multipart: Multipart,
) -> Result<Json<MutationResponse>> {
    require_allowed_channel(&state, &channel)?;
    let (metadata, files) = read_release_multipart(multipart).await?;
    let manifest = state.storage.put_release(&channel, metadata, files).await?;
    let replicated = state
        .cluster
        .replicate_event(replication_event_model(&channel, &manifest.version, ReplicationEventType::ReleaseUploaded))
        .await;

    Ok(Json(MutationResponse {
        channel,
        version: manifest.version,
        replicated,
    }))
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
    Path((channel, version)): Path<(String, String)>,
    Json(request): Json<ChangelogPatchRequest>,
) -> Result<Json<MutationResponse>> {
    require_allowed_channel(&state, &channel)?;
    let manifest = state.storage.patch_changelog(&channel, &version, request.notes).await?;
    let replicated = state
        .cluster
        .replicate_event(replication_event_model(&channel, &manifest.version, ReplicationEventType::ChangelogPatched))
        .await;

    Ok(Json(MutationResponse {
        channel,
        version: manifest.version,
        replicated,
    }))
}

async fn withdraw_release(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Path((channel, version)): Path<(String, String)>,
) -> Result<Json<MutationResponse>> {
    require_allowed_channel(&state, &channel)?;
    let manifest = state.storage.withdraw_release(&channel, &version).await?;
    let replicated = state
        .cluster
        .replicate_event(replication_event_model(&channel, &manifest.version, ReplicationEventType::ReleaseWithdrawn))
        .await;

    Ok(Json(MutationResponse {
        channel,
        version: manifest.version,
        replicated,
    }))
}

async fn copy_release(
    State(state): State<AppState>,
    _auth: AdminAuth,
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
        .replicate_event(replication_event_model(&target_channel, &manifest.version, ReplicationEventType::ReleaseCopied))
        .await;

    Ok(Json(MutationResponse {
        channel: target_channel,
        version: manifest.version,
        replicated,
    }))
}

async fn channel_stats(State(state): State<AppState>, _auth: AdminAuth, Path(channel): Path<String>) -> Result<Json<ChannelStats>> {
    require_allowed_channel(&state, &channel)?;
    Ok(Json(state.stats.channel_stats(&channel).await?))
}

#[derive(Debug, serde::Deserialize)]
struct LogQuery {
    lines: Option<usize>,
}

async fn recent_logs(State(state): State<AppState>, _auth: AdminAuth, Query(query): Query<LogQuery>) -> Result<Response> {
    let path = state
        .logging
        .active_file_path()
        .ok_or_else(|| UdsError::Config("file logging is disabled".to_string()))?;
    let events = read_recent_events(path, query.lines.unwrap_or(200).min(10_000)).await?;
    let body = events_to_ndjson(&events)?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        body,
    )
        .into_response())
}

async fn stream_logs(State(state): State<AppState>, _auth: AdminAuth, Query(query): Query<LogQuery>) -> Result<Response> {
    let path = state
        .logging
        .active_file_path()
        .ok_or_else(|| UdsError::Config("file logging is disabled".to_string()))?
        .to_path_buf();
    let stream = stream_events_from_file(path, query.lines.unwrap_or(100).min(10_000)).await;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        Body::from_stream(stream),
    )
        .into_response())
}

async fn catalog(State(state): State<AppState>, _auth: ClusterAuth) -> Result<Json<CatalogResponse>> {
    Ok(Json(state.storage.catalog().await?))
}

async fn local_stats(State(state): State<AppState>, _auth: ClusterAuth, Path(channel): Path<String>) -> Result<Json<ChannelStats>> {
    require_allowed_channel(&state, &channel)?;
    Ok(Json(state.stats.channel_stats(&channel).await?))
}

async fn replication_event(_auth: ClusterAuth, Json(_event): Json<ReplicationEvent>) -> StatusCode {
    StatusCode::ACCEPTED
}

async fn read_release_multipart(mut multipart: Multipart) -> Result<(ReleaseUploadMetadata, BTreeMap<String, Bytes>)> {
    let mut metadata = None;
    let mut files = BTreeMap::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| UdsError::BadRequest(format!("invalid multipart body: {error}")))?
    {
        let Some(name) = field.name().map(str::to_string) else {
            continue;
        };

        let bytes = field
            .bytes()
            .await
            .map_err(|error| UdsError::BadRequest(format!("invalid multipart field '{name}': {error}")))?;

        if name == "metadata" {
            metadata = Some(serde_json::from_slice::<ReleaseUploadMetadata>(&bytes)?);
        } else {
            files.insert(name, bytes);
        }
    }

    let metadata = metadata.ok_or_else(|| UdsError::BadRequest("multipart field 'metadata' is required".to_string()))?;
    Ok((metadata, files))
}

fn require_allowed_channel(state: &AppState, channel: &str) -> Result<()> {
    if state.config.channel_is_allowed(channel) {
        Ok(())
    } else {
        Err(UdsError::NotFound(format!("channel {channel} is not configured")))
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
