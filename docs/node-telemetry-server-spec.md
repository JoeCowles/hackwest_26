# macOS Storage Monitoring: Node Telemetry and Central Server Specification

Status: Historical requirements draft. The shared Google Doc is authoritative; see its Server Spec sections 14 and 15 for current API contracts and delivery status.  
Scope: Central server and the contract consumed by node collectors  
Out of scope: Implementation of the node daemon

## 1. Goals

The system provides administrators with a current, explainable view of storage health across a cluster of macOS nodes. A daemon on each node sends observations to a central server. The server validates, stores, aggregates, displays, and alerts on those observations.

The first release should answer five questions quickly:

1. Which nodes, filesystems, mounts, or physical devices are unhealthy or unreachable?
2. How much storage is used, available, reserved, or quota-limited?
3. What read/write throughput and operation rate is each node producing?
4. Are local APFS and shared NFS volumes behaving normally?
5. Which conditions need administrator attention now?

The server stores telemetry and metadata only. It must not ingest file contents. File paths, process arguments, usernames, and other sensitive identifiers should be excluded by default or pseudonymized.

## 2. Key design principles

- Model the storage dependency graph. A node contains physical devices, partitions, APFS containers, APFS volumes, snapshots, and mounts. These are separate objects linked by stable IDs.
- Preserve meaning. Observed counters, derived rates, health assessments, and alerts are different records.
- Never turn missing information into zero. Every observation carries a state: `ok`, `unsupported`, `permission_denied`, `stale`, `timeout`, `parse_error`, or `failed`.
- Preserve source and scope. Every metric identifies its collection source, unit, timestamp, object, and whether it is node-wide, device-wide, volume-wide, mount-wide, or user-wide.
- Expect partial reports. A node can report healthy local storage while an NFS probe times out.
- Avoid blocking the ingest path. Slow alert delivery, rollups, and dashboard updates run asynchronously.
- Make writes idempotent. Retransmitted telemetry must not create duplicate samples or events.

## 3. Storage object model

The central server stores an inventory graph reported by each node:

```text
node
  -> physical device
    -> partition or provider layer
      -> APFS physical store
        -> APFS container
          -> APFS volume
            -> snapshot
              -> mount

node
  -> NFS mount
    -> server endpoint + exported path
```

Each object has a server-stable UUID, a node-local identity, a type, properties, and zero or more parent relationships. Device numbers and mount paths are properties, not stable IDs. The server should retain inventory generations so replacement, remount, and counter-reset events can be distinguished from normal counter changes.

## 4. Metrics to collect

### 4.1 Node heartbeat and context: MVP

| Metric | Type/unit | Suggested cadence | Purpose |
|---|---|---:|---|
| `node_up` | boolean | 5 seconds | Basic heartbeat |
| `node_boot_id` | opaque ID | heartbeat | Separates counter generations after reboot |
| `node_uptime_seconds` | gauge, seconds | 30 seconds | Detects reboot and instability |
| `node_agent_version` | string | heartbeat | Compatibility and rollout visibility |
| `node_os_version`, `node_os_build` | string | inventory/change | Explains feature availability |
| `node_clock_offset_seconds` | gauge, seconds | 5 minutes | Detects timestamps that cannot be trusted |
| `node_thermal_pressure` | enum | 30 seconds | Explains throttled storage performance |
| `node_memory_pressure` | enum | 30 seconds | Explains paging-driven I/O |
| `node_swap_in_bytes_total`, `node_swap_out_bytes_total` | counter, bytes | 30 seconds | Workload context, not disk health |

The server derives `last_seen_age_seconds` from ingest time and marks a node offline after a configurable missed-heartbeat interval.

### 4.2 Mount and filesystem inventory: MVP

For every local or network mount:

- Stable object ID, filesystem ID, source, mount point, filesystem type, mount flags, read-only state, and inventory generation.
- Capacity: `capacity_bytes`, `used_bytes`, `free_bytes`, and `available_bytes`.
- File accounting when meaningful: `files_total`, `files_free`, and `files_used`.
- Capability flags: quota support, case sensitivity, ownership support, encryption capability, and local/network/removable classification.
- Observation state and age. Cached NFS capacity is valid data only when marked `stale` with the original observation timestamp.

`free_bytes` and `available_bytes` remain separate. For APFS, container free space, volume-used space, reserve, quota, and purgeable estimates remain separate because space sharing prevents simple addition across volumes.

