//! Interactive, single-node server configuration and optional systemd installation.
//!
//! Pure validation/rendering and filesystem operations live separately from the
//! prompts so they can be exercised without a terminal.

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use inquire::{Confirm, Select, Text};
use url::Url;

use crate::config::{ConfigureServerArgs, ListenerConfig, ServerConfig, ServerMode, TlsMode};
use crate::errors::{Result, UdsError};

/// Defines the DEFAULT CONFIG value used by UDS.
const DEFAULT_CONFIG: &str = "uds.toml";

/// Defines the SYSTEM CONFIG value used by UDS.
const SYSTEM_CONFIG: &str = "/etc/uds/config.toml";

/// Defines the SYSTEM BINARY value used by UDS.
const SYSTEM_BINARY: &str = "/usr/local/bin/uds";

/// Defines the UNIT PATH value used by UDS.
const UNIT_PATH: &str = "/etc/systemd/system/uds.service";

/// Root-owned oneshot unit that applies cryptographically verified staging data.
const UPDATE_UNIT_PATH: &str = "/etc/systemd/system/uds-update.service";

/// Bounded filesystem trigger for explicitly submitted update operations.
const UPDATE_PATH_UNIT: &str = "/etc/systemd/system/uds-update.path";

/// Runs the run workflow for UDS.
pub async fn run(args: ConfigureServerArgs) -> Result<()> {
    //
    // Load an existing single-node configuration or start from secure
    // production defaults.
    //
    let mut path = args.config.unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG));
    let original = load_existing(&path)?;
    let mut config = original
        .as_ref()
        .map(|(_, config)| config.clone())
        .unwrap_or_else(ServerConfig::production_single_node_default);

    if config.mode != ServerMode::SingleNode {
        return Err(config_error(
            "the selected file is a fleet configuration; v1 of this wizard only configures single-node servers",
        ));
    }
    if config.public_api.tls.mode == TlsMode::Acme || config.admin_api.tls.mode == TlsMode::Acme {
        return Err(config_error(
            "the selected file uses ACME, which is not supported by this wizard or the current runtime",
        ));
    }
    config.mode = ServerMode::SingleNode;
    let mut new_owner_token = None;
    if config.owner_token_verifier.is_empty() {
        let token = crate::auth::generate_owner_token()?;
        config.owner_token_verifier = crate::auth::verifier(&token);
        new_owner_token = Some(token);
    }

    //
    // Collect the settings required by every single-node installation before
    // offering optional advanced tuning.
    //
    println!("UDS single-node configuration\n");
    config.public_api.bind = prompt_parse("Public API bind address:", config.public_api.bind)?;
    config.admin_api.bind = prompt_parse("Admin API bind address:", config.admin_api.bind)?;
    config.public_base_url = prompt_text("Public base URL:", &config.public_base_url)?;
    config.data_dir = PathBuf::from(prompt_text(
        "Data directory:",
        &config.data_dir.display().to_string(),
    )?);
    let channel_default = config
        .channels
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    config.channels = parse_channels(&prompt_text(
        "Channels (comma-separated):",
        &channel_default,
    )?)?;

    prompt_listener_tls("Public API", &mut config.public_api)?;
    prompt_listener_tls("Admin API", &mut config.admin_api)?;
    let insecure = insecure_listener_names(&config);
    if !insecure.is_empty() {
        eprintln!(
            "WARNING: {} will be reachable beyond loopback without TLS; bearer tokens can be intercepted.",
            insecure.join(" and ")
        );
        if !Confirm::new("Continue with these insecure listener settings?")
            .with_default(false)
            .prompt()
            .map_err(prompt_error)?
        {
            return Err(config_error(
                "configuration cancelled because insecure listeners were not confirmed",
            ));
        }
    }

    if original.is_some()
        && Confirm::new("Replace the existing owner token with a newly generated token?")
            .with_default(false)
            .prompt()
            .map_err(prompt_error)?
    {
        let token = crate::auth::generate_owner_token()?;
        config.owner_token_verifier = crate::auth::verifier(&token);
        new_owner_token = Some(token);
    }

    if Confirm::new("Configure advanced logging, upload, statistics, and shutdown settings?")
        .with_default(false)
        .prompt()
        .map_err(prompt_error)?
    {
        prompt_advanced(&mut config)?;
    }
    config.cluster.node_id_path = config.data_dir.join("node-id");

    //
    // Validate and review the complete configuration before changing any file.
    // Secrets are redacted from the terminal preview.
    //
    validate_preflight(&config, &path)?;
    println!("\nConfiguration review (secrets redacted)\n");
    println!("{}", redacted_toml(&config)?);
    if !Confirm::new(&format!("Write configuration to {}?", path.display()))
        .with_default(false)
        .prompt()
        .map_err(prompt_error)?
    {
        println!("Configuration cancelled; no files were changed.");
        return Ok(());
    }

    let saved = atomic_save(&path, &config, 0o600)?;
    report_saved(&saved);
    if let Some(token) = new_owner_token {
        println!("\nOWNER TOKEN — shown once; store it in a password manager now:\n{token}\n");
    }

    //
    // Offer service installation only on hosts where systemd is available.
    // The wizard never elevates its own privileges.
    //
    if systemd_available()
        && Confirm::new("Install or update the UDS systemd service now?")
            .with_default(false)
            .prompt()
            .map_err(prompt_error)?
    {
        if !is_root() {
            return Err(config_error(
                "systemd installation requires root; rerun this command as root (the wizard never invokes sudo)",
            ));
        }
        if is_in_home(&path)
            && Confirm::new(&format!(
                "Move the service configuration to {SYSTEM_CONFIG}?"
            ))
            .with_default(true)
            .prompt()
            .map_err(prompt_error)?
        {
            path = PathBuf::from(SYSTEM_CONFIG);
            let moved = atomic_save(&path, &config, 0o640)?;
            report_saved(&moved);
        }
        install_systemd(&config, &path)?;
    }

    print_next_steps(&config);
    Ok(())
}

