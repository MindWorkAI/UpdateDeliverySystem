use std::path::Path;

use reqwest::multipart::{Form, Part};

use crate::client::config::ClientProfile;
use crate::client::import::PreparedUpload;
use crate::errors::{Result, UdsError};
use crate::models::{ChangelogPatchRequest, CopyReleaseRequest, MutationResponse, ReleaseListResponse};
use crate::stats::ChannelStats;

#[derive(Debug, Clone)]
pub struct AdminClient {
    http: reqwest::Client,
    base_url: String,
    admin_token: String,
}

impl AdminClient {
    pub fn new(profile: &ClientProfile) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent("uds-client")
            .build()
            .map_err(|error| UdsError::Config(format!("failed to create HTTP client: {error}")))?;
        Ok(Self {
            http,
            base_url: profile.base_url.trim_end_matches('/').to_string(),
            admin_token: profile.admin_token.clone(),
        })
    }

    pub async fn list_releases(&self, channel: &str) -> Result<ReleaseListResponse> {
        self.get_json(&format!("/admin/v1/channels/{channel}/releases")).await
    }

    pub async fn upload_release(&self, channel: &str, upload: &PreparedUpload) -> Result<MutationResponse> {
        let metadata = serde_json::to_string(&upload.metadata)?;
        let mut form = Form::new().text("metadata", metadata);
        for artifact in &upload.artifacts {
            let bytes = tokio::fs::read(&artifact.path).await?;
            let part = Part::bytes(bytes).file_name(artifact.file_name.clone());
            form = form.part(artifact.field_name.clone(), part);
        }

        let response = self
            .http
            .post(self.url(&format!("/admin/v1/channels/{channel}/releases")))
            .bearer_auth(&self.admin_token)
            .multipart(form)
            .send()
            .await
            .map_err(|error| UdsError::Storage(format!("upload failed: {error}")))?;
        parse_json_response(response).await
    }

    pub async fn patch_changelog(&self, channel: &str, version: &str, notes: String) -> Result<MutationResponse> {
        self.patch_json(
            &format!("/admin/v1/channels/{channel}/releases/{version}/changelog"),
            &ChangelogPatchRequest { notes },
        )
        .await
    }

    pub async fn withdraw_release(&self, channel: &str, version: &str) -> Result<MutationResponse> {
        let response = self
            .http
            .delete(self.url(&format!("/admin/v1/channels/{channel}/releases/{version}")))
            .bearer_auth(&self.admin_token)
            .send()
            .await
            .map_err(|error| UdsError::Storage(format!("withdraw request failed: {error}")))?;
        parse_json_response(response).await
    }

    pub async fn copy_release(&self, source_channel: &str, target_channel: &str, version: &str) -> Result<MutationResponse> {
        self.post_json(
            &format!("/admin/v1/channels/{target_channel}/copy"),
            &CopyReleaseRequest {
                source_channel: source_channel.to_string(),
                version: version.to_string(),
            },
        )
        .await
    }

    pub async fn channel_stats(&self, channel: &str) -> Result<ChannelStats> {
        self.get_json(&format!("/admin/v1/channels/{channel}/stats")).await
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self
            .http
            .get(self.url(path))
            .bearer_auth(&self.admin_token)
            .send()
            .await
            .map_err(|error| UdsError::Storage(format!("request failed: {error}")))?;
        parse_json_response(response).await
    }

    async fn post_json<T: serde::Serialize, R: serde::de::DeserializeOwned>(&self, path: &str, body: &T) -> Result<R> {
        let response = self
            .http
            .post(self.url(path))
            .bearer_auth(&self.admin_token)
            .json(body)
            .send()
            .await
            .map_err(|error| UdsError::Storage(format!("request failed: {error}")))?;
        parse_json_response(response).await
    }

    async fn patch_json<T: serde::Serialize, R: serde::de::DeserializeOwned>(&self, path: &str, body: &T) -> Result<R> {
        let response = self
            .http
            .patch(self.url(path))
            .bearer_auth(&self.admin_token)
            .json(body)
            .send()
            .await
            .map_err(|error| UdsError::Storage(format!("request failed: {error}")))?;
        parse_json_response(response).await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

pub fn display_path(path: &Path) -> String {
    path.display().to_string()
}

async fn parse_json_response<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| UdsError::Storage(format!("failed to read response body: {error}")))?;
    if !status.is_success() {
        return Err(UdsError::Storage(format!("UDS returned HTTP {status}: {text}")));
    }
    serde_json::from_str(&text).map_err(UdsError::Json)
}
