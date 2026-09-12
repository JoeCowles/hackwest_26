# macOS Storage Monitoring: NFS Specification

*Status: Draft v0.1*

*Scope: The physical NFS server + attached drive under test, and the node collector daemon's NFS telemetry path back to the central server. Implements Server Spec §3 (`node -> NFS mount -> server endpoint + exported path`), §4.6, and uses the §6/§7 ingest contract.*

> Section references prefixed "Server Spec" point at the *Server Spec* tab of the team design doc
> ([TCL challenge 2026](https://docs.google.com/document/d/1JnsZHlYPXQ1IRsMSEHeboICzqqviKJylI7gRsuUK13E/edit)),
> which defines the storage object model, telemetry envelope, ingest API, and alert thresholds this spec builds on.
> Per `AGENTS.md` the Google Doc is the shared source of truth. This file mirrors the
> doc's "NFS spec" tab; change the doc first and reflect it here, not the other way round.

## 1. Purpose

Give the monitoring system a real, shared NFS volume to observe end-to-end, and define exactly how the node daemon collects NFS health/performance data and ships it to the central server. This is not a general-purpose NFS product — it is the minimum real NFS deployment needed to exercise and demo Server Spec §4.6.

## 2. Topology: two independent connections

There are two separate machine roles and two separate network links. Do not conflate them:

| Link | Endpoints | Carries |
|---|---|---|
| A. NFS mount | NFS client node <-> NFS server's `ip:2049` | Actual file protocol traffic (READ/WRITE/GETATTR/etc.) |
| B. Telemetry | Every node's collector daemon <-> central server's `ip:port` | HTTPS JSON telemetry batches (Server Spec §5/§6) |

A single machine can be an NFS *server* (exporting the attached drive), an NFS *client* (mounting someone else's export), and always runs the *collector daemon* (talking to link B) regardless of its NFS role. The daemon's job on link A is passive observation of mount health; it is never a required hop for actual file I/O.

## 3. NFS server setup (macOS reference)

- **Drive**: external USB/Thunderbolt drive, formatted APFS (or ExFAT if cross-platform client access to the *unmounted* raw device is ever needed — APFS is the default and the recommended choice since it matches Server Spec §4.5 accounting).
- **Export directory**: a dedicated top-level directory on that volume, not the volume root, so ownership/permissions stay contained.
- **`/etc/exports`**: one line per export, explicit client ACL (no bare `-network` wildcards for the demo), and an explicit identity-mapping policy:
  ```
  /Volumes/<drive>/<export> -mapall=<uid>:<gid> -network=<demo-subnet> -mask=<mask>
  ```
  `-mapall` is simplest for a demo (all client access maps to one identity); document this as a deliberate simplification, not a hidden default, per Server Spec §2 ("never turn missing information into zero" applies to config assumptions too).
- **Start/verify**: `sudo nfsd enable && sudo nfsd start`, confirm with `sudo nfsd status` and `showmount -e <server-ip>` from a client.
- **Port**: `2049/tcp` only. NFSv4.1 carries mount and lock operations over the same port, so there is no separate `mountd`/`rpcbind` (portmapper) port to open. Pin and firewall `2049/tcp` for the demo network.
- **Quota support**: macOS's NFS server does not expose meaningful per-user NFS quota data to clients. Record this as `unsupported` in the mount's capability flags (§4.2) rather than omitting the field — the Server Spec treats a missing capability as a fact to be recorded, not a zero.

## 4. NFS protocol version and security

**NFSv4.1, TCP, `sec=sys` (AUTH_SYS).** v4.1 is the version the Server Spec targets — §4.6 states macOS's native client supports NFS through v4.1, and the whole monitoring contract (negotiated version/minor version, stateful mount status, the pNFS exclusion) is written against it. It also needs only port 2049, with no separate mountd/portmapper round trip.

- **Risk to verify early:** macOS's `nfsd` *server-side* v4.1 support is far less exercised than its client-side support. Confirm the server actually negotiates v4.1 during initial setup rather than assuming it.
- **`sec=krb5` is out of scope** for the hackathon — real setup cost, and the Server Spec requires only that the negotiated security flavor be *recorded*, not that any particular flavor be used.
- Whatever the client actually negotiates (not what was requested) is what gets recorded — see §6 below.

## 5. Client mount configuration

- Mount explicitly, don't rely on defaults: `mount_nfs -o vers=4.1,proto=tcp,sec=sys,rsize=<n>,wsize=<n> <server-ip>:<export> <mountpoint>`.
- Have the daemon read back the *effective* negotiated options after mount (macOS can silently downgrade version or transport), not the options requested in the mount command — this effective set is what populates `effective_mount_options` in §4.6.
- Every monitored node that mounts the export runs the same collector daemon; the export's own host also runs it, reporting its local drive under §4.2/4.3/4.4/4.5 as an ordinary local mount, separate from any NFS-mount telemetry.

## 6. Node daemon: NFS telemetry collection (implements Server Spec §4.6)

For each configured NFS mount, on each poll cycle:

| Field | macOS collection source | Cadence |
|---|---|---|
| Server endpoint, export path, negotiated version/transport/security, I/O sizes, effective mount options | `mount_nfs -v` output at mount time; re-verify periodically since options can change across a remount | inventory / on change |
| Mount status (`mounted`, `not responding`, `dead`, `recovering`, `unmounted`) | derived from `nfsstat -m` + `getmntinfo` reachability; see status-derivation rules below | 30 seconds |
| `nfs_outstanding_request_entries`, `nfs_oldest_request_wait_seconds` | native NFS status interface if available on the running macOS version; mark `unsupported` where it is not, never fabricate a zero | 30 seconds |
| Node-wide client counters: op totals by operation, RPC timeouts, retransmissions, malformed/unexpected replies, protocol errors, cache hits/misses, paging activity | `nfsstat -c` (client-wide, not per-mount — do not duplicate these onto every mount as if each generated its own copy, per §4.6) | 5 seconds |
| Capacity (`capacity_bytes`, `used_bytes`, `free_bytes`, `available_bytes`) with observation age/timeout state | `statfs`/`getattrlist` on the mount point; a stalled/hung mount must return a `stale`-tagged cached value with its original timestamp, never a silent retry-forever | 30 seconds |
| Optional bounded active probe (result, operation tested, duration, identity class, error category) | a single small, timed `stat()` (or bounded read) against a known path in the export, executed off the hot ingest path with a hard timeout and rate limit so it can never be mistaken for load generation | configurable, default 60 seconds |

**Mount status derivation:**
- `mounted` — last capacity/probe observation succeeded within its expected window.
- `recovering` — client-visible retransmissions/timeouts are actively increasing but the mount has not exceeded the dead threshold.
- `not responding` — outstanding requests exist and `nfs_oldest_request_wait_seconds` exceeds the warning threshold (Server Spec §9: 5s warning).
- `dead` — exceeds the critical threshold (§9: 30s) or the OS reports the mount as hard-down.
- `unmounted` — intentionally absent from `getmntinfo`; distinct from every failure state above.

**Explicitly out of scope**: true pNFS layout/stripe/recall/data-server metrics. macOS's native client has no pNFS data-server path, so this daemon reports ordinary NFSv4.1 client health only, per Server Spec §4.6.

## 7. Node daemon: telemetry transport to the central server

- **Enrollment**: on first run, exchange a single-use administrator-issued token for a node ID + revocable credential (Server Spec §7), store the credential in the macOS Keychain, never in a config file.
- **Daemon configuration fields**:
  - `central_server_url` (host:port, separate and unrelated to any NFS server address)
  - path/reference to the enrolled node credential
  - list of local NFS mounts to watch (mount point, or discover all NFS-type mounts from `getmntinfo` automatically)
  - poll intervals per metric class, defaulting to the cadences in the table above and in Server Spec §4.1/4.3
- **Batching**: samples accumulate locally and POST to `/api/v1/nodes/{node_id}/telemetry` as an idempotent batch keyed by `(node_id, boot_id, sequence)`, per Server Spec §5.
- **Backpressure/offline handling**: if the central server is unreachable, buffer locally up to a bounded size/time limit, then drop oldest — the local monitoring loop (mount-status derivation, alerting groundwork) must keep running even with zero connectivity to the central server; a lost telemetry link must never block local NFS-mount observation.
- **Transport**: HTTPS; TLS is required outside local dev, matching Server Spec §7. Mutual TLS is future work, not required for the hackathon MVP.

## 8. Failure modes to model explicitly

Three links fail independently and must be distinguishable in the resulting telemetry, not collapsed into one "broken" signal:

1. **NFS server down / export unreachable** — client-side mount status goes to `not responding`/`dead`; local daemon keeps running and keeps trying to reach the central server to report exactly that.
2. **Network partition between client and NFS server only** — same client-visible symptoms as (1); the daemon cannot and should not try to distinguish "server down" from "network down" from the client side alone — report the observable symptom, not a diagnosis.
3. **Central server / telemetry link down** — NFS mount itself may be perfectly healthy; the daemon buffers/drops telemetry per §7 but its local view of NFS health is unaffected. This is the "expect partial reports" principle from Server Spec §2 applied concretely.

## 9. MVP boundary for this sub-component

Aligned with Server Spec §11.

**Build first:**
- One macOS NFS server exporting one directory on one attached external drive.
- NFSv4.1 over TCP with `sec=sys`, server-side negotiation verified early.
- Collector daemon running on the NFS server node and at least one separate client node.
- Mount status, capacity, node-wide client counters, and the bounded active probe, all shipped to the central server over HTTPS.
- The two "NFS unavailable" and generic node-offline alerts from Server Spec §11 wired against this data.

**Defer:**
- Multiple exports or multi-server NFS topologies.
- `sec=krb5` / Kerberos.
- pNFS in any form.
- Per-mount (as opposed to node-wide) client-counter attribution beyond what `nfsstat` natively exposes.
- Per-user NFS I/O attribution (Server Spec §4.7 stretch).

## 10. Open decisions carried over from this spec

- Exact export ACL/mapping policy for the demo network (placeholder `-mapall` above; confirm actual demo-network CIDR and identity to map to).
- Whether the NFS server node's own daemon should also mount and monitor its own export as a client (loopback), for symmetry with other client nodes — recommended for demo completeness but adds a mount that only exists for test purposes.
- Final choice of bounded-probe path/file inside the export, and how it gets seeded so the probe never hits an empty/missing target.