/// Performs the prompt listener tls operation required by UDS.
fn prompt_listener_tls(label: &str, listener: &mut ListenerConfig) -> Result<()> {
    listener.tls.mode = Select::new(
        &format!("{label} TLS mode:"),
        vec![WizardTlsMode::Off, WizardTlsMode::Files],
    )
    .with_starting_cursor(usize::from(listener.tls.mode == TlsMode::Files))
    .prompt()
    .map_err(prompt_error)?
    .into();
    if listener.tls.mode == TlsMode::Files {
        listener.tls.cert_path = Some(PathBuf::from(prompt_text(
            &format!("{label} TLS certificate path:"),
            &display_optional_path(listener.tls.cert_path.as_deref()),
        )?));
        listener.tls.key_path = Some(PathBuf::from(prompt_text(
            &format!("{label} TLS private-key path:"),
            &display_optional_path(listener.tls.key_path.as_deref()),
        )?));
    } else {
        listener.tls.cert_path = None;
        listener.tls.key_path = None;
    }
    Ok(())
}

/// Performs the insecure listener names operation required by UDS.
fn insecure_listener_names(config: &ServerConfig) -> Vec<&'static str> {
    let mut names = Vec::new();
    if config.public_api.tls.mode == TlsMode::Off && !config.public_api.bind.ip().is_loopback() {
        names.push("Public API");
    }
    if config.admin_api.tls.mode == TlsMode::Off && !config.admin_api.bind.ip().is_loopback() {
        names.push("Admin API");
    }
    names
}

