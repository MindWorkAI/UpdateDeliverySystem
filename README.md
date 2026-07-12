# Update Delivery System

The Update Delivery System (UDS) distributes signed MindWork AI Studio updates to clients.

UDS can run as a small single-node system for one organization, or as a fleet behind a public load balancer. Each UDS node stores releases directly on the file system and exposes a Tauri-compatible update endpoint, download endpoint, private administration API, and optional internal fleet API.

## Current Status

This repository contains the initial Rust implementation. The single-node update path, release storage, changelog updates, downloads, local statistics, TLS file mode, and fleet-mode switches are implemented. Fleet discovery and replication endpoints are present as the first runtime shape; deep peer-to-peer artifact synchronization will be expanded without changing the public API.

## Supported Platforms and Production Use

For production deployments, we strongly recommend running UDS on a maintained Linux server. The macOS and Windows builds are intended for development, testing, and evaluation. The manual UDS Update Feature is available only for supported Linux installations.

Release binaries are provided for Linux on x86_64 and ARM64, macOS on Apple Silicon, and Windows on x86_64 and ARM64. Linux releases use the standard GNU target and require glibc; Alpine Linux and other systems that provide only musl are not supported. Each release documents the minimum glibc symbol version detected during its build.

The UDS Update Feature supports only single nodes installed by the configuration assistant as `/usr/local/bin/uds` and managed by systemd. It never updates unattended or on a schedule: an administrator must select and confirm one exact version with `uds client updates`. Manually launched processes must be replaced and restarted by their administrator. Containerized deployments must use their image and orchestrator rollout instead of replacing the executable inside a running container.

## UDS Release Process

The build and release workflow runs only when a `v<SemVer>` tag is pushed. The tag must match the package version in `Cargo.toml`, and the first section in `CHANGELOG.md` must use the matching `# UDS v<SemVer>` heading. Successful builds are submitted to VirusTotal before GitHub publishes them as a prerelease.

The release workflow requires these GitHub Actions secrets:

- `VIRUS_TOTAL_KEY`: API key used to submit the five release packages to VirusTotal.
- `UDS_UPDATE_SIGNING_KEY_PEM`: complete PEM-encoded Ed25519 private key used only to sign `latest.json`.

Generate the update-signing key pair on a trusted computer:

```bash
umask 077
openssl genpkey -algorithm ED25519 -out uds-update-private-key.pem
openssl pkey -in uds-update-private-key.pem -pubout -out uds-update-public-key.pem
```

Store the complete private PEM as the `UDS_UPDATE_SIGNING_KEY_PEM` secret and keep a protected offline backup. Never commit, upload, or share the private key. Commit the public PEM as `release/uds-update-public-key.pem`; the workflow verifies that it matches the private key before signing a release.

Each release contains the five platform packages, `SHA256SUMS`, `latest.json`, and `latest.json.sig`. The JSON manifest is signed byte-for-byte, so reformatting it after signing invalidates its signature. Promoting the prerelease to a regular GitHub release makes its manifest available through `/releases/latest/download/latest.json`.

## Quick Start: Single Node

The interactive wizard creates or updates a validated single-node configuration. It redacts
secrets in its review, protects changed files with a backup, and can optionally install a hardened
systemd service when run as root on a systemd-based Linux host:

```bash
uds server configure --config /etc/uds/config.toml
```

The wizard supports TLS termination outside UDS or existing certificate/key files. ACME and fleet
setup intentionally remain separate future workflows. Existing non-interactive server commands
remain supported.

Runtime server starts always require `--config`. Running `uds server` without a configuration
fails closed and points to the configuration wizard; UDS never creates or prints an ephemeral
owner credential during startup.

Create a configuration file:

```toml
mode = "single-node"
public_base_url = "https://updates.example.org"
data_dir = "/var/lib/uds"
owner_token_verifier = "sha512:<128-lowercase-hex-characters>"
channels = ["stable", "beta", "experimental", "mature"]

[public_api]
bind = "127.0.0.1:8080"

[public_api.tls]
mode = "off"

[admin_api]
bind = "127.0.0.1:8081"

[admin_api.tls]
mode = "off"
```

Start UDS:

```bash
uds server --config /etc/uds/config.toml --single-node-mode
```

TLS is configured independently per listener. Use `mode = "off"` on a non-loopback listener only in a trusted private network or behind a TLS-terminating proxy. UDS logs a warning because bearer tokens otherwise cross the network unencrypted.

## Configuration