### 4.3 I/O performance: MVP

Collect cumulative counters from each physical block-storage driver and derive rates on the server or node from two samples in the same generation:

| Metric | Type/unit | Suggested cadence |
|---|---|---:|
| `device_read_bytes_total`, `device_write_bytes_total` | counter, bytes | 5 seconds |
| `device_read_ops_total`, `device_write_ops_total` | counter, operations | 5 seconds |
| `device_read_errors_total`, `device_write_errors_total` | counter, errors | 5 seconds |
| `device_retries_total` | counter, retries | 5 seconds |
| `device_read_time_ns_total`, `device_write_time_ns_total` | counter, nanoseconds | 5 seconds |

Derived values shown in the dashboard:

- Read and write throughput in bytes/second and GB/second.
- Read and write operations/second.
- Mean transfer size in bytes/operation.
- Mean accounted driver time in milliseconds/operation.
- Error and retry rates.

The UI must call driver timing "mean accounted driver time," not application latency. Generic macOS counters do not provide portable latency percentiles, current queue depth, or true utilization. Physical-device counters also cannot be assigned to individual APFS volumes without a separate attribution source.

### 4.4 Physical device health: MVP when available

Inventory fields include model, vendor, serial or pseudonymous fingerprint, firmware, protocol, connection type, logical/physical block sizes, removable state, and writable state.

SMART/NVMe health observations include:

- Overall health result and collection availability.
- Critical warning bits.
- Current and warning temperatures.
- Available spare and spare threshold.
- Endurance percentage used.
- Media/data-integrity error count and error-log count.
- Unsafe shutdowns, power cycles, power-on hours, and thermal-throttling history.
- ATA reallocated, pending, and uncorrectable sectors where the vendor exposes meaningful values.

SMART is optional per device. `unsupported` or `permission_denied` is not equivalent to healthy. Provider-specific RAID or NAS health should be represented as a separate provider observation rather than inferred from a virtual disk.

### 4.5 APFS-specific health and accounting: MVP inventory, stretch diagnostics

MVP fields:

- Physical-store, container, volume, and volume-group relationships.
- Volume role, UUID, mount state, encryption/lock state, reserve, and quota.
- Snapshot count, oldest snapshot age, newest snapshot age, and total metadata available from the platform.
- Container capacity/free space and per-volume used space.

Stretch fields:

- Purgeable-space estimates and maintained directory statistics where supported.
- FileVault status as recovery/security context.
- Last read-only filesystem verification result, scope, duration, and age.
- Backup/local-snapshot freshness.

Filesystem verification and synthetic write tests are operator-triggered jobs, not periodic metrics, because they can be expensive or disruptive.

### 4.6 NFS health and performance: MVP

For every NFS mount, collect:

- Server endpoint, export, negotiated NFS version/minor version, transport, security flavor, I/O sizes, cache options, and effective mount options.
- Mount status: mounted, not responding, dead, recovering, or unmounted.
- `nfs_outstanding_request_entries` and `nfs_oldest_request_wait_seconds` when the native status interface is available.
- Node-wide NFS client counters: operation totals by protocol operation, RPC timeouts, retransmissions, malformed/unexpected replies, protocol errors, cache hits/misses, and paging activity.
- Capacity with observation age and timeout state.
- Optional bounded active probe: result, operation tested, duration, identity class, and error category.

Node-wide NFS counters must not be duplicated as if each mount generated them. A server endpoint being reachable does not prove that a mounted export is usable. Native macOS supports NFS through v4.1 but does not currently expose a native pNFS layout/data-server path, so true pNFS layout, stripe, recall, and data-server metrics require server-side telemetry or a different client and are out of scope for the macOS daemon.

### 4.7 Quotas and user attribution: MVP capability, stretch detail

For filesystems that expose quota data:

| Metric | Scope |
|---|---|
| `quota_limit_bytes`, `quota_used_bytes`, `quota_available_bytes` | user/group on filesystem |
| `quota_soft_limit_bytes`, `quota_hard_limit_bytes` | user/group on filesystem |
| `quota_grace_expires_at` | user/group on filesystem |
| `quota_files_used`, `quota_files_limit` | user/group on filesystem |

Every record identifies the subject as a local opaque UID/GID or server-generated pseudonym, the filesystem, and the identity used for the query. Unavailable quota data means unknown, not unlimited.

