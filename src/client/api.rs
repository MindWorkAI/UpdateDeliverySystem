use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::multipart::{Form, Part};

use crate::client::config::ClientProfile;
use crate::client::import::PreparedUpload;
use crate::errors::{Result, UdsError};
use crate::logging::LogEventLine;
use crate::models::{
    ChangelogPatchRequest, CopyReleaseRequest, MutationResponse, ReleaseListResponse, UploadPolicy,
};
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
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(|error| UdsError::Config(format!("failed to create HTTP client: {error}")))?;
        Ok(Self {
            http,
            base_url: profile.base_url.trim_end_matches('/').to_string(),
            admin_token: profile.admin_token.clone(),
        })
    }

    pub async fn list_releases(&self, channel: &str) -> Result<ReleaseListResponse> {
        self.get_json(&format!("/admin/v1/channels/{channel}/releases"))
            .await
    }

    pub async fn upload_policy(&self) -> Result<UploadPolicy> {
        self.get_json("/admin/v1/upload-policy").await
    }

    pub async fn upload_release(
        &self,
        channel: &str,
        upload: &PreparedUpload,
    ) -> Result<MutationResponse> {
        let metadata = serde_json::to_string(&upload.metadata)?;
        let mut form = Form::new().text("metadata", metadata);
        for artifact in &upload.artifacts {
            let file = tokio::fs::File::open(&artifact.path).await?;
            let stream = tokio_util::io::ReaderStream::new(file);
            let body = reqwest::Body::wrap_stream(stream);
            let part =
                Part::stream_with_length(body, artifact.size).file_name(artifact.file_name.clone());
            form = form.part(artifact.field_name.clone(), part);
        }

        let response = self
            .http
            .post(self.url(&format!("/admin/v1/channels/{channel}/releases")))
            .bearer_auth(&self.admin_token)
            .multipart(form)
            .timeout(Duration::from_secs(30 * 60))
            .send()
            .await
            .map_err(|error| UdsError::Storage(format!("upload failed: {error}")))?;
        parse_json_response(response).await
    }

    pub async fn patch_changelog(
        &self,
        channel: &str,
        version: &str,
        notes: String,
    ) -> Result<MutationResponse> {
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
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|error| UdsError::Storage(format!("withdraw request failed: {error}")))?;
        parse_json_response(response).await
    }

    pub async fn copy_release(
        &self,
        source_channel: &str,
        target_channel: &str,
        version: &str,
    ) -> Result<MutationResponse> {
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
        self.get_json(&format!("/admin/v1/channels/{channel}/stats"))
            .await
    }

    pub async fn recent_logs(&self, lines: usize) -> Result<Vec<LogEventLine>> {
        let text = self
            .get_text(&format!("/admin/v1/logs/recent?lines={lines}"))
            .await?;
        parse_ndjson_events(&text)
    }

    pub async fn stream_logs<F>(&self, lines: usize, mut on_event: F) -> Result<()>
    where
        F: FnMut(LogEventLine),
    {
        let mut seen = HashSet::new();
        let mut backoff = Duration::from_millis(250);
        let mut history_lines = lines;
        loop {
            let sent = self
                .http
                .get(self.url(&format!("/admin/v1/logs/stream?lines={history_lines}")))
                .bearer_auth(&self.admin_token)
                .send()
                .await;
            let response = match sent {
                Ok(response) => response,
                Err(_) => {
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(10));
                    continue;
                }
            };
            let status = response.status();
            if status.is_client_error() {
                let text = response.text().await.unwrap_or_default();
                return Err(UdsError::Storage(format!(
                    "UDS returned HTTP {status}: {text}"
                )));
            }
            if !status.is_success() {
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(10));
                continue;
            }
            backoff = Duration::from_millis(250);
            history_lines = 0;
            let mut stream = response.bytes_stream();
            let mut buffer = Vec::new();
            while let Some(chunk) = stream.next().await {
                let Ok(chunk) = chunk else { break };
                buffer.extend_from_slice(&chunk);
                while let Some(index) = buffer.iter().position(|b| *b == b'\n') {
                    let line = buffer.drain(..=index).collect::<Vec<_>>();
                    if line.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    let event: LogEventLine = serde_json::from_slice(&line)?;
                    if seen.insert(event.event_id) {
                        on_event(event);
                    }
                    if seen.len() > 20_000 {
                        seen.clear();
                    }
                }
            }
            tokio::time::sleep(backoff).await;
        }
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self
            .http
            .get(self.url(path))
            .bearer_auth(&self.admin_token)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|error| UdsError::Storage(format!("request failed: {error}")))?;
        parse_json_response(response).await
    }

    async fn get_text(&self, path: &str) -> Result<String> {
        let response = self
            .http
            .get(self.url(path))
            .bearer_auth(&self.admin_token)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|error| UdsError::Storage(format!("request failed: {error}")))?;
        parse_text_response(response).await
    }

    async fn post_json<T: serde::Serialize, R: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<R> {
        let response = self
            .http
            .post(self.url(path))
            .bearer_auth(&self.admin_token)
            .json(body)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|error| UdsError::Storage(format!("request failed: {error}")))?;
        parse_json_response(response).await
    }

    async fn patch_json<T: serde::Serialize, R: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &T,
    ) -> Result<R> {
        let response = self
            .http
            .patch(self.url(path))
            .bearer_auth(&self.admin_token)
            .json(body)
            .timeout(Duration::from_secs(30))
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

async fn parse_json_response<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T> {
    let text = parse_text_response(response).await?;
    serde_json::from_str(&text).map_err(UdsError::Json)
}

async fn parse_text_response(response: reqwest::Response) -> Result<String> {
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| UdsError::Storage(format!("failed to read response body: {error}")))?;
    if !status.is_success() {
        return Err(UdsError::Storage(format!(
            "UDS returned HTTP {status}: {text}"
        )));
    }
    Ok(text)
}

fn parse_ndjson_events(text: &str) -> Result<Vec<LogEventLine>> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<LogEventLine>(line).map_err(UdsError::Json))
        .collect()
}