```toml
mode = "fleet"
public_base_url = "https://updates.example.org"
data_dir = "/var/lib/uds"
owner_token_verifier = "sha512:<128-lowercase-hex-characters>"
cluster_token = "replace-with-a-long-random-cluster-token"
channels = ["stable", "beta", "experimental", "mature"]

[public_api]
bind = "0.0.0.0:443"

[public_api.tls]
mode = "files"
cert_path = "/etc/uds/tls/fullchain.pem"
key_path = "/etc/uds/tls/privkey.pem"

[admin_api]
bind = "10.20.0.12:8081"

[admin_api.tls]
mode = "off"

[fleet_api]
bind = "10.20.0.12:8082"
fleet_base_url = "http://10.20.0.12:8082"

[fleet_api.tls]
mode = "off"

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

[shutdown]
grace_period_seconds = 300
```

### Modes

- `single-node`: requires public and admin listeners and forbids `fleet_api`. This is the recommended mode for small organizations.
- `fleet`: additionally requires `fleet_api`, `fleet_base_url`, and `cluster_token`. Discovery advertises `fleet_base_url`, which must be an absolute HTTP(S) URL using a reachable host or IP, not a wildcard address.

Keep the public listener reachable from update clients, restrict the admin listener to administrator networks, and allow the fleet listener only between UDS nodes. Enforce those boundaries with host or network firewalls. The fleet API uses `/fleet/v1/replication/events`, `/fleet/v1/catalog`, and `/fleet/v1/stats/local/{channel}`. Plain HTTP is supported for private fleet networks but transmits the mandatory cluster token without encryption.

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

Returns basic service status, system mode, and node ID. After shutdown has started, the endpoint
returns `503 Service Unavailable` with `{"status":"draining"}` for any request that was already
accepted before the listener closed.

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
uds client tokens list
uds client tokens create
uds client tokens enable <uuid>
uds client tokens disable <uuid>
```

The first client run offers to create a local profile. The profile stores the UDS base URL, one personal admin token, and the default channel in a user-local config file. Owner tokens are never stored in client profiles or any separate client configuration. UDS hardens this file so only the current user can read or write it on Linux and macOS, and uses `icacls` for equivalent best-effort ACL hardening on Windows.

### UDS Update Feature

Run `uds client updates` to update a supported single node manually. The client initially lists only newer regular releases. The administrator may switch to a separate prerelease list, switch back, choose one exact version, review its version, build, and release notes, and then explicitly confirm or cancel. UDS never chooses a version, updates unattended, or runs updates on a schedule.

The authenticated Admin API implements:

- `GET /admin/v1/updates/releases?kind=regular|prerelease` for local node identity, current version/build, capability, and newer releases in exactly one category.
- `POST /admin/v1/updates` with a client-generated operation UUID, the local node ID, exact version, and explicit prerelease permission.
- `GET /admin/v1/updates/{operation_id}` for durable status (`queued`, `downloading`, `staged`, `applying`, `boot_confirmed`, `succeeded`, `rolled_back`, or `failed`).

Repeated identical POSTs are idempotent; reusing an operation UUID for different input returns `409 Conflict`, and a node accepts only one active operation. Polling tolerates the expected connection interruption while systemd restarts UDS.

UDS retrieves official GitHub releases page by page, excludes drafts and invalid or non-newer SemVer tags, and keeps regular releases separate from prereleases. Before staging, it verifies the Ed25519 signature over `latest.json` with the embedded public key and checks schema, tag, Linux platform, architecture, update support, archive size, and SHA-256. The unprivileged service atomically stores only the verified manifest, signature, archive, and operation data below `data_dir/self-update/operations`.

The configuration assistant installs a hardened readiness-aware `uds.service`, a root `uds-update.service` oneshot, and a bounded `uds-update.path` trigger. The helper verifies the signed inputs again, reads only the signed regular executable entry from the archive, rejects unsafe paths and links, retains the old executable as `/usr/local/bin/uds.previous`, and restarts UDS. If the new service does not remain active for 30 seconds, the helper restores the previous executable. A successful update retains `uds.previous` until the next successful update; a successful rollback restores the regular `uds` path and leaves no `uds.previous`.

Other executable locations, manually started services, containers, macOS, Windows, and fleet mode do not support this feature.

#### Planned Fleet Updates

Fleet updates remain planned. Administrators will select nodes through the load balancer, submit stable operations, configure batches, require health checks, and pause between batches. The coordinator will resolve stable node IDs to private Fleet API addresses and direct work to each target; clients will not need private addresses.

This requires complete peer discovery, stale-peer expiry, and real replication first. Until those prerequisites exist, fleet mode explicitly reports the UDS Update Feature as unavailable. The signed release manifest and signing process are the shared trust foundation for today's single-node workflow and this later fleet workflow.

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

Normal administration accepts either an enabled `uds_admin_v1_…` token or the break-glass `uds_owner_v1_…` token in `Authorization: Bearer <token>`.

- `GET /admin/v1/channels/{channel}/releases`
- `POST /admin/v1/channels/{channel}/releases`
- `PATCH /admin/v1/channels/{channel}/releases/{version}/changelog`
- `DELETE /admin/v1/channels/{channel}/releases/{version}`
- `POST /admin/v1/channels/{target_channel}/copy`
- `GET /admin/v1/channels/{channel}/stats`
- `GET /admin/v1/upload-policy`
- `GET /admin/v1/logs/recent?lines=200`
- `GET /admin/v1/logs/stream?lines=100`

The following token-management endpoints accept only the owner token. A valid admin token receives `403`; missing or invalid credentials receive `401`. Their responses include `Cache-Control: no-store` and never expose stored verifiers.

- `GET /admin/v1/admin-tokens`
- `POST /admin/v1/admin-tokens` with `{ "name": "…", "reason": "…" }`
- `PATCH /admin/v1/admin-tokens/{uuid}` with `{ "enabled": false, "reason": "…" }`

The creation response keeps extensible metadata separate from the one-time secret:

```json
{
  "metadata": {
    "id": "4aaf79c9-08c9-40e6-a16f-13c19023ad83",
    "name": "Release automation",
    "created_at": "2026-07-11T18:00:00Z",
    "creation_reason": "Publish approved releases",
    "enabled": true,
    "status_history": []
  },
  "token": "uds_admin_v1_4aaf79c9-08c9-40e6-a16f-13c19023ad83_<base64url-secret>"
}
```

This response is the only place the new token secret appears. Names and creation reasons are immutable. Every real enable/disable transition appends its required reason to immutable history; repeating the current state is idempotent.

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
ExecStart=/usr/local/bin/uds server --config /etc/uds/config.toml
Restart=on-failure
RestartSec=5s
TimeoutStopSec=330s
StandardOutput=journal
StandardError=journal
ReadWritePaths=/var/lib/uds /var/log/mindwork-ai/uds

[Install]
WantedBy=multi-user.target
```

