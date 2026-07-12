//! Deliberately compact route tables for the public, admin, and fleet APIs.
//!
//! Route registrations stay on one line so readers can scan path-to-handler
//! mappings vertically. Rustfmt skips only these small builder functions.

use super::*;

/// Builds the unauthenticated API consumed by update clients.
#[rustfmt::skip]
pub fn build_public_router(state: AppState) -> Router {
    apply_common_layers(
        Router::new()
            .route("/health", get(health))
            .route("/api/v1/updates/{channel}/{target}/{arch}/{current_version}", get(check_update))
            .route("/api/v1/downloads/{channel}/{version}/{platform}/{file_name}", get(download_artifact)),
        state,
    )
}

/// Builds the authenticated API used by UDS administrators and owners.
#[rustfmt::skip]
pub fn build_admin_router(state: AppState) -> Router {
    let upload_policy = state.config.upload.policy().expect("validated upload policy");
    let upload_body_limit = upload_policy
        .max_total_artifact_bytes
        .saturating_add(upload_policy.max_metadata_bytes)
        .saturating_add(1024 * 1024)
        .min(usize::MAX as u64) as usize;

    let mut router = Router::new()
        .route("/health", get(health))
        .route("/admin/v1/channels/{channel}/releases", get(list_releases).post(upload_release).layer(DefaultBodyLimit::max(upload_body_limit)))
        .route("/admin/v1/upload-policy", get(get_upload_policy))
        .route("/admin/v1/channels/{channel}/releases/{version}/changelog", patch(patch_changelog))
        .route("/admin/v1/channels/{channel}/releases/{version}", delete(withdraw_release))
        .route("/admin/v1/channels/{target_channel}/copy", post(copy_release))
        .route("/admin/v1/channels/{channel}/stats", get(channel_stats))
        .route("/admin/v1/admin-tokens", get(list_admin_tokens).post(create_admin_token))
        .route("/admin/v1/admin-tokens/{id}", patch(set_admin_token_status))
        .route("/admin/v1/updates/releases", get(update_releases))
        .route("/admin/v1/updates", post(start_update))
        .route("/admin/v1/updates/{operation_id}", get(update_status));

    if state.config.logging.admin_api.enabled && state.config.logging.file.enabled {
        router = router
            .route("/admin/v1/logs/recent", get(recent_logs))
            .route("/admin/v1/logs/stream", get(stream_logs));
    }

    apply_common_layers(router.layer(middleware::from_fn(no_store_token_responses)), state)
}

/// Builds the private node-to-node API used by a UDS fleet.
#[rustfmt::skip]
pub fn build_fleet_router(state: AppState) -> Router {
    apply_common_layers(
        Router::new()
            .route("/health", get(health))
            .route("/fleet/v1/replication/events", post(replication_event))
            .route("/fleet/v1/auth/admin-tokens", get(fleet_admin_tokens).post(merge_fleet_admin_tokens))
            .route("/fleet/v1/catalog", get(catalog))
            .route("/fleet/v1/stats/local/{channel}", get(local_stats)),
        state,
    )
}
