use std::collections::BTreeMap;
use std::fmt::{self, Write as FmtWrite};
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use flexi_logger::writers::{ArcFileLogWriter, FileLogWriter, FileLogWriterHandle};
use flexi_logger::{Cleanup, Criterion, FileSpec, Naming};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::FormatItem;
use time::macros::format_description;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::{FmtContext, FormattedFields};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry};

use crate::config::{LogLevel, LoggingColorMode, ServerConfig};
use crate::errors::{Result, UdsError};

const TIMESTAMP_FORMAT: &[FormatItem<'_>] =
    format_description!("[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:3]");
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

pub struct LoggingRuntime {
    active_file_path: Option<PathBuf>,
    _file_log_handle: Option<FileLogWriterHandle>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogEventLine {
    pub timestamp: String,
    pub level: LogLevel,
    pub target: String,
    pub fields: BTreeMap<String, String>,
    pub message: String,
}

pub fn init_server_logging(config: &ServerConfig) -> Result<LoggingRuntime> {
    let filter = build_env_filter(&config.logging.level, &config.logging.filter)?;
    let file_path = effective_log_file_path(config);

    let color = match config.logging.console.color {
        LoggingColorMode::Always => true,
        LoggingColorMode::Never => false,
        LoggingColorMode::Auto => io::stdout().is_terminal(),
    };

    let console_layer = config.logging.console.enabled.then(|| {
        tracing_subscriber::fmt::layer()
            .event_format(UdsEventFormatter { color })
            .fmt_fields(UdsFieldFormatter)
            .with_writer(io::stdout)
    });

    let (file_layer, file_log_handle, active_file_path) = if config.logging.file.enabled {
        let base_path = file_path.clone().ok_or_else(|| {
            UdsError::Config("logging file path could not be resolved".to_string())
        })?;
        let (writer, handle, active_path) = build_file_log_writer(
            &base_path,
            config.logging.file.max_size_mb,
            config.logging.file.max_archived_files,
        )?;
        let file_writer = writer.clone();
        (
            Some(
                tracing_subscriber::fmt::layer()
                    .event_format(UdsEventFormatter { color: false })
                    .fmt_fields(UdsFieldFormatter)
                    .with_writer(move || file_writer.clone()),
            ),
            Some(handle),
            Some(active_path),
        )
    } else {
        (None, None, None)
    };

    Registry::default()
        .with(filter)
        .with(console_layer)
        .with(file_layer)
        .try_init()
        .map_err(|error| UdsError::Config(format!("failed to initialize logging: {error}")))?;

    Ok(LoggingRuntime {
        active_file_path,
        _file_log_handle: file_log_handle,
    })
}

pub fn init_client_logging() -> Result<()> {
    let filter = if let Ok(filter) = std::env::var("RUST_LOG") {
        EnvFilter::try_new(filter)
            .map_err(|error| UdsError::Config(format!("invalid RUST_LOG filter: {error}")))?
    } else {
        EnvFilter::try_new("warn")
            .map_err(|error| UdsError::Config(format!("invalid client log filter: {error}")))?
    };

    let color = io::stdout().is_terminal();
    Registry::default()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .event_format(UdsEventFormatter { color })
                .fmt_fields(UdsFieldFormatter)
                .with_writer(io::stdout),
        )
        .try_init()
        .map_err(|error| {
            UdsError::Config(format!("failed to initialize client logging: {error}"))
        })?;
    Ok(())
}

pub fn effective_log_file_path(config: &ServerConfig) -> Option<PathBuf> {
    if !config.logging.file.enabled {
        return None;
    }
    Some(
        config
            .logging
            .file
            .path
            .clone()
            .unwrap_or_else(|| config.data_dir.join("logs/events.log")),
    )
}

fn build_file_log_writer(
    base_path: &Path,
    max_size_mb: u64,
    max_archived_files: usize,
) -> Result<(ArcFileLogWriter, FileLogWriterHandle, PathBuf)> {
    let file_spec = FileSpec::try_from(base_path.to_path_buf())
        .map_err(|error| UdsError::Config(format!("invalid logging file path: {error}")))?;
    let active_path = file_spec.as_pathbuf(Some("rCURRENT"));
    let (writer, handle) = FileLogWriter::builder(file_spec)
        .append()
        .use_utc()
        .rotate(
            Criterion::Size(max_size_mb * 1024 * 1024),
            Naming::Numbers,
            Cleanup::KeepLogFiles(max_archived_files),
        )
        .try_build_with_handle()
        .map_err(|error| UdsError::Config(format!("failed to initialize file logging: {error}")))?;

    Ok((writer, handle, active_path))
}

pub fn build_env_filter(level: &str, configured_filter: &str) -> Result<EnvFilter> {
    if let Ok(filter) = std::env::var("RUST_LOG") {
        return EnvFilter::try_new(filter)
            .map_err(|error| UdsError::Config(format!("invalid RUST_LOG filter: {error}")));
    }

    let mut filter = String::new();
    filter.push_str(level.trim());
    for target in NOISY_TARGETS {
        filter.push(',');
        filter.push_str(target);
        filter.push_str("=info");
    }
    if !configured_filter.trim().is_empty() {
        filter.push(',');
        filter.push_str(configured_filter.trim());
    }

    EnvFilter::try_new(filter)
        .map_err(|error| UdsError::Config(format!("invalid logging filter: {error}")))
}

impl LoggingRuntime {
    pub fn active_file_path(&self) -> Option<&Path> {
        self.active_file_path.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn disabled() -> Self {
        Self {
            active_file_path: None,
            _file_log_handle: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct UdsEventFormatter {
    color: bool,
}

impl<S, N> FormatEvent<S, N> for UdsEventFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let metadata = event.metadata();
        let mut visitor = EventFieldVisitor::default();
        event.record(&mut visitor);

        let timestamp = OffsetDateTime::now_utc()
            .format(TIMESTAMP_FORMAT)
            .map_err(|_| fmt::Error)?;
        let level = metadata.level();
        let level_text = level.as_str();

        if self.color {
            write!(
                writer,
                "[{}] {} [{}] ",
                colorize(&timestamp, level),
                colorize(level_text, level),
                metadata.target()
            )?;
        } else {
            write!(
                writer,
                "[{}] {} [{}] ",
                timestamp,
                level_text,
                metadata.target()
            )?;
        }

        if !visitor.fields.is_empty() {
            write!(writer, "[")?;
            for (index, (key, value)) in visitor.fields.iter().enumerate() {
                if index > 0 {
                    write!(writer, ", ")?;
                }
                write!(writer, "{key} = {value}")?;
            }
            write!(writer, "] ")?;
        }

        if self.color {
            write!(writer, "{}", colorize(&visitor.message, level))?;
        } else {
            write!(writer, "{}", visitor.message)?;
        }
        writeln!(writer)
    }
}

struct UdsFieldFormatter;

impl<'writer> FormatFields<'writer> for UdsFieldFormatter {
    fn format_fields<R: tracing_subscriber::field::RecordFields>(
        &self,
        mut writer: Writer<'writer>,
        fields: R,
    ) -> fmt::Result {
        let mut visitor = EventFieldVisitor::default();
        fields.record(&mut visitor);
        if !visitor.message.is_empty() {
            write!(writer, "{}", visitor.message)?;
        }
        Ok(())
    }

    fn add_fields(
        &self,
        current: &'writer mut FormattedFields<Self>,
        fields: &tracing::span::Record<'_>,
    ) -> fmt::Result {
        let mut writer = current.as_writer();
        self.format_fields(writer.by_ref(), fields)
    }
}

#[derive(Default)]
struct EventFieldVisitor {
    message: String,
    fields: BTreeMap<String, String>,
}

impl Visit for EventFieldVisitor {
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.record_value(field.name(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.record_value(field.name(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.record_value(field.name(), value.to_string());
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.record_value(field.name(), value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.record_value(field.name(), format!("{value:?}"));
    }
}

impl EventFieldVisitor {
    fn record_value(&mut self, name: &str, value: String) {
        if name == "message" {
            self.message = value;
        } else {
            self.fields.insert(name.to_string(), value);
        }
    }
}

pub async fn read_recent_events(path: &Path, lines: usize) -> Result<Vec<LogEventLine>> {
    let content = fs::read_to_string(path).await.unwrap_or_default();
    let mut events = content
        .lines()
        .rev()
        .filter_map(parse_log_line)
        .take(lines)
        .collect::<Vec<_>>();
    events.reverse();
    Ok(events)
}

pub async fn stream_events_from_file(
    path: PathBuf,
    lines: usize,
) -> impl futures_util::Stream<Item = std::result::Result<bytes::Bytes, io::Error>> {
    async_stream::stream! {
        for event in read_recent_events(&path, lines).await.unwrap_or_default() {
            yield Ok(bytes::Bytes::from(ndjson_line(&event)));
        }

        let mut offset = fs::metadata(&path).await.map(|metadata| metadata.len()).unwrap_or(0);
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let Ok(metadata) = fs::metadata(&path).await else {
                continue;
            };
            if metadata.len() < offset {
                offset = 0;
            }
            if metadata.len() == offset {
                continue;
            }

            let mut file = match fs::File::open(&path).await {
                Ok(file) => file,
                Err(error) => {
                    yield Err(io::Error::other(format!("failed to open log file: {error}")));
                    continue;
                }
            };
            if let Err(error) = file.seek(SeekFrom::Start(offset)).await {
                yield Err(error);
                continue;
            }
            let mut appended = String::new();
            if let Err(error) = file.read_to_string(&mut appended).await {
                yield Err(error);
                continue;
            }
            offset = metadata.len();
            for event in appended.lines().filter_map(parse_log_line) {
                yield Ok(bytes::Bytes::from(ndjson_line(&event)));
            }
        }
    }
}

pub fn events_to_ndjson(events: &[LogEventLine]) -> Result<String> {
    let mut body = String::new();
    for event in events {
        body.push_str(&ndjson_line(event));
    }
    Ok(body)
}

pub fn ndjson_line(event: &LogEventLine) -> String {
    let mut line = serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string());
    line.push('\n');
    line
}

pub fn parse_log_line(line: &str) -> Option<LogEventLine> {
    let rest = line.strip_prefix('[')?;
    let (timestamp, rest) = rest.split_once("] ")?;
    let (level, rest) = rest.split_once(' ')?;
    let level = parse_level(level)?;
    let rest = rest.strip_prefix('[')?;
    let (target, mut rest) = rest.split_once("] ")?;
    let mut fields = BTreeMap::new();
    if let Some(field_rest) = rest.strip_prefix('[')
        && let Some((field_text, message_rest)) = field_rest.split_once("] ")
    {
        fields = parse_fields(field_text);
        rest = message_rest;
    }
    Some(LogEventLine {
        timestamp: timestamp.to_string(),
        level,
        target: target.to_string(),
        fields,
        message: rest.to_string(),
    })
}

pub fn should_display_level(event_level: LogLevel, minimum: Option<LogLevel>) -> bool {
    minimum.is_none_or(|minimum| event_level >= minimum)
}

pub fn render_log_event(event: &LogEventLine, color: bool) -> String {
    let level = format!("{:?}", event.level).to_uppercase();
    let mut fields = String::new();
    if !event.fields.is_empty() {
        fields.push_str(" [");
        for (index, (key, value)) in event.fields.iter().enumerate() {
            if index > 0 {
                fields.push_str(", ");
            }
            let _ = write!(fields, "{key} = {value}");
        }
        fields.push(']');
    }
    let line = format!(
        "[{}] {} [{}]{} {}",
        event.timestamp, level, event.target, fields, event.message
    );
    if color {
        colorize_for_log_level(&line, event.level)
    } else {
        line
    }
}

pub fn color_enabled(no_color: bool) -> bool {
    !no_color && io::stdout().is_terminal()
}

fn parse_level(level: &str) -> Option<LogLevel> {
    match level {
        "TRACE" => Some(LogLevel::Trace),
        "DEBUG" => Some(LogLevel::Debug),
        "INFO" => Some(LogLevel::Info),
        "WARN" => Some(LogLevel::Warn),
        "ERROR" => Some(LogLevel::Error),
        _ => None,
    }
}

fn parse_fields(fields: &str) -> BTreeMap<String, String> {
    let mut result = BTreeMap::new();
    for pair in fields.split(", ") {
        if let Some((key, value)) = pair.split_once(" = ") {
            result.insert(key.to_string(), value.to_string());
        }
    }
    result
}

fn colorize(value: &str, level: &Level) -> String {
    let code = match *level {
        Level::ERROR => 196,
        Level::WARN => 208,
        Level::INFO => 34,
        Level::DEBUG => 7,
        Level::TRACE => 8,
    };
    format!("\x1b[38;5;{code}m{value}\x1b[0m")
}

fn colorize_for_log_level(value: &str, level: LogLevel) -> String {
    let code = match level {
        LogLevel::Error => 196,
        LogLevel::Warn => 208,
        LogLevel::Info => 34,
        LogLevel::Debug => 7,
        LogLevel::Trace => 8,
    };
    format!("\x1b[38;5;{code}m{value}\x1b[0m")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_includes_noisy_dependency_caps() {
        let filter = build_env_filter("debug", "").unwrap().to_string();
        assert!(filter.contains("h2=info"));
        assert!(filter.contains("tower_http=info"));
    }

    #[test]
    fn parses_formatted_log_line() {
        let line = "[2026-07-08 12:00:00.001] INFO [uds::test] [channel = stable, version = 26.7.2] uploaded";
        let event = parse_log_line(line).unwrap();
        assert_eq!(event.level, LogLevel::Info);
        assert_eq!(event.target, "uds::test");
        assert_eq!(event.fields.get("channel").unwrap(), "stable");
        assert_eq!(event.message, "uploaded");
    }

    #[test]
    fn flexi_logger_current_path_uses_stable_rotation_infix() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.log");
        let (writer, _handle, active_path) = build_file_log_writer(&path, 1, 2).unwrap();
        let mut writer = writer.clone();
        std::io::Write::write_all(&mut writer, b"hello\n").unwrap();
        std::io::Write::flush(&mut writer).unwrap();
        assert_eq!(active_path, temp.path().join("events_rCURRENT.log"));
        assert!(active_path.exists());
    }
}
