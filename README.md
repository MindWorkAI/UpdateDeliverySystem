# Update Delivery System

The Update Delivery System (UDS) distributes signed MindWork AI Studio updates to clients.

UDS can run as a small single-node system for one organization, or as a fleet behind a public load balancer. Each UDS node stores releases directly on the file system and exposes a Tauri-compatible update endpoint, download endpoint, private administration API, and optional internal fleet API.

## Current Status

This repository contains the initial Rust implementation. The single-node update path, release storage, changelog updates, downloads, local statistics, TLS file mode, and fleet-mode switches are implemented. Fleet discovery and replication endpoints are present as the first runtime shape; deep peer-to-peer artifact synchronization will be expanded without changing the public API.

## Quick Start: Single Node

Create a configuration file:

```toml
mode = "single-node"
bind = "0.0.0.0:8080"
public_base_url = "https://updates.example.org"
data_dir = "/var/lib/uds"
admin_token = "replace-with-a-long-random-admin-token"
channels = ["stable", "beta", "experimental", "mature"]

[tls]
mode = "off"
```

Start UDS:

```bash
uds --config /etc/uds/config.toml --single-node-mode
```

Use `tls.mode = "off"` only when TLS is terminated by a reverse proxy or load balancer. If UDS is exposed directly, configure TLS certificate files.

## Configuration

```toml
mode = "fleet"
bind = "0.0.0.0:443"
public_base_url = "https://updates.example.org"
data_dir = "/var/lib/uds"
admin_token = "replace-with-a-long-random-admin-token"
cluster_token = "replace-with-a-long-random-cluster-token"
channels = ["stable", "beta", "experimental", "mature"]

[tls]
mode = "files"
cert_path = "/etc/uds/tls/fullchain.pem"
key_path = "/etc/uds/tls/privkey.pem"

[cluster]
node_id_path = "node-id"
broadcast_addr = "255.255.255.255:44231"
broadcast_interval_seconds = 30
reconcile_interval_seconds = 300

[logging]
level = "info"
filter = ""
client_ip = "audit-security"

[logging.console]
enabled = true
color = "auto"

[logging.file]
enabled = true
path = "/var/log/mindwork-ai/uds/events.ndjson"
max_size_mb = 100
max_archived_files = 5

[logging.admin_api]
enabled = true

[upload]
max_artifact_size_mb = 512
max_total_artifact_size_mb = 2048
max_metadata_size_kb = 1024
max_platforms = 32

[stats]
queue_capacity = 8192
max_pending_events = 100000
rollup_trigger_events = 10000
rollup_interval_seconds = 900
```

### Modes

- `single-node`: disables broadcast discovery, peer reconciliation, replication, and internal peer routes. This is the recommended mode for small organizations.
- `fleet`: enables the internal fleet shape and background broadcast task. This mode requires `cluster_token`.

The CLI flag `--single-node-mode` overrides the configuration file and forces single-node mode.

### Release Channels

- `stable`: the current release recommended for most installations.
- `mature`: an older, field-tested release for environments that prioritize reliability over receiving updates quickly. This channel does not provide long-term support, a guaranteed support period, or an extended maintenance commitment.
- `beta`: a preview of an upcoming stable release.
- `experimental`: early releases intended for testing new or potentially disruptive changes.

### TLS Modes

- `off`: UDS serves plain HTTP. Use this behind a TLS-terminating load balancer or reverse proxy.
- `files`: UDS serves HTTPS with administrator-provided certificate and key files. This enables HTTP/1.1 and HTTP/2.
- `acme`: reserved for automatic ACME certificate management. The configuration is validated, but the runtime currently asks administrators to use `files` mode or load-balancer TLS termination until ACME serving is wired in.

For HTTP/3, terminate TLS and HTTP/3 at the edge and proxy to UDS over HTTP/1.1 or HTTP/2.

## Public API

### Health

```http
GET /health
```

Returns basic service status, system mode, and node ID.

### Update Check

```http
GET /api/v1/updates/{channel}/{target}/{arch}/{current_version}
```

Examples:

```bash
curl https://updates.example.org/api/v1/updates/stable/windows/x86_64/26.5.5
```

When no update is available, UDS returns `204 No Content`.

When an update is available, UDS returns Tauri-compatible JSON:

```json
{
  "version": "26.7.2",
  "url": "https://updates.example.org/api/v1/downloads/stable/26.7.2/windows-x86_64/MindWork-AI-Studio.zip",
  "signature": "base64-or-minisign-signature",
  "pub_date": "2026-07-07T12:00:00Z",
  "notes": "## 26.6.0\n\nChanged ...\n\n## 26.7.2\n\nFixed ..."
}
```

The `notes` field is personalized for the client version. If a user skipped three releases, UDS returns the changelog for all releases newer than the installed version and up to the offered version.

### Download

```http
GET /api/v1/downloads/{channel}/{version}/{target}-{arch}/{file_name}
```

UDS streams the artifact and records local download statistics.

## Administration Client

Administrators should normally use the `uds` client mode instead of calling the Admin API manually:

```bash
uds client
```

You can also start a specific workflow directly:

```bash
uds client configure
uds client upload
uds client changelog
uds client withdraw
uds client copy
uds client stats
uds client logs
```

The first client run offers to create a local profile. The profile stores the UDS base URL, admin token, and default channel in a user-local config file. UDS hardens this file so only the current user can read or write it on Linux and macOS, and uses `icacls` for equivalent best-effort ACL hardening on Windows.

### Configure the Client

```bash
uds client configure
```

The prompt asks for:

- Profile name.
- UDS base URL.
- Admin token.
- Default channel.

Config file locations are selected from the operating system's user config directory, for example `~/.config/mindwork-ai/uds/client.toml` on many Linux systems.

### Upload a Release

```bash
uds client upload
```

The upload wizard asks for the channel and source. Supported sources:

- A GitHub release URL, GitHub tag URL, or direct `latest.json` URL.
- A local Tauri `latest.json` file plus a directory containing the referenced artifacts.

For GitHub imports, UDS downloads the Tauri updater `latest.json`, downloads all referenced artifacts, computes SHA-256 hashes and sizes, and shows a review screen before uploading anything to UDS. The review includes version, channel, notes preview, platform keys, file names, source URLs, sizes, SHA-256 hashes, and signatures. The upload only starts after explicit confirmation.

### Correct a Changelog

Use this when a typo or wording issue must be corrected without rebuilding release artifacts.

```bash
uds client changelog
```

The client asks for the channel, fetches available releases from UDS, shows them newest-first, and lets the administrator select the release. Only release notes are changed. The version, artifact files, signatures, and checksums stay unchanged.

### Withdraw a Release

```bash
uds client withdraw
```

The client asks for the channel, fetches available releases from UDS, shows them newest-first, and asks for confirmation before withdrawing the selected release. Withdrawn releases remain on disk but are not offered to clients.

### Copy a Release to Another Channel

```bash
uds client copy
```

The client asks for the source channel, fetches releases newest-first, lets the administrator select one, then asks for the target channel and confirmation. This is useful when a release should move from `beta` to `stable` without uploading the artifacts again.

### Retrieve Statistics

```bash
uds client stats
```

Statistics include update checks, downloads, estimated traffic, and per-platform counters. Single-node mode returns local statistics. Fleet mode keeps the same API and is designed to aggregate peer statistics.

### View Logs

```bash
uds client logs
uds client logs --follow
uds client logs --lines 500
uds client logs --level warn
uds client logs --no-color
```

The log viewer streams logs from the Admin API and colorizes them locally when the client is attached to an interactive terminal. UDS never writes ANSI color codes to journald or log files.

## Administration API Reference

The client uses the following Admin API endpoints. All administration requests require `Authorization: Bearer <admin_token>`.

- `GET /admin/v1/channels/{channel}/releases`
- `POST /admin/v1/channels/{channel}/releases`
- `PATCH /admin/v1/channels/{channel}/releases/{version}/changelog`
- `DELETE /admin/v1/channels/{channel}/releases/{version}`
- `POST /admin/v1/channels/{target_channel}/copy`
- `GET /admin/v1/channels/{channel}/stats`
- `GET /admin/v1/upload-policy`
- `GET /admin/v1/logs/recent?lines=200`
- `GET /admin/v1/logs/stream?lines=100`

Log API responses use newline-delimited JSON (`application/x-ndjson`) and require file logging plus `logging.admin_api.enabled = true`.