/// Performs the prompt advanced operation required by UDS.
fn prompt_advanced(config: &mut ServerConfig) -> Result<()> {
    config.logging.level = prompt_text("Log level/filter:", &config.logging.level)?;
    config.logging.file.enabled = prompt_bool("Enable file logging?", config.logging.file.enabled)?;
    config.logging.file.max_size_mb = prompt_parse(
        "Maximum log file size (MiB):",
        config.logging.file.max_size_mb,
    )?;
    config.logging.file.max_archived_files = prompt_parse(
        "Archived log files to retain:",
        config.logging.file.max_archived_files,
    )?;
    config.upload.max_artifact_size_mb = prompt_parse(
        "Maximum artifact size (MiB):",
        config.upload.max_artifact_size_mb,
    )?;
    config.upload.max_total_artifact_size_mb = prompt_parse(
        "Maximum total upload size (MiB):",
        config.upload.max_total_artifact_size_mb,
    )?;
    config.upload.max_metadata_size_kb = prompt_parse(
        "Maximum metadata size (KiB):",
        config.upload.max_metadata_size_kb,
    )?;
    config.upload.max_platforms = prompt_parse(
        "Maximum platforms per release:",
        config.upload.max_platforms,
    )?;
    config.stats.queue_capacity = prompt_parse("Statistics queue capacity:", config.stats.queue_capacity)?;
    config.stats.max_pending_events = prompt_parse(
        "Maximum pending statistics events:",
        config.stats.max_pending_events,
    )?;
    config.stats.rollup_trigger_events = prompt_parse(
        "Statistics rollup trigger:",
        config.stats.rollup_trigger_events,
    )?;
    config.stats.rollup_interval_seconds = prompt_parse(
        "Statistics rollup interval (seconds):",
        config.stats.rollup_interval_seconds,
    )?;
    config.shutdown.grace_period_seconds = prompt_parse(
        "Shutdown grace period (seconds):",
        config.shutdown.grace_period_seconds,
    )?;
    Ok(())
}

/// Performs the prompt text operation required by UDS.
fn prompt_text(message: &str, default: &str) -> Result<String> {
    Text::new(message)
        .with_default(default)
        .prompt()
        .map_err(prompt_error)
}

/// Performs the prompt parse operation required by UDS.
fn prompt_parse<T>(message: &str, default: T) -> Result<T>
where
    T: std::str::FromStr + fmt::Display,
    T::Err: fmt::Display,
{
    let value = prompt_text(message, &default.to_string())?;
    value
        .parse()
        .map_err(|error| config_error(format!("invalid value for {message} {error}")))
}

/// Performs the prompt bool operation required by UDS.
fn prompt_bool(message: &str, default: bool) -> Result<bool> {
    Confirm::new(message)
        .with_default(default)
        .prompt()
        .map_err(prompt_error)
}

/// Performs the prompt error operation required by UDS.
fn prompt_error(error: inquire::InquireError) -> UdsError {
    config_error(format!("prompt failed: {error}"))
}

/// Performs the load existing operation required by UDS.
fn load_existing(path: &Path) -> Result<Option<(String, ServerConfig)>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(path)?;
    let config = toml::from_str::<ServerConfig>(&text).map_err(|error| {
        config_error(format!(
            "refusing to overwrite invalid config {}: {error}",
            path.display()
        ))
    })?;
    config.validate().map_err(|error| {
        config_error(format!(
            "refusing to overwrite invalid config {}: {error}",
            path.display()
        ))
    })?;
    Ok(Some((text, config)))
}

