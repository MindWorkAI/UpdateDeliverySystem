use std::collections::{BTreeMap, HashSet};
use std::fmt::{self, Write as _};
use std::io::{self, IsTerminal};
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use flexi_logger::writers::{ArcFileLogWriter, FileLogWriter, FileLogWriterHandle};
use flexi_logger::{Cleanup, Criterion, FileSpec, Naming};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use time::macros::format_description;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader, SeekFrom};
use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::{FmtContext, FormattedFields};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry};
use uuid::Uuid;

use crate::config::{ClientIpLoggingMode, LogLevel, LoggingColorMode, ServerConfig};
use crate::errors::{Result, UdsError};

const NOISY_TARGETS: &[&str] = &[
    "h2",
    "hyper",
    "hyper_util",
    "axum",
    "axum_server",
    "tower",
    "tower_http",
    "rustls",
    "tokio_rustls",
    "reqwest",
];
const MAX_RECENT_EVENTS: usize = 10_000;
const EVENT_TIMESTAMP: &[time::format_description::FormatItem<'_>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogEventKind {
    System,
    Http,
    Audit,
    Security,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ClientIpSource {
    Socket,
    Disabled,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogEventLine {
    pub schema_version: u8,
    pub event_id: Uuid,
    pub timestamp: String,
    pub level: LogLevel,
    pub kind: LogEventKind,
    pub target: String,
    pub request_id: Option<String>,
    pub client_ip: Option<IpAddr>,
    pub client_ip_source: ClientIpSource,
    pub fields: BTreeMap<String, Value>,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct RequestMetadata {
    pub request_id: String,
    pub socket_ip: Option<IpAddr>,
    pub method: String,
    pub route: Option<String>,
}

pub struct LoggingRuntime {
    active_file_path: Option<PathBuf>,
    sender: broadcast::Sender<LogEventLine>,
    client_ip_mode: ClientIpLoggingMode,
    _file_log_handle: Option<FileLogWriterHandle>,
}

pub fn init_server_logging(config: &ServerConfig) -> Result<LoggingRuntime> {
    let filter = build_env_filter(&config.logging.level, &config.logging.filter)?;
    let color = match config.logging.console.color {
        LoggingColorMode::Always => true,
        LoggingColorMode::Never => false,
        LoggingColorMode::Auto => io::stdout().is_terminal(),
    };
    let (sender, _) = broadcast::channel(1024);
    let console_layer = config.logging.console.enabled.then(|| {
        tracing_subscriber::fmt::layer()
            .event_format(HumanFormatter { color })
            .fmt_fields(UdsFieldFormatter)
            .with_writer(io::stdout)
    });
    let (file_layer, handle, active_file_path) = if config.logging.file.enabled {
        let path = effective_log_file_path(config)
            .ok_or_else(|| UdsError::Config("logging file path could not be resolved".into()))?;
        let (writer, handle, active) = build_file_log_writer(
            &path,
            config.logging.file.max_size_mb,
            config.logging.file.max_archived_files,
        )?;
        let file_writer = writer.clone();
        (
            Some(
                tracing_subscriber::fmt::layer()
                    .event_format(NdjsonFormatter {
                        sender: sender.clone(),
                    })
                    .fmt_fields(UdsFieldFormatter)
                    .with_writer(move || file_writer.clone()),
            ),
            Some(handle),
            Some(active),
        )
    } else {
        (None, None, None)
    };
    Registry::default()
        .with(filter)
        .with(console_layer)
        .with(file_layer)
        .try_init()
        .map_err(|e| UdsError::Config(format!("failed to initialize logging: {e}")))?;
    Ok(LoggingRuntime {
        active_file_path,
        sender,
        client_ip_mode: config.logging.client_ip,
        _file_log_handle: handle,
    })
}

pub fn init_client_logging() -> Result<()> {
    let filter = EnvFilter::try_new(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .map_err(|e| UdsError::Config(format!("invalid client log filter: {e}")))?;
    Registry::default()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .event_format(HumanFormatter {
                    color: io::stdout().is_terminal(),
                })
                .fmt_fields(UdsFieldFormatter)
                .with_writer(io::stdout),
        )
        .try_init()
        .map_err(|e| UdsError::Config(format!("failed to initialize client logging: {e}")))?;
    Ok(())
}

impl LoggingRuntime {
    pub fn active_file_path(&self) -> Option<&Path> {
        self.active_file_path.as_deref()
    }
    pub fn subscribe(&self) -> broadcast::Receiver<LogEventLine> {
        self.sender.subscribe()
    }
    pub fn event(
        &self,
        level: LogLevel,
        kind: LogEventKind,
        target: &str,
        request: Option<&RequestMetadata>,
        fields: BTreeMap<String, Value>,
        message: &str,
    ) -> LogEventLine {
        build_event(
            self.client_ip_mode,
            level,
            kind,
            target,
            request,
            fields,
            message,
        )
    }
    pub fn emit(&self, event: &LogEventLine) {
        if let Ok(json) = serde_json::to_string(event) {
            match event.level {
                LogLevel::Trace => tracing::trace!(uds_event = %json, "{}", event.message),
                LogLevel::Debug => tracing::debug!(uds_event = %json, "{}", event.message),
                LogLevel::Info => tracing::info!(uds_event = %json, "{}", event.message),
                LogLevel::Warn => tracing::warn!(uds_event = %json, "{}", event.message),
                LogLevel::Error => tracing::error!(uds_event = %json, "{}", event.message),
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn disabled() -> Self {
        let (sender, _) = broadcast::channel(16);
        Self {
            active_file_path: None,
            sender,
            client_ip_mode: ClientIpLoggingMode::AuditSecurity,
            _file_log_handle: None,
        }
    }
}

pub fn build_event(
    mode: ClientIpLoggingMode,
    level: LogLevel,
    kind: LogEventKind,
    target: &str,
    request: Option<&RequestMetadata>,
    fields: BTreeMap<String, Value>,
    message: &str,
) -> LogEventLine {
    let allowed = request.is_some()
        && match mode {
            ClientIpLoggingMode::Never => false,
            ClientIpLoggingMode::AuditSecurity => {
                matches!(kind, LogEventKind::Audit | LogEventKind::Security)
            }
            ClientIpLoggingMode::Always => true,
        };
    let (client_ip, client_ip_source) = if !allowed {
        (None, ClientIpSource::Disabled)
    } else if let Some(ip) = request.and_then(|r| r.socket_ip) {
        (Some(ip), ClientIpSource::Socket)
    } else {
        (None, ClientIpSource::Unavailable)
    };
    LogEventLine {
        schema_version: 1,
        event_id: Uuid::new_v4(),
        timestamp: OffsetDateTime::now_utc()
            .format(EVENT_TIMESTAMP)
            .unwrap_or_default(),
        level,
        kind,
        target: target.into(),
        request_id: request.map(|r| r.request_id.clone()),
        client_ip,
        client_ip_source,
        fields,
        message: sanitize(message),
    }
}

pub fn effective_log_file_path(config: &ServerConfig) -> Option<PathBuf> {
    config.logging.file.enabled.then(|| {
        config
            .logging
            .file
            .path
            .clone()
            .unwrap_or_else(|| config.data_dir.join("logs/events.ndjson"))
    })
}

fn build_file_log_writer(
    base_path: &Path,
    max_size_mb: u64,
    max_archived_files: usize,
) -> Result<(ArcFileLogWriter, FileLogWriterHandle, PathBuf)> {
    let spec = FileSpec::try_from(base_path.to_path_buf())
        .map_err(|e| UdsError::Config(format!("invalid logging file path: {e}")))?;
    let active = spec.as_pathbuf(Some("rCURRENT"));
    let (writer, handle) = FileLogWriter::builder(spec)
        .append()
        .use_utc()
        .rotate(
            Criterion::Size(max_size_mb * 1024 * 1024),
            Naming::Numbers,
            Cleanup::KeepLogFiles(max_archived_files),
        )
        .try_build_with_handle()
        .map_err(|e| UdsError::Config(format!("failed to initialize file logging: {e}")))?;
    Ok((writer, handle, active))
}

pub fn build_env_filter(level: &str, configured: &str) -> Result<EnvFilter> {
    if let Ok(filter) = std::env::var("RUST_LOG") {
        return EnvFilter::try_new(filter)
            .map_err(|e| UdsError::Config(format!("invalid RUST_LOG filter: {e}")));
    }
    let mut value = level.trim().to_string();
    for target in NOISY_TARGETS {
        let _ = write!(value, ",{target}=info");
    }
    if !configured.trim().is_empty() {
        value.push(',');
        value.push_str(configured.trim());
    }
    EnvFilter::try_new(value).map_err(|e| UdsError::Config(format!("invalid logging filter: {e}")))
}

#[derive(Default)]
struct EventVisitor {
    message: String,
    event_json: Option<String>,
    fields: BTreeMap<String, Value>,
}
impl Visit for EventVisitor {
    fn record_i64(&mut self, f: &Field, v: i64) {
        self.value(f, Value::from(v));
    }
    fn record_u64(&mut self, f: &Field, v: u64) {
        self.value(f, Value::from(v));
    }
    fn record_bool(&mut self, f: &Field, v: bool) {
        self.value(f, Value::from(v));
    }
    fn record_str(&mut self, f: &Field, v: &str) {
        self.value(f, Value::from(v));
    }
    fn record_debug(&mut self, f: &Field, v: &dyn fmt::Debug) {
        self.value(f, Value::from(format!("{v:?}")));
    }
}
impl EventVisitor {
    fn value(&mut self, f: &Field, v: Value) {
        match f.name() {
            "message" => self.message = v.as_str().unwrap_or_default().into(),
            "uds_event" => self.event_json = v.as_str().map(str::to_string),
            _ => {
                self.fields.insert(f.name().into(), v);
            }
        }
    }
}

struct UdsFieldFormatter;
impl<'w> FormatFields<'w> for UdsFieldFormatter {
    fn format_fields<R: tracing_subscriber::field::RecordFields>(
        &self,
        mut w: Writer<'w>,
        fields: R,
    ) -> fmt::Result {
        let mut v = EventVisitor::default();
        fields.record(&mut v);
        write!(w, "{}", sanitize(&v.message))
    }
    fn add_fields(
        &self,
        current: &'w mut FormattedFields<Self>,
        fields: &tracing::span::Record<'_>,
    ) -> fmt::Result {
        self.format_fields(current.as_writer(), fields)
    }
}

#[derive(Clone)]
struct NdjsonFormatter {
    sender: broadcast::Sender<LogEventLine>,
}
impl<S, N> FormatEvent<S, N> for NdjsonFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'w> FormatFields<'w> + 'static,
{
    fn format_event(
        &self,
        _: &FmtContext<'_, S, N>,
        mut w: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let mut v = EventVisitor::default();
        event.record(&mut v);
        let parsed = v
            .event_json
            .as_deref()
            .and_then(|s| serde_json::from_str::<LogEventLine>(s).ok());
        let line = parsed.unwrap_or_else(|| {
            build_event(
                ClientIpLoggingMode::Never,
                level(event.metadata().level()),
                LogEventKind::System,
                event.metadata().target(),
                None,
                v.fields,
                &v.message,
            )
        });
        let _ = self.sender.send(line.clone());
        writeln!(
            w,
            "{}",
            serde_json::to_string(&line).map_err(|_| fmt::Error)?
        )
    }
}

#[derive(Clone, Copy)]
struct HumanFormatter {
    color: bool,
}
impl<S, N> FormatEvent<S, N> for HumanFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'w> FormatFields<'w> + 'static,
{
    fn format_event(
        &self,
        _: &FmtContext<'_, S, N>,
        mut w: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let mut v = EventVisitor::default();
        event.record(&mut v);
        let parsed = v
            .event_json
            .as_deref()
            .and_then(|s| serde_json::from_str::<LogEventLine>(s).ok());
        let line = parsed
            .map(|e| render_log_event(&e, self.color))
            .unwrap_or_else(|| {
                let e = build_event(
                    ClientIpLoggingMode::Never,
                    level(event.metadata().level()),
                    LogEventKind::System,
                    event.metadata().target(),
                    None,
                    v.fields,
                    &v.message,
                );
                render_log_event(&e, self.color)
            });
        writeln!(w, "{line}")
    }
}

fn level(level: &Level) -> LogLevel {
    match *level {
        Level::TRACE => LogLevel::Trace,
        Level::DEBUG => LogLevel::Debug,
        Level::INFO => LogLevel::Info,
        Level::WARN => LogLevel::Warn,
        Level::ERROR => LogLevel::Error,
    }
}
fn sanitize(value: &str) -> String {
    value.chars().flat_map(|c| c.escape_default()).collect()
}

pub async fn read_recent_events(path: &Path, lines: usize) -> Result<Vec<LogEventLine>> {
    let limit = lines.min(MAX_RECENT_EVENTS);
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut paths = rotated_paths(path).await;
    paths.push(path.to_path_buf());
    let mut result = Vec::new();
    for path in paths.into_iter().rev() {
        if result.len() >= limit {
            break;
        }
        let mut events = read_file_backwards(&path, limit - result.len()).await?;
        events.append(&mut result);
        result = events;
    }
    Ok(result)
}

async fn rotated_paths(active: &Path) -> Vec<PathBuf> {
    let Some(dir) = active.parent() else {
        return vec![];
    };
    let Ok(mut rd) = fs::read_dir(dir).await else {
        return vec![];
    };
    let stem = active
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .replace("_rCURRENT", "");
    let mut paths = vec![];
    while let Ok(Some(e)) = rd.next_entry().await {
        let p = e.path();
        if p != active
            && p.file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.starts_with(&stem))
        {
            paths.push(p);
        }
    }
    paths.sort();
    paths
}

async fn read_file_backwards(path: &Path, limit: usize) -> Result<Vec<LogEventLine>> {
    let Ok(mut file) = fs::File::open(path).await else {
        return Ok(vec![]);
    };
    let len = file.metadata().await?.len();
    let complete_last_line = if len == 0 {
        true
    } else {
        file.seek(SeekFrom::End(-1)).await?;
        let mut byte = [0_u8; 1];
        file.read_exact(&mut byte).await?;
        byte[0] == b'\n'
    };
    let start = len.saturating_sub(8 * 1024 * 1024);
    file.seek(SeekFrom::Start(start)).await?;
    let mut reader = BufReader::new(file);
    if start > 0 {
        let mut discard = String::new();
        reader.read_line(&mut discard).await?;
    }
    let mut lines = reader.lines();
    let mut parsed = Vec::new();
    let mut pending = None;
    while let Some(line) = lines.next_line().await? {
        if let Some(previous) = pending.replace(line) {
            push_parsed_line(&mut parsed, &previous);
        }
    }
    if complete_last_line && let Some(line) = pending {
        push_parsed_line(&mut parsed, &line);
    }
    Ok(parsed.into_iter().rev().take(limit).rev().collect())
}

fn push_parsed_line(parsed: &mut Vec<LogEventLine>, line: &str) {
    match serde_json::from_str(line) {
        Ok(event) => parsed.push(event),
        Err(_) => parsed.push(build_event(
            ClientIpLoggingMode::Never,
            LogLevel::Warn,
            LogEventKind::System,
            "uds::logging",
            None,
            BTreeMap::new(),
            "corrupt log event skipped",
        )),
    }
}

pub fn events_to_ndjson(events: &[LogEventLine]) -> Result<String> {
    Ok(events.iter().map(ndjson_line).collect())
}
pub fn ndjson_line(event: &LogEventLine) -> String {
    format!(
        "{}\n",
        serde_json::to_string(event).unwrap_or_else(|_| "{}".into())
    )
}

pub fn stream_events(
    runtime: std::sync::Arc<LoggingRuntime>,
    lines: usize,
) -> impl futures_util::Stream<Item = std::result::Result<bytes::Bytes, io::Error>> {
    async_stream::stream! { let mut receiver=runtime.subscribe(); let history=if let Some(path)=runtime.active_file_path(){read_recent_events(path,lines).await.unwrap_or_default()}else{vec![]}; let mut seen:HashSet<Uuid>=history.iter().map(|e|e.event_id).collect(); for event in history { yield Ok(bytes::Bytes::from(ndjson_line(&event))); } loop { match tokio::time::timeout(std::time::Duration::from_secs(15),receiver.recv()).await { Ok(Ok(event)) if seen.insert(event.event_id)=>yield Ok(bytes::Bytes::from(ndjson_line(&event))), Ok(Err(broadcast::error::RecvError::Lagged(n)))=>{let mut f=BTreeMap::new();f.insert("skipped_events".into(),Value::from(n));let event=build_event(ClientIpLoggingMode::Never,LogLevel::Warn,LogEventKind::System,"uds::logging",None,f,"log stream receiver lagged");yield Ok(bytes::Bytes::from(ndjson_line(&event)));}, Ok(Err(broadcast::error::RecvError::Closed))=>break, Err(_)=>yield Ok(bytes::Bytes::from_static(b"\n")), _=>{} } } }
}

pub fn should_display_level(event: LogLevel, minimum: Option<LogLevel>) -> bool {
    minimum.is_none_or(|m| event >= m)
}
pub fn render_log_event(event: &LogEventLine, color: bool) -> String {
    let fields = event
        .fields
        .iter()
        .map(|(k, v)| format!("{k} = {}", sanitize(&v.to_string())))
        .collect::<Vec<_>>()
        .join(", ");
    let extra = if fields.is_empty() {
        String::new()
    } else {
        format!(" [{fields}]")
    };
    let line = format!(
        "[{}] {:?} [{}]{} {}",
        event.timestamp,
        event.level,
        event.target,
        extra,
        sanitize(&event.message)
    );
    if color {
        colorize(&line, event.level)
    } else {
        line
    }
}
pub fn color_enabled(no_color: bool) -> bool {
    !no_color && io::stdout().is_terminal()
}
fn colorize(v: &str, l: LogLevel) -> String {
    let c = match l {
        LogLevel::Error => 196,
        LogLevel::Warn => 208,
        LogLevel::Info => 34,
        LogLevel::Debug => 7,
        LogLevel::Trace => 8,
    };
    format!("\x1b[38;5;{c}m{v}\x1b[0m")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ip_matrix() {
        let r = RequestMetadata {
            request_id: "r".into(),
            socket_ip: Some("2001:db8::1".parse().unwrap()),
            method: "GET".into(),
            route: None,
        };
        for kind in [
            LogEventKind::Http,
            LogEventKind::Audit,
            LogEventKind::Security,
        ] {
            let e = build_event(
                ClientIpLoggingMode::AuditSecurity,
                LogLevel::Info,
                kind,
                "t",
                Some(&r),
                BTreeMap::new(),
                "m",
            );
            assert_eq!(e.client_ip.is_some(), kind != LogEventKind::Http);
        }
    }
    #[test]
    fn typed_roundtrip() {
        let mut f = BTreeMap::new();
        f.insert("n".into(), Value::from(4));
        let e = build_event(
            ClientIpLoggingMode::Never,
            LogLevel::Info,
            LogEventKind::System,
            "t",
            None,
            f,
            "line\n\x1b[31m",
        );
        let d: LogEventLine = serde_json::from_str(ndjson_line(&e).trim()).unwrap();
        assert_eq!(d.fields["n"], 4);
        assert!(!d.message.contains('\n'));
    }
}