## Logging

UDS uses UTC timestamps and a log layout inspired by the MindWork AI Studio runtime:

```text
[2026-07-08T12:00:00.001Z] INFO [update_delivery_system::tls] [bind = "0.0.0.0:443"] starting HTTPS server with file-based TLS
```

Console logs remain human-readable. Files use typed NDJSON with one event per line. Terminal colors are only used when `logging.console.color = "always"` or when `color = "auto"` and stdout is an interactive terminal.

The `logging.client_ip` option controls whether the direct client socket IP address is included in request-related log events:

- `never`: Never log client IP addresses.
- `audit-security`: Log client IP addresses only for audit and security events. This is the default.
- `always`: Log client IP addresses for HTTP, audit, security, and other request-related events, including errors and panics.

UDS uses only the direct socket IP address. It does not evaluate `Forwarded` or `X-Forwarded-For`. When UDS runs behind a load balancer, the logged socket IP is therefore probably the load balancer's IP address. Events without an HTTP request context never include a client IP address.

The default production log base path is:

```text
<data_dir>/logs/events.ndjson
```

UDS uses `flexi_logger` for file rotation and cleanup. The configured `logging.file.path` is the base name for the rotating file set. With the default path above, the active file is `events_rCURRENT.ndjson`; archived files are named `events_r00000.ndjson`, `events_r00001.ndjson`, and so on. The active file is rotated when it exceeds `logging.file.max_size_mb`, which defaults to 100 MB, and UDS keeps up to `logging.file.max_archived_files` archives.

When UDS runs as a systemd service, systemd captures stdout and stderr automatically. Admins can inspect local service logs with:

```bash
journalctl -u uds -f
```

For remote or colorized log viewing, use:

```bash
uds client logs --follow
```

Normal systemd services cannot be attached to like `docker attach`, because stdout and stderr are connected when the service starts. UDS therefore keeps journald clean and provides colored viewing through the client.

Example systemd unit:

```ini
[Unit]
Description=MindWork AI Studio Update Delivery System
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=uds
Group=uds
ExecStart=/usr/local/bin/uds --config /etc/uds/config.toml
Restart=on-failure
RestartSec=5s
StandardOutput=journal
StandardError=journal
ReadWritePaths=/var/lib/uds /var/log/mindwork-ai/uds

[Install]
WantedBy=multi-user.target
```

## Fleet Operation

In fleet mode, multiple UDS instances can run behind a public load balancer. Public requests can hit any node. Admin mutations are accepted by one node and represented as replication events for the fleet.

Recommended fleet layout:

- Put all public nodes in a private network.
- Expose only the load balancer publicly.
- Use the same `cluster_token` on all nodes.
- Use a distinct persistent `data_dir` per node.
- Back up every node's `data_dir`, or use storage-level replication if your platform provides it.
- Terminate HTTP/3 at the load balancer if needed.

The internal peer API is enabled only in fleet mode and uses `cluster_token`. Do not expose internal endpoints to the public internet.

## Storage Layout

UDS stores data below `data_dir`. Artifacts are immutable and addressed by their SHA-256 digest, so copying a release between channels does not duplicate artifact bytes:

```text
blobs/
  sha256/
    ab/
      ab0123.../
        data
releases/
  stable/
    26.7.2/
      manifest.json
staging/
  uploads/
stats/
  events/
  processing/
  rollups/
node-id
```

Uploads stream to staging files and are only published after every artifact and the release metadata have passed validation. Manifests are written atomically. Changelog patches and withdrawals update the manifest without deleting artifacts.

Statistics are best effort and never make update checks or downloads fail. Events are queued with bounded capacity, persisted as UUID files, and compacted in crash-recoverable batches every 15 minutes or when the configured threshold is reached.

## Security Notes

- Generate long random `admin_token` and `cluster_token` values.
- Rotate tokens during maintenance windows and update every node consistently.
- Use HTTPS for public traffic. Plain HTTP is appropriate only behind trusted TLS termination.
- Keep `data_dir` private and backed up.
- Never place real tokens in scripts committed to version control.

## Development

Build and test:

```bash
cargo check
cargo test
```

Run locally:

```bash
cargo run --bin uds -- --single-node-mode
```
