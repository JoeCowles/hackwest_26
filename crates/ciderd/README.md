# ciderd

A reusable Rust crate and a single macOS executable implementing the supplied
[storage telemetry guide](contract/guide.org). It sends schema 2.0 latest-state
heartbeats to an authenticated HTTPS endpoint. It does not provide a server.

## Orchard workspace integration

The workspace includes both `orchard-server` and `ciderd`. Orchard accepts this
crate's schema-2 heartbeat at `POST /api/v2/ciderd/heartbeat` and projects accepted
observations into its existing authenticated web-console read API. The server
shares this crate's pure wire validator, not its collectors or worker runtime.
The example sends heartbeats every **5 seconds** and requires trusted HTTPS,
including loopback. Configure `ca_certificate_file` for a private CA.

For a new node, obtain a single-use enrollment token from the server's native
window and place it in an owner-only token file. After configuring the example's
absolute paths, enroll once:

```sh
ciderd enroll-server --config /etc/ciderd.toml --name studio-mac --enrollment-token-file /secure/enrollment-token
```

This prepares a private state directory before exchanging the token through the
existing v1 enrollment route, stores the node credential as a mode-0600 file,
and creates a matching local identity. It
refuses to overwrite either file and never prints the credential. Do not use
local-only `enroll` first unless provisioning the matching server identity by
hand. A lost response or partial local write needs explicit administrator
recovery; enrollment is never automatically retried. Protect and remove the
one-use token file after successful enrollment. Credential-file storage is the
current collector implementation, not Keychain integration.

Run the daemon separately from the server. Inventory is an upsert graph; only
explicit tombstones remove resources. Repeated immutable collection IDs do not
create new samples. The server acknowledges only committed receipts and requests
inventory when its cached revision is missing. Do not mix v1 inventory/telemetry
writes with schema-2 heartbeats on one enrolled identity.

The compatibility integration passed workspace builds/checks, all 100 tests,
and the real collector-to-packaged-server TLS smoke test. The latter exercises
enrollment, private state/credential creation, five-second heartbeats, and
read-API measurements. Run `node scripts/smoke-ciderd.mjs` from the workspace
after building both packages and the debug server bundle. The canonical
receiver contract and validation boundaries are in section 17 of the Google
Doc's Server Spec tab; no exhaustive hardware or visual validation is implied.

## Library and runtime

The Cargo package `ciderd` exposes library `ciderd`. Public modules
include configuration and wire types, source parsers, native workers, state,
identity, command supervision, heartbeat transport, and runtime entry points.
`runtime::run_with_worker` and `snapshot_with_worker` let an embedding application
provide the deployed `ciderd` path for worker subprocesses.

One state owner merges results and publishes `Arc<Heartbeat>` snapshots through
a watch channel. The sender clones the Arc before copying/serializing; it never
holds a watch borrow across network or disk work. Collection, persistence,
credential reload, and HTTP have separate tasks and bounded work admission.

Each transmission gets a new sequence. Repeated measurements retain their
collection IDs, original times, values, and epochs. Partial/failed results retain
older available fields. Integer counters are decimal strings through u128;
unavailable fields never become zero. The cache is capped at 4096 collection
records and 16 MiB, with a 256 KiB per-collection limit. Metadata is capped at
8 MiB. Oversized outbound payloads become explicit coverage-limited summaries.

## Collected sources

| Source | Implementation and scope |
| --- | --- |
| Physical disk inventory | `diskutil list -plist physical`, structural plist decoding |
| APFS containers/volumes | `diskutil apfs list -plist`; separate container/volume accounting |
| APFS snapshots | Per-volume `diskutil apfs listSnapshots -plist`; count and inventory |
| Local block I/O | Native IOKit driver records, registry IDs, exact cumulative counters |
| USB connection evidence | Bounded nearest-device ancestry, opaque enclosure/incarnation identity, reported negotiated bitrate, explicit enumeration completeness |
| Cached mounts | Caller-owned `getfsstat(MNT_NOWAIT)` records without pathname queries |
| Fresh capacity | Isolated `statfs` worker, verifies expected fsid before accepting data |
| NFS client | Versioned `nfsstat -f JSON -c` tables, host-client scope |
| NFS mount status | Isolated SDK-backed `VFS_CTL_NSTATUS`, bounded size/count validation |
| SMART, optional | Configured `smartctl -x --json=v -n standby,2`; exact NVMe values and strictly admitted ATA 5/197/198 sector gauges |

Cached capacity accompanies the `mount.inventory` collection that acquired it;
fresh capacity uses `filesystem.capacity`. This provenance distinction prevents
cached enumeration from overwriting a fresh worker's running/timeout state.
Both retain the supplied catalog's filesystem metric names and units.

The host `iokit.block` collection also carries a version-1
`extensions.usb_device_snapshot`, including complete empty enumerations. It uses
the same five-second acquisition cadence and original collection timestamps as
the native worker. `src/device_snapshot.rs` defines its bounded pure types; the
server shares those types without linking collection code. USB failures preserve
usable I/O counters and cannot certify absence. Raw USB serial properties stay
inside the private acquisition/parser boundary; the heartbeat contains an opaque
node-scoped identity. The identity follows a reported enclosure, not verified
installed media. Missing serials use a boot/registry identity with no comparison
across reconnects. Negotiated bitrates describe the USB transport, not observed
read/write throughput or media health. See [the USB demo runbook](../../docs/usb-demo-alerts.md).

