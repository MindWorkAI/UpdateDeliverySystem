use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::cluster::ClusterState;
use crate::config::{ServerConfig, ServerMode};
use crate::errors::{Result, UdsError};
use crate::logging::{
    LoggingRuntime, events_to_ndjson, read_recent_events, stream_events_from_file,
};
use crate::models::{
    CatalogResponse, ChangelogPatchRequest, CopyReleaseRequest, MutationResponse,
    ReleaseUploadMetadata, ReplicationEvent, ReplicationEventType, UploadPolicy,
};
use crate::security::{AdminAuth, ClusterAuth};
use crate::stats::{ChannelStats, StatsEvent, StatsEventKind, StatsRecorder};
use crate::storage::{StagedArtifact, Storage};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<ServerConfig>,
    pub storage: Arc<Storage>,
    pub stats: Arc<StatsRecorder>,
    pub cluster: ClusterState,
    pub logging: Arc<LoggingRuntime>,
}

pub fn build_router(state: AppState) -> Router {
    let upload_policy = state
        .config
        .upload
        .policy()
        .expect("validated upload policy");
    let upload_body_limit = upload_policy
        .max_total_artifact_bytes
        .saturating_add(upload_policy.max_metadata_bytes)
        .saturating_add(1024 * 1024)
        .min(usize::MAX as u64) as usize;
    let mut router = Router::new()
        .route("/health", get(health))
        .route(
            "/api/v1/updates/{channel}/{target}/{arch}/{current_version}",
            get(check_update),
        )
        .route(
            "/api/v1/downloads/{channel}/{version}/{platform}/{file_name}",
            get(download_artifact),
        )
        .route(
            "/admin/v1/channels/{channel}/releases",
            get(list_releases)
                .post(upload_release)
                .layer(DefaultBodyLimit::max(upload_body_limit)),
        )
        .route("/admin/v1/upload-policy", get(get_upload_policy))
        .route(
            "/admin/v1/channels/{channel}/releases/{version}/changelog",
            patch(patch_changelog),
        )
        .route(
            "/admin/v1/channels/{channel}/releases/{version}",
            delete(withdraw_release),
        )
        .route(
            "/admin/v1/channels/{target_channel}/copy",
            post(copy_release),
        )
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
    Path((channel, version, platform, file_name)): Path<(String, String, String, String)>,
) -> Result<Response> {
    require_allowed_channel(&state, &channel)?;
    let (path, artifact_size) = state
        .storage
        .artifact_path(&channel, &version, &platform, &file_name)
        .await?;
    let file = File::open(path).await?;
    let mut file_stream = ReaderStream::new(file);
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
        header::HeaderValue::from_static("application/octet-stream"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        header::HeaderValue::from_static("attachment"),
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        header::HeaderValue::from_str(&artifact_size.to_string())
            .map_err(|error| UdsError::Storage(format!("invalid artifact size header: {error}")))?,
    );
    Ok(response)
}

async fn upload_release(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Path(channel): Path<String>,
    multipart: Multipart,
) -> Result<Json<MutationResponse>> {
    require_allowed_channel(&state, &channel)?;
    let policy = state.config.upload.policy()?;
    let upload =
        read_release_multipart(multipart, state.storage.upload_staging_root(), &policy).await?;
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

    Ok(Json(MutationResponse {
        channel,
        version: manifest.version,
        replicated,
    }))
}

async fn get_upload_policy(
    State(state): State<AppState>,
    _auth: AdminAuth,
) -> Result<Json<UploadPolicy>> {
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
        .replicate_event(replication_event_model(
            &channel,
            &manifest.version,
            ReplicationEventType::ReleaseWithdrawn,
        ))
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
        .replicate_event(replication_event_model(
            &target_channel,
            &manifest.version,
            ReplicationEventType::ReleaseCopied,
        ))
        .await;

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
        [(header::CONTENT_TYPE, "application/x-ndjson")],
        body,
    )
        .into_response())
}

async fn stream_logs(
    State(state): State<AppState>,
    _auth: AdminAuth,
    Query(query): Query<LogQuery>,
) -> Result<Response> {
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

async fn catalog(
    State(state): State<AppState>,
    _auth: ClusterAuth,
) -> Result<Json<CatalogResponse>> {
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
    std::fs::create_dir_all(&staging_root)?;
    let temp_dir = tempfile::Builder::new()
        .prefix("upload-")
        .tempdir_in(staging_root)?;
    let mut metadata = None;
    let mut files = BTreeMap::new();
    let mut total_artifact_bytes = 0u64;

    while let Some(mut field) = multipart.next_field().await.map_err(map_multipart_error)? {
        let name = field.name().map(str::to_string).ok_or_else(|| {
            UdsError::BadRequest("all multipart fields must have a name".to_string())
        })?;

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
                serde_json::from_slice::<ReleaseUploadMetadata>(&bytes).map_err(|error| {
                    UdsError::BadRequest(format!("invalid release metadata: {error}"))
                })?,
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

    let metadata = metadata.ok_or_else(|| {
        UdsError::BadRequest("multipart field 'metadata' is required".to_string())
    })?;
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

fn replication_event_model(
    channel: &str,
    version: &str,
    event_type: ReplicationEventType,
) -> ReplicationEvent {
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

    async fn test_app() -> (Router, Arc<StatsRecorder>, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let mut config = ServerConfig::development_default();
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
        let state = AppState {
            config: Arc::new(config),
            storage: Arc::new(storage),
            stats: stats.clone(),
            cluster,
            logging: Arc::new(LoggingRuntime::disabled()),
        };
        (build_router(state), stats, temp)
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
                    .header(header::AUTHORIZATION, "Bearer change-me-admin-token")
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
        let (router, stats, _temp) = test_app().await;
        let response = upload(router.clone(), b"artifact bytes").await;
        assert_eq!(response.status(), StatusCode::OK);

        let response = router
            .oneshot(
                Request::get("/api/v1/downloads/stable/1.2.3/linux-x86_64/studio.tar.gz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "14");
        assert_eq!(stats.channel_stats("stable").await.unwrap().downloads, 0);
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap(),
            "artifact bytes"
        );
        assert_eq!(stats.channel_stats("stable").await.unwrap().downloads, 1);
    }

    #[tokio::test]
    async fn upload_rejects_artifact_above_policy_limit() {
        let (router, _stats, _temp) = test_app().await;
        let artifact = vec![0u8; 1024 * 1024 + 1];
        let response = upload(router, &artifact).await;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn upload_policy_requires_admin_authentication() {
        let (router, _stats, _temp) = test_app().await;
        let response = router
            .oneshot(
                Request::get("/admin/v1/upload-policy")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