Per-user I/O attribution is a stretch feature. If collected, send aggregate byte/operation deltas by opaque user ID and process category, not filenames or command-line arguments.

### 4.8 Security and anomalous behavior: stretch

The server can evaluate normalized events and rates for:

- Repeated permission or authentication failures.
- Large changes in per-user write volume relative to that user's baseline.
- Sudden mass-delete or rename activity when a suitable event source is enabled.
- Access to protected or unusual mount classes.
- New removable devices or unexpected mount-option changes.
- Sustained quota exhaustion attempts.
- Device detach/reconnect loops and repeated NFS recovery cycles.

Alerts should describe observable behavior, not label a person "nefarious." Endpoint Security data requires Apple entitlement and is an optional integration. Paths and file contents are never transmitted by default.

## 5. Telemetry envelope

Collectors send batches. The server deduplicates a batch using `(node_id, boot_id, sequence)`.

```json
{
  "schema_version": "1.0",
  "node_id": "node_01J...",
  "boot_id": "boot_01J...",
  "sequence": 1842,
  "observed_at": "2026-09-12T20:15:31.442Z",
  "sent_at": "2026-09-12T20:15:31.781Z",
  "agent": { "version": "0.1.0", "os_build": "25A..." },
  "inventory_generation": 7,
  "samples": [
    {
      "object_id": "device_01J...",
      "name": "device_read_bytes_total",
      "kind": "counter",
      "value": 984237188,
      "unit": "bytes",
      "state": "ok",
      "source": "iokit",
      "observed_at": "2026-09-12T20:15:30.000Z"
    }
  ],
  "events": []
}
```

Metric names and units come from a versioned server catalog. Unknown additive fields are ignored and retained in the raw batch for forward compatibility. Invalid required fields reject the batch with field-level errors.

## 6. HTTP API

All routes are under `/api/v1`. JSON is the initial wire format. HTTPS is required outside local development.

### Collector-facing routes

| Method and route | Purpose |
|---|---|
| `POST /nodes/enroll` | Exchange a single-use enrollment token for a node ID and credential |
| `PUT /nodes/{node_id}/inventory` | Replace the node's current object graph using an inventory generation |
| `POST /nodes/{node_id}/telemetry` | Ingest an idempotent batch of samples and events |
| `POST /nodes/{node_id}/goodbye` | Optional graceful shutdown/uninstall signal |

Successful telemetry ingestion returns `202 Accepted`, the accepted sequence, server time, and optional configuration version. Use `400` for malformed payloads, `401/403` for authentication/authorization, `409` for identity or generation conflicts, `413` for oversized batches, and `429` with `Retry-After` for backpressure.

### Administrator-facing routes

| Method and route | Purpose |
|---|---|
| `GET /nodes` | Cluster summary with health and last-seen state |
| `GET /nodes/{node_id}` | Node inventory and current health |
| `GET /objects/{object_id}/metrics` | Time range query for one storage object |
| `GET /filesystems` | Capacity, quota, and availability summary |
| `GET /alerts` | Active and historical alerts |
| `POST /alerts/{alert_id}/acknowledge` | Record administrator acknowledgement |
| `GET /events` | Filtered operational/security event stream |
| `GET /stream` | Server-sent events for live dashboard updates |

The dashboard uses the administrator API; it does not read the database directly.

## 7. Authentication and transport

- Enrollment uses a short-lived, single-use token created by an administrator.
- After enrollment, each node receives its own revocable credential. A compromised node cannot write telemetry for another node.
- Requests include a timestamp and unique request ID. The server rejects excessive clock skew and replayed IDs.
- API credentials are stored in the macOS Keychain by the future daemon.
- TLS protects telemetry in transit. Mutual TLS is a future option, not required for the hackathon MVP.
- Administrator access is separate from node ingest credentials.

## 8. Central server architecture

Recommended hackathon implementation:

```text
node daemons
  -> HTTP ingest API
    -> validation + idempotency
      -> durable telemetry store
        -> rollup worker
        -> alert evaluator
          -> dashboard API / live stream
          -> notification adapter
```

Use a Rust macOS application with an optional headless mode for the API and workers. The existing web dashboard is a separate API client. Use SQLite in WAL mode behind a storage interface. The monitoring database is small compared with the monitored filesystems; retention and rollups still keep it bounded.