The native boundary is in `src/platform/`; its small C shim compiles against the
selected Apple SDK rather than duplicating Apple struct layouts. Other hosts can
build/test pure types and parsers; live acquisition requires macOS.

Collection intervals are independent, with configurable jitter and skipped missed ticks.
The example schedules inventory and I/O every five seconds and all other scans
every 15 seconds, with jitter disabled. Optional NFS quotas also default to
15 seconds. These are scheduling intervals; slow jobs and worker contention can
delay completed observations.
Admission prioritizes scopes waiting longest. Command, native, and path/NFS
worker budgets are separate; one slow mount cannot consume native IOKit slots.
Each scope remains admitted through parsing/publication as well as child exit.
Restored dynamic resources must be rediscovered in the current session before
fresh path/SMART/snapshot work starts.

## Enrollment and deployment

Copy [the example config](examples/ciderd.toml), choose local identity,
state and credential paths, and replace the illustrative endpoint. The endpoint
must implement the supplied [request](contract/heartbeat.schema.json) and
[acknowledgement](contract/heartbeat-ack.schema.json) contracts. Enrollment
creates a local node ID; provision a per-node bearer credential bound to that ID
on the server separately.

```sh
ciderd validate-config --config /absolute/path/config.toml
ciderd enroll --config /absolute/path/config.toml
ciderd run --config /absolute/path/config.toml
```

The credential file must belong to the daemon user and have mode 0600. HTTPS
certificate validation is mandatory; redirects, inherited HTTP proxies and
hidden retries are disabled. A private deployment CA can be configured with
`heartbeat.ca_certificate_file`; it extends system trust. Credentials reload on
a separate 30-second schedule. Failed reloads retain the last validated token
and publish a bounded event. Credential values and raw command output are never
included in operational events.

Production `run` requires root, root-controlled local executable/config/state
paths and non-writable parent directories. Optional SMART paths must meet the
same deployment policy; a normal user-writable Homebrew symlink is not suitable
for a privileged daemon. `--allow-unprivileged` supports development, with
permissions/capabilities reflected in collector results. Keep every daemon
backing path on local storage.

The [LaunchDaemon example](examples/org.example.ciderd.plist) uses
`/usr/local/libexec/ciderd` and foreground `run`. Customize its paths and
label, install a root-owned binary/config/credential/state directory, arrange
stdout/stderr rotation, validate with `plutil -lint`, then register it using your
deployment process. This repository does not automatically install, register,
sign, or start a LaunchDaemon.

Identity enrollment never overwrites an existing identity. A single-instance
lock protects a durably incremented generation before the first send. Invalid
identity state or missing inventory after a previous invocation requires explicit
recovery or enrollment with a new node ID. Do not copy enrolled identities when
cloning machines.

Commands use absolute executables, fixed argument arrays, a minimal environment,
and separately capped stdout/stderr. Timeout/cap results publish before a reaper
finishes; process slots remain reserved until exit and journal cleanup. Active-job
intent is stored before spawn. On restart, unknown launch intents and same-boot
live PIDs reserve slots without signalling recorded processes. PID reuse can
therefore conservatively reduce coverage until the process exits or the host
reboots. Never delete live job journals merely to regain worker slots.

SIGTERM stops admission, attempts one final bounded heartbeat, and performs a
bounded cleanup. Jobs that survive remain recorded for restart accounting.

## Verification and limits

```sh
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Tests cover schemas/semantics, precision, missing fields, partial readings,
acknowledgements, identity locking, cache bounds, journal recovery, timeouts,
output floods, cancellation, TLS transport and native worker smoke tests.
The source bundle's synthetic examples and schemas are retained under
`contract/`. `validate_contract.py` remains the supplied developer fixture
checker; it is not part of the shipped runtime.

Physical diskutil identities are explicitly scoped to a boot and BSD locator.
An undetected replacement reusing a locator between reconciliations cannot be
resolved reliably from that output. IOKit counter epochs use registry identity.
Every SMART reading carries a private fixed-size continuity marker; a reported
WWN takes precedence over reported serial/model/protocol identity, while missing
reported identity is explicitly `caller_epoch_only` and weak. The scheduled
adapter validates a reported SMART device locator against its admitted physical
resource. Disk Arbitration event-driven lifecycle enrichment remains future work.

Foundation capacity estimates, APFS purgeability estimates, NFSv4.1 supplementary
tables, NFS server/provider metrics, active probes and diagnostics are not enabled.
No trace, repair, unmount, self-test or write probe is started by polling. The
configured diagnostic/write-probe switches are rejected if enabled. Native
NSTATUS has been compiled and tested for unavailable/gone cases; successful
NSTATUS against a real NFS mount, Intel macOS, signed packaging and installed
launchd operation still require target-environment verification.

Configured NFS user quotas are collected using read-only rquota v1 GETQUOTA over
UDP. Add `[nfs_quotas]` and explicit `[[nfs_quotas.targets]]` entries; the example
configuration defaults to no targets. Quotas use the real process AUTH_SYS UID,
GID and up to 16 real supplementary groups. The queried UID is never used as the
caller identity. Each target runs in the existing bounded remote subprocess pool;
slow DNS, portmapper or rquotad cannot block five-second heartbeats. Missing quota,
permission denial, timeout and unsupported service remain explicit observations.
See [NFS quota setup and verification](../../docs/nfs-user-quotas.md). User quotas
apply to server filesystems and can overlap across exports; do not sum them or
confuse them with APFS volume quota settings.