/// Validates the validate preflight input before UDS trusts or persists it.
pub fn validate_preflight(config: &ServerConfig, destination: &Path) -> Result<()> {
    config.validate()?;
    let url = Url::parse(&config.public_base_url)
        .map_err(|error| config_error(format!("public_base_url is invalid: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(config_error(
            "public_base_url must be an absolute HTTP or HTTPS URL",
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(config_error(
            "public_base_url must not contain a query or fragment",
        ));
    }
    for (label, tls) in [
        ("Public API", &config.public_api.tls),
        ("Admin API", &config.admin_api.tls),
    ] {
        if tls.mode == TlsMode::Files {
            check_readable(
                tls.cert_path.as_deref().unwrap(),
                &format!("{label} TLS certificate"),
            )?;
            check_readable(
                tls.key_path.as_deref().unwrap(),
                &format!("{label} TLS private key"),
            )?;
        }
    }
    check_destination(destination)?;
    check_directory_target(&config.data_dir, "data directory")?;
    if config.logging.file.enabled {
        let log_path = config
            .logging
            .file
            .path
            .clone()
            .unwrap_or_else(|| config.data_dir.join("logs/events.ndjson"));
        if let Some(parent) = log_path.parent() {
            check_directory_target(parent, "log directory")?;
        }
    }
    Ok(())
}

/// Performs the check readable operation required by UDS.
fn check_readable(path: &Path, label: &str) -> Result<()> {
    OpenOptions::new()
        .read(true)
        .open(path)
        .map(|_| ())
        .map_err(|error| {
            config_error(format!(
                "{label} {} is not readable: {error}",
                path.display()
            ))
        })
}

/// Performs the check destination operation required by UDS.
fn check_destination(path: &Path) -> Result<()> {
    if path.exists() {
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map(|_| ())
            .map_err(|error| {
                config_error(format!(
                    "configuration {} is not readable and writable: {error}",
                    path.display()
                ))
            })
    } else {
        check_directory_target(
            path.parent().unwrap_or_else(|| Path::new(".")),
            "configuration directory",
        )
    }
}

/// Performs the check directory target operation required by UDS.
fn check_directory_target(path: &Path, label: &str) -> Result<()> {
    let mut candidate = path;
    while !candidate.exists() {
        candidate = candidate.parent().ok_or_else(|| {
            config_error(format!(
                "{label} {} has no existing ancestor",
                path.display()
            ))
        })?;
    }
    if !candidate.is_dir() {
        return Err(config_error(format!(
            "{label} {} is not a directory",
            candidate.display()
        )));
    }
    let metadata = fs::metadata(candidate)?;
    if metadata.permissions().readonly() {
        return Err(config_error(format!(
            "{label} {} is read-only",
            candidate.display()
        )));
    }

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let candidate_c = std::ffi::CString::new(candidate.as_os_str().as_bytes())
            .map_err(|_| config_error(format!("{label} contains a NUL byte")))?;
        if unsafe { libc::access(candidate_c.as_ptr(), libc::W_OK) } != 0 {
            return Err(config_error(format!(
                "{label} {} is not writable by the current user",
                candidate.display()
            )));
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
/// Result of atomically saving a configuration and its optional backup.
pub struct SaveOutcome {
    /// The path carried by this UDS data contract.
    pub path: PathBuf,

    /// The backup carried by this UDS data contract.
    pub backup: Option<PathBuf>,
}

/// Provides the atomic save operation used by UDS callers.
pub fn atomic_save(path: &Path, config: &ServerConfig, mode: u32) -> Result<SaveOutcome> {
    config.validate()?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let backup = if path.exists() {
        let backup = next_backup_path(path);
        fs::copy(path, &backup)?;
        set_mode(&backup, mode)?;
        Some(backup)
    } else {
        None
    };
    let rendered = toml::to_string_pretty(config)
        .map_err(|error| config_error(format!("could not serialize configuration: {error}")))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(rendered.as_bytes())?;
    temporary.as_file().sync_all()?;
    set_mode(temporary.path(), mode)?;
    temporary
        .persist(path)
        .map_err(|error| UdsError::Io(error.error))?;
    sync_directory(parent)?;
    Ok(SaveOutcome {
        path: path.to_path_buf(),
        backup,
    })
}

/// Performs the next backup path operation required by UDS.
fn next_backup_path(path: &Path) -> PathBuf {
    for index in 0.. {
        let suffix = if index == 0 {
            ".bak".to_string()
        } else {
            format!(".bak.{index}")
        };
        let candidate = PathBuf::from(format!("{}{}", path.display(), suffix));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

/// Performs the set mode operation required by UDS.
#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

/// Verifies that set mode.
#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

/// Performs the sync directory operation required by UDS.
fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

/// Provides the redacted toml operation used by UDS callers.
pub fn redacted_toml(config: &ServerConfig) -> Result<String> {
    let mut copy = config.clone();
    copy.owner_token_verifier = "<redacted>".into();
    if copy.cluster_token.is_some() {
        copy.cluster_token = Some("<redacted>".into());
    }
    toml::to_string_pretty(&copy).map_err(|error| config_error(format!("could not render review: {error}")))
}

/// Produces the render systemd unit representation returned or displayed by UDS.
pub fn render_systemd_unit(config: &ServerConfig, binary: &Path, config_path: &Path) -> String {
    let data = absolute_path(&config.data_dir);
    let log = config
        .logging
        .file
        .path
        .as_deref()
        .and_then(Path::parent)
        .map(absolute_path)
        .unwrap_or_else(|| data.join("logs"));
    let privileged = config.public_api.bind.port() < 1024
        || config.admin_api.bind.port() < 1024
        || config
            .fleet_api
            .as_ref()
            .is_some_and(|v| v.bind.port() < 1024);
    let capability = if privileged {
        "AmbientCapabilities=CAP_NET_BIND_SERVICE\nCapabilityBoundingSet=CAP_NET_BIND_SERVICE\n"
    } else {
        "CapabilityBoundingSet=\n"
    };
    format!(
        r#"
[Unit]
Description=MindWork AI Studio Update Delivery System
After=network-online.target
Wants=network-online.target

[Service]
Type=notify
NotifyAccess=main
User=uds
Group=uds
ExecStart={} server --config {}
Restart=on-failure
RestartSec=5s
TimeoutStopSec={}s
NoNewPrivileges=true
PrivateTmp=true
PrivateDevices=true
ProtectSystem=strict
ProtectHome=read-only
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
RestrictSUIDSGID=true
LockPersonality=true
MemoryDenyWriteExecute=true
RestrictRealtime=true
{}ReadWritePaths={} {}

[Install]
WantedBy=multi-user.target
"#,
        binary.display(),
        config_path.display(),
        config.shutdown.grace_period_seconds.saturating_add(30),
        capability,
        data.display(),
        log.display()
    )
    .trim_start()
    .to_string()
}

/// Renders the root oneshot which alone may replace `/usr/local/bin/uds`.
pub fn render_update_unit(config: &ServerConfig) -> String {
    format!(
        r#"[Unit]
Description=Apply a manually selected UDS update
After=uds.service

[Service]
Type=oneshot
User=root
Group=root
ExecStart=/usr/local/bin/uds server apply-updates --data-dir {} --binary /usr/local/bin/uds
NoNewPrivileges=true
PrivateTmp=true
PrivateDevices=true
ProtectHome=true
ProtectSystem=strict
ReadWritePaths=/usr/local/bin {}
CapabilityBoundingSet=
RestrictSUIDSGID=true
LockPersonality=true
"#,
        absolute_path(&config.data_dir).display(),
        absolute_path(&config.data_dir).display()
    )
}

/// Renders the limited path trigger installed only by the single-node wizard.
pub fn render_update_path_unit(config: &ServerConfig) -> String {
    format!(
        r#"[Unit]
Description=Watch for explicitly staged UDS updates

[Path]
PathChanged={}/self-update/operations
Unit=uds-update.service

[Install]
WantedBy=multi-user.target
"#,
        absolute_path(&config.data_dir).display()
    )
}

/// Performs the install systemd operation required by UDS.
fn install_systemd(config: &ServerConfig, config_path: &Path) -> Result<()> {
    let data_dir = absolute_path(&config.data_dir);
    if [
        Path::new("/"),
        Path::new("/var"),
        Path::new("/home"),
        Path::new("/usr"),
    ]
    .contains(&data_dir.as_path())
    {
        return Err(config_error(
            "data_dir must be a dedicated directory before systemd installation",
        ));
    }
    ensure_service_account()?;
    let binary = choose_binary()?;
    fs::create_dir_all(&config.data_dir)?;
    if let Some(log_dir) = config.logging.file.path.as_deref().and_then(Path::parent) {
        if [Path::new("/"), Path::new("/var"), Path::new("/var/log")].contains(&log_dir) {
            return Err(config_error(
                "the log file must use a dedicated subdirectory, not a system directory directly",
            ));
        }
        fs::create_dir_all(log_dir)?;
        run_checked(
            Command::new("chown").args(["uds:uds", &log_dir.display().to_string()]),
            "set log ownership",
        )?;
    }
    run_checked(
        Command::new("chown").args(["root:uds", &config_path.display().to_string()]),
        "set config ownership",
    )?;
    set_mode(config_path, 0o640)?;
    run_checked(
        Command::new("chown").args(["-R", "uds:uds", &config.data_dir.display().to_string()]),
        "set data ownership",
    )?;
    write_atomic_text(
        Path::new(UNIT_PATH),
        &render_systemd_unit(config, &binary, config_path),
        0o644,
    )?;
    write_atomic_text(
        Path::new(UPDATE_UNIT_PATH),
        &render_update_unit(config),
        0o644,
    )?;
    write_atomic_text(
        Path::new(UPDATE_PATH_UNIT),
        &render_update_path_unit(config),
        0o644,
    )?;
    run_checked(
        Command::new("systemctl").arg("daemon-reload"),
        "reload systemd",
    )?;
    if Confirm::new("Enable and start (or restart) uds.service now?")
        .with_default(true)
        .prompt()
        .map_err(prompt_error)?
    {
        run_checked(
            Command::new("systemctl").args(["enable", "--now", "uds.service"]),
            "enable and start uds.service",
        )?;
        run_checked(
            Command::new("systemctl").args(["enable", "--now", "uds-update.path"]),
            "enable the manual update trigger",
        )?;
        run_checked(
            Command::new("systemctl").args(["restart", "uds.service"]),
            "restart uds.service",
        )?;
        verify_service(config)?;
    }
    Ok(())
}

/// Performs the ensure service account operation required by UDS.
fn ensure_service_account() -> Result<()> {
    if !Command::new("getent")
        .args(["group", "uds"])
        .stdout(Stdio::null())
        .status()?
        .success()
    {
        run_checked(
            Command::new("groupadd").args(["--system", "uds"]),
            "create uds group",
        )?;
    }
    if !Command::new("getent")
        .args(["passwd", "uds"])
        .stdout(Stdio::null())
        .status()?
        .success()
    {
        run_checked(
            Command::new("useradd").args([
                "--system",
                "--gid",
                "uds",
                "--home-dir",
                "/var/lib/uds",
                "--no-create-home",
                "--shell",
                "/usr/sbin/nologin",
                "uds",
            ]),
            "create uds user",
        )?;
    }
    Ok(())
}

/// Performs the choose binary operation required by UDS.
fn choose_binary() -> Result<PathBuf> {
    let current = std::env::current_exe()?;
    let risky =
        current.starts_with("/tmp") || current.starts_with("/home") || current.to_string_lossy().contains("/target/");
    if risky {
        println!(
            "Warning: the current binary is in a build, temporary, or home path: {}",
            current.display()
        );
    }
    let choice = Select::new(
        "Binary for the systemd service:",
        vec![
            BinaryChoice::Copy,
            BinaryChoice::Current,
            BinaryChoice::Cancel,
        ],
    )
    .prompt()
    .map_err(prompt_error)?;
    match choice {
        BinaryChoice::Current => Ok(current),
        BinaryChoice::Cancel => Err(config_error("systemd installation cancelled")),
        BinaryChoice::Copy => {
            let target = PathBuf::from(SYSTEM_BINARY);
            if target.exists()
                && !Confirm::new(&format!("Overwrite {SYSTEM_BINARY}?"))
                    .with_default(false)
                    .prompt()
                    .map_err(prompt_error)?
            {
                return Err(config_error("systemd installation cancelled"));
            }
            copy_atomic(&current, &target, 0o755)?;
            Ok(target)
        }
    }
}

/// Performs the verify service operation required by UDS.
fn verify_service(config: &ServerConfig) -> Result<()> {
    let status = Command::new("systemctl")
        .args(["is-active", "uds.service"])
        .output()?;
    if !status.status.success() {
        show_diagnostics();
        return Err(config_error("uds.service did not become active"));
    }
    let mut health = Url::parse(&config.public_base_url).map_err(|e| config_error(e.to_string()))?;
    health.set_path("/health");
    health.set_query(None);
    health.set_fragment(None);
    let response = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| config_error(e.to_string()))?
        .get(health)
        .send();
    if !matches!(response, Ok(ref value) if value.status().is_success()) {
        show_diagnostics();
        return Err(config_error(
            "uds.service is active, but /health did not return success within five seconds",
        ));
    }
    Ok(())
}

/// Performs the show diagnostics operation required by UDS.
fn show_diagnostics() {
    for args in [
        ["status", "--no-pager", "uds.service"].as_slice(),
        ["--no-pager", "-u", "uds.service", "-n", "40"].as_slice(),
    ] {
        let program = if args[0] == "status" { "systemctl" } else { "journalctl" };
        if let Ok(output) = Command::new(program).args(args).output() {
            eprintln!("{}", String::from_utf8_lossy(&output.stdout));
        }
    }
}

/// Performs the systemd available operation required by UDS.
fn systemd_available() -> bool {
    cfg!(target_os = "linux")
        && Path::new("/run/systemd/system").is_dir()
        && Command::new("systemctl")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
}

/// Performs the is root operation required by UDS.
#[cfg(unix)]
fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// Verifies that is root.
#[cfg(not(unix))]
fn is_root() -> bool {
    false
}

/// Performs the is in home operation required by UDS.
fn is_in_home(path: &Path) -> bool {
    std::env::var_os("HOME").is_some_and(|home| absolute_path(path).starts_with(home))
}

/// Performs the absolute path operation required by UDS.
fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    }
}

/// Performs the run checked operation required by UDS.
fn run_checked(command: &mut Command, action: &str) -> Result<()> {
    let output = command.output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(config_error(format!(
            "failed to {action}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// Performs the write atomic text operation required by UDS.
fn write_atomic_text(path: &Path, value: &str, mode: u32) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(value.as_bytes())?;
    temp.as_file().sync_all()?;
    set_mode(temp.path(), mode)?;
    temp.persist(path).map_err(|e| UdsError::Io(e.error))?;
    sync_directory(parent)
}

/// Performs the copy atomic operation required by UDS.
fn copy_atomic(source: &Path, destination: &Path, mode: u32) -> Result<()> {
    let parent = destination.parent().unwrap();
    fs::create_dir_all(parent)?;
    let mut input = fs::File::open(source)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    std::io::copy(&mut input, &mut temp)?;
    temp.as_file().sync_all()?;
    set_mode(temp.path(), mode)?;
    temp.persist(destination)
        .map_err(|e| UdsError::Io(e.error))?;
    sync_directory(parent)
}

/// Performs the parse channels operation required by UDS.
fn parse_channels(value: &str) -> Result<BTreeSet<String>> {
    let channels = value
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    if channels.is_empty() {
        return Err(config_error("at least one channel is required"));
    }
    if channels.iter().any(|v| {
        !v.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    }) {
        return Err(config_error(
            "channel names may only contain ASCII letters, numbers, '-' and '_'",
        ));
    }
    Ok(channels)
}

/// Performs the display optional path operation required by UDS.
fn display_optional_path(path: Option<&Path>) -> String {
    path.map(|p| p.display().to_string()).unwrap_or_default()
}

/// Performs the config error operation required by UDS.
fn config_error(message: impl Into<String>) -> UdsError {
    UdsError::Config(message.into())
}

/// Performs the report saved operation required by UDS.
fn report_saved(outcome: &SaveOutcome) {
    println!("Saved configuration to {}", outcome.path.display());
    if let Some(path) = &outcome.backup {
        println!("Original preserved in protected backup {}", path.display());
    }
}

/// Performs the print next steps operation required by UDS.
fn print_next_steps(config: &ServerConfig) {
    println!(
        "\nNext steps:\n- Health: {}/health\n- Logs: journalctl -u uds -f\n- Client: uds client configure\n- Terminate TLS at a reverse proxy unless TLS files are configured.\n- Back up the configuration and {} regularly.",
        config.public_base_url.trim_end_matches('/'),
        config.data_dir.display()
    );
}

#[derive(Clone, Copy, Debug)]
/// TLS choices currently supported by the interactive configuration wizard.
enum WizardTlsMode {
    /// Represents the item concept used by UDS.
    Off,

    /// Represents the item concept used by UDS.
    Files,
}
impl fmt::Display for WizardTlsMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Off => "Off (reverse proxy or private HTTP)",
                Self::Files => "Certificate and key files",
            }
        )
    }
}
impl From<WizardTlsMode> for TlsMode {
    fn from(value: WizardTlsMode) -> Self {
        match value {
            WizardTlsMode::Off => Self::Off,
            WizardTlsMode::Files => Self::Files,
        }
    }
}

#[derive(Clone, Copy, Debug)]
/// Source from which the systemd installer obtains the UDS executable.
enum BinaryChoice {
    /// Represents the item concept used by UDS.
    Copy,

    /// Represents the item concept used by UDS.
    Current,

    /// Represents the item concept used by UDS.
    Cancel,
}
impl fmt::Display for BinaryChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Copy => "Copy atomically to /usr/local/bin/uds",
                Self::Current => "Use the current path",
                Self::Cancel => "Cancel systemd installation",
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn valid_config(root: &Path) -> ServerConfig {
        let mut config = ServerConfig::production_single_node_default();
        config.public_base_url = "https://updates.example.test".into();
        config.data_dir = root.join("data");
        config.cluster.node_id_path = config.data_dir.join("node-id");
        config.owner_token_verifier = crate::auth::verifier("uds_owner_v1_test-secret");
        config
    }

    /// Verifies that redacts secrets.
    #[test]
    fn redacts_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = valid_config(dir.path());
        config.cluster_token = Some("another-secret-token".into());
        let review = redacted_toml(&config).unwrap();
        assert!(!review.contains(&config.owner_token_verifier));
        assert!(!review.contains("another-secret-token"));
        assert!(review.contains("<redacted>"));
    }

    /// Verifies that atomic save reloads and protects config and backup.
    #[test]
    fn atomic_save_reloads_and_protects_config_and_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config = valid_config(dir.path());
        atomic_save(&path, &config, 0o600).unwrap();
        fs::write(&path, "original comments\n").unwrap();
        let outcome = atomic_save(&path, &config, 0o600).unwrap();
        let loaded: ServerConfig = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        loaded.validate().unwrap();
        assert_eq!(
            fs::read_to_string(outcome.backup.as_ref().unwrap()).unwrap(),
            "original comments\n"
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(outcome.backup.unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    /// Verifies that unit is hardened and only grants low port capability.
    #[test]
    fn unit_is_hardened_and_only_grants_low_port_capability() {
        let mut config = valid_config(Path::new("/srv/uds-test"));
        let normal = render_systemd_unit(
            &config,
            Path::new("/usr/local/bin/uds"),
            Path::new(SYSTEM_CONFIG),
        );
        let expected_normal = r#"[Unit]
Description=MindWork AI Studio Update Delivery System
After=network-online.target
Wants=network-online.target

[Service]
Type=notify
NotifyAccess=main
User=uds
Group=uds
ExecStart=/usr/local/bin/uds server --config /etc/uds/config.toml
Restart=on-failure
RestartSec=5s
TimeoutStopSec=330s
NoNewPrivileges=true
PrivateTmp=true
PrivateDevices=true
ProtectSystem=strict
ProtectHome=read-only
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
RestrictSUIDSGID=true
LockPersonality=true
MemoryDenyWriteExecute=true
RestrictRealtime=true
CapabilityBoundingSet=
ReadWritePaths=/srv/uds-test/data /srv/uds-test/data/logs

[Install]
WantedBy=multi-user.target
"#;
        assert_eq!(normal, expected_normal);

        config.public_api.bind = "0.0.0.0:443".parse().unwrap();
        let privileged = render_systemd_unit(&config, Path::new(SYSTEM_BINARY), Path::new(SYSTEM_CONFIG));
        let expected_privileged = expected_normal.replace(
            "CapabilityBoundingSet=\n",
            "AmbientCapabilities=CAP_NET_BIND_SERVICE\nCapabilityBoundingSet=CAP_NET_BIND_SERVICE\n",
        );
        assert_eq!(privileged, expected_privileged);

        let update = render_update_unit(&config);
        assert!(update.contains("Type=oneshot"));
        assert!(update.contains("User=root"));
        assert!(update.contains("ReadWritePaths=/usr/local/bin /srv/uds-test/data"));
        let trigger = render_update_path_unit(&config);
        assert!(trigger.contains("PathChanged=/srv/uds-test/data/self-update/operations"));
        assert!(trigger.contains("Unit=uds-update.service"));
    }

    /// Verifies that rejects bad url and channels.
    #[test]
    fn rejects_bad_url_and_channels() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = valid_config(dir.path());
        config.public_base_url = "ftp://example.test".into();
        assert!(validate_preflight(&config, &dir.path().join("config.toml")).is_err());
        assert!(parse_channels(" , ").is_err());
        assert!(parse_channels("stable,not valid").is_err());
    }
}