Suggested storage tables:

- `nodes`, `node_credentials`, `objects`, `object_edges`, `inventory_generations`
- `metric_catalog`, `metric_samples`, `events`
- `alert_rules`, `alert_instances`, `alert_deliveries`
- `hourly_rollups`, `daily_rollups`

Keep high-frequency raw samples for 24 hours, five-minute rollups for 30 days, and hourly rollups for one year by default. Make retention configurable.

## 9. Health model and initial alert rules

Do not collapse all health into one unexplained boolean. Each object has dimensions:

- Availability
- Capacity
- Performance
- Media health
- Filesystem integrity
- Security/activity
- Recovery readiness
- Telemetry freshness

Each dimension is `healthy`, `warning`, `critical`, or `unknown`, with reasons and supporting observations. The overall status is the worst known dimension, while `unknown` is shown separately so missing telemetry remains visible.

Initial configurable rules:

| Rule | Warning | Critical |
|---|---:|---:|
| Node heartbeat age | 30 seconds | 90 seconds |
| Filesystem available capacity | below 15% | below 5% |
| Filesystem forecast to full | within 14 days | within 3 days |
| NFS oldest outstanding request | 5 seconds | 30 seconds |
| NFS mount status | recovering/not responding | dead |
| SMART/NVMe warning | degradation indicator | critical warning or media errors increasing |
| Device I/O errors | first increase | sustained/repeated increase |
| Temperature | device-specific warning | device-specific critical threshold |
| Quota use | above 85% | above 95% |

Rules use debounce, cooldown, and recovery notifications. Capacity alerts should require consecutive observations, not a single transient sample. Notifications start with a generic webhook adapter; SMS can be demonstrated through a provider such as Twilio without coupling alert evaluation to that provider.

## 10. Dashboard MVP

The primary screen should show:

- Cluster capacity: total, used, available, and projected exhaustion.
- Node cards: health dimensions, last seen, aggregate read/write GB/s, and active alerts.
- Storage topology: device to container to volume to mount relationships.
- Local versus NFS volume table with capacity, status, and observation freshness.
- Time-series charts for throughput, operation rate, errors, retries, and NFS waits.
- Alert inbox with severity, evidence, first/last occurrence, acknowledgement, and recovery state.

Every number should expose its timestamp, unit, source, scope, and collection state on inspection.

## 11. MVP boundary for the hackathon

Build first:

1. Node enrollment and authenticated telemetry ingestion.
2. Inventory graph for nodes, devices, APFS containers/volumes, and NFS mounts.
3. Heartbeat, capacity, block I/O counters/rates, SMART summary, and NFS status.
4. SQLite persistence, retention, and five-minute rollups.
5. Cluster dashboard and five core alerts: node offline, capacity low, NFS unavailable, device errors, and SMART warning.
6. Webhook notification adapter and a short seeded/demo data mode.

Defer until the core path works:

- Per-user I/O baselines and behavioral anomaly detection.
- Endpoint Security integration.
- Packet capture, `fs_usage`, sysdiagnose, and verification-job orchestration.
- Provider plugins for OpenZFS, hardware RAID, and NAS management APIs.
- Server-side pNFS telemetry.
- PostgreSQL/TimescaleDB and multi-server high availability.

## 12. Open decisions

Proposed defaults, subject to team agreement:

- Server language: Rust.
- Demo database: SQLite WAL behind a storage interface.
- Live dashboard updates: Server-Sent Events rather than WebSockets.
- Expected telemetry cadence: 5-second device counters, 5-second heartbeat, 30-second capacity/NFS status, 5-minute SMART and detailed accounting.
- Privacy: opaque user IDs; no paths, filenames, file contents, or command lines in routine telemetry.
- Notification MVP: webhook first, SMS adapter second.

## 13. Future work

- Provider SDK for OpenZFS, RAID controllers, and NAS platforms.
- Server-fed pNFS layout/data-server metrics from systems that implement pNFS.
- Baseline and seasonal anomaly detection for users, devices, and mounts.
- Forecasting based on growth rate, workload schedule, snapshots, and purgeable space.
- Signed remote configuration and safe, operator-approved diagnostic jobs.
- Multi-tenant organizations, role-based access, audit logs, and SSO.
- Prometheus/OpenTelemetry export and integration with existing observability stacks.
- High-availability ingest, sharded time-series storage, and long-term object-store archival.
