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
channels = ["stable", "beta", "experimental", "lts"]

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
channels = ["stable", "beta", "experimental", "lts"]

[tls]
mode = "files"
cert_path = "/etc/uds/tls/fullchain.pem"
key_path = "/etc/uds/tls/privkey.pem"

[cluster]
node_id_path = "node-id"
broadcast_addr = "255.255.255.255:44231"
broadcast_interval_seconds = 30
reconcile_interval_seconds = 300
```

### Modes

- `single-node`: disables broadcast discovery, peer reconciliation, replication, and internal peer routes. This is the recommended mode for small organizations.
- `fleet`: enables the internal fleet shape and background broadcast task. This mode requires `cluster_token`.

The CLI flag `--single-node-mode` overrides the configuration file and forces single-node mode.

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

## Administration API

All administration requests require:

```http
Authorization: Bearer <admin_token>
```

### Upload a Release

```http
POST /admin/v1/channels/{channel}/releases
Content-Type: multipart/form-data
```

The multipart request must contain one `metadata` field with JSON and one file field per platform.

Example metadata:

```json
{
  "version": "26.7.2",
  "pub_date": "2026-07-07T12:00:00Z",
  "notes": "Fixed update delivery for enterprise deployments.",
  "platforms": {
    "windows-x86_64": {
      "file_field": "windows_x86_64_bundle",
      "file_name": "MindWork-AI-Studio_26.7.2_windows_x86_64.zip",
      "signature": "signature-content"
    },
    "darwin-aarch64": {
      "file_field": "darwin_aarch64_bundle",
      "file_name": "MindWork-AI-Studio_26.7.2_darwin_aarch64.tar.gz",
      "signature": "signature-content"
    }
  }
}
```

Example upload:

```bash
curl -X POST https://updates.example.org/admin/v1/channels/stable/releases \
  -H "Authorization: Bearer ${UDS_ADMIN_TOKEN}" \
  -F 'metadata=@metadata.json;type=application/json' \
  -F 'windows_x86_64_bundle=@MindWork-AI-Studio_26.7.2_windows_x86_64.zip' \
  -F 'darwin_aarch64_bundle=@MindWork-AI-Studio_26.7.2_darwin_aarch64.tar.gz'
```

### Correct a Changelog

Use this when a typo or wording issue must be corrected without rebuilding release artifacts.

```bash
curl -X PATCH https://updates.example.org/admin/v1/channels/stable/releases/26.7.2/changelog \
  -H "Authorization: Bearer ${UDS_ADMIN_TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{"notes":"Fixed the corrected release notes text."}'
```

Only release notes are changed. The version, artifact files, signatures, and checksums stay unchanged.

### Withdraw a Release

```bash
curl -X DELETE https://updates.example.org/admin/v1/channels/stable/releases/26.7.2 \
  -H "Authorization: Bearer ${UDS_ADMIN_TOKEN}"
```

Withdrawn releases remain on disk but are not offered to clients.

### Copy a Release to Another Channel

```bash
curl -X POST https://updates.example.org/admin/v1/channels/beta/copy \
  -H "Authorization: Bearer ${UDS_ADMIN_TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{"source_channel":"stable","version":"26.7.2"}'
```

This is useful when a release should move from `beta` to `stable` without uploading the artifacts again.

### Retrieve Statistics

```bash
curl https://updates.example.org/admin/v1/channels/stable/stats \
  -H "Authorization: Bearer ${UDS_ADMIN_TOKEN}"
```

Statistics include update checks, downloads, estimated traffic, and per-platform counters. Single-node mode returns local statistics. Fleet mode keeps the same API and is designed to aggregate peer statistics.

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

UDS stores data below `data_dir`:

```text
releases/
  stable/
    26.7.2/
      manifest.json
      MindWork-AI-Studio_26.7.2_windows_x86_64.zip
stats/
  raw/
  rollups/
node-id
```

Manifests are written atomically. Changelog patches and withdrawals update the manifest without deleting artifacts.

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