### Graceful shutdown

On `SIGTERM` (the signal used by `systemctl stop`) or `SIGINT`, UDS immediately closes its listener
and reports itself as draining. Existing artifact downloads and release uploads may finish for up to
`shutdown.grace_period_seconds`, which defaults to 300 seconds. Once the deadline expires, UDS logs
each remaining transfer and closes its connection. A second shutdown signal skips the remaining
grace period.

Configure the load balancer to use `/health`, remove an unreachable or non-2xx node from rotation,
and retry failed connection attempts on another healthy node. Its detection interval still governs
how quickly it proactively removes a draining node; closing the UDS listener prevents new
connections in the meantime. Set systemd's `TimeoutStopSec` higher than the UDS grace period so the
service has time to write its final transfer and shutdown events.

## Fleet Operation

In fleet mode, multiple UDS instances can run behind a public load balancer. Public requests can hit any node. Admin mutations are accepted by one node and represented as replication events for the fleet.

Recommended fleet layout:

- Put all public nodes in a private network.
- Expose only the load balancer publicly.
- Use the same `cluster_token` on all nodes.
- Use a distinct persistent `data_dir` per node.
- Back up every node's `data_dir`, or use storage-level replication if your platform provides it.
- Terminate HTTP/3 at the load balancer if needed.

The internal peer API is enabled only in fleet mode and uses `cluster_token`. Token-store synchronization uses `/fleet/v1/auth/admin-tokens`; it includes verifiers and must never be exposed outside the protected fleet network. Do not expose internal endpoints to the public internet.

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
auth/
  admin-tokens.json
node-id
```

Uploads stream to staging files and are only published after every artifact and the release metadata have passed validation. Manifests are written atomically. Changelog patches and withdrawals update the manifest without deleting artifacts.

Statistics are best effort and never make update checks or downloads fail. Events are queued with bounded capacity, persisted as UUID files, and compacted in crash-recoverable batches every 15 minutes or when the configured threshold is reached.

## Security Notes

- Let `uds server configure` generate the 512-bit owner secret. It prints the owner token exactly once and stores only its `sha512:` verifier; put the token in a password manager.
- Owners should use personal admin tokens for daily work. Fetch the owner token only to run `uds client tokens …`; the prompt is hidden and no command-line owner-token option exists.
- Admin-token secrets are generated server-side with 512 bits of randomness, returned once, and represented at rest only by versioned SHA-512 verifiers. The auth directory and store use private permissions and atomic replacement.
- Never log tokens, secrets, or verifiers. Deactivated-token attempts are security events containing only the token ID.
- Generate a long random `cluster_token` and update every fleet node consistently.
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
cargo run --bin uds -- server --single-node-mode
```
