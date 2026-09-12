# ciderd

A reusable Rust crate and a single macOS executable implementing the supplied
[storage telemetry guide](contract/guide.org). It sends schema 2.0 latest-state
heartbeats to an authenticated HTTPS endpoint. It does not provide a server.

```sh
cargo build --release --bin ciderd --locked
cargo run --bin ciderd -- snapshot --seconds 3
cargo run --bin ciderd -- validate-config --config crates/ciderd/examples/ciderd.toml
```

`snapshot` collects offline with a temporary node/session identity, prints JSON,
and stops. It reads no bearer credential and sends no HTTP. Preview admission
journals live in a private `ciderd-preview-<uid>` directory under the
system temporary directory. A preview that cannot reap a worker reports the
remaining count; future previews conservatively reserve those slots.

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
| Cached mounts | Caller-owned `getfsstat(MNT_NOWAIT)` records without pathname queries |
| Fresh capacity | Isolated `statfs` worker, verifies expected fsid before accepting data |
| NFS client | Versioned `nfsstat -f JSON -c` tables, host-client scope |
| NFS mount status | Isolated SDK-backed `VFS_CTL_NSTATUS`, bounded size/count validation |
| SMART, optional | Configured `smartctl -x --json=v -n standby,2`; exit bitmask and exact integers |

Cached capacity accompanies the `mount.inventory` collection that acquired it;
fresh capacity uses `filesystem.capacity`. This provenance distinction prevents
cached enumeration from overwriting a fresh worker's running/timeout state.
Both retain the supplied catalog's filesystem metric names and units.

The native boundary is in `src/platform/`; its small C shim compiles against the
selected Apple SDK rather than duplicating Apple struct layouts. Other hosts can
build/test pure types and parsers; live acquisition requires macOS.

Collection intervals are independent, with jitter and skipped missed ticks.
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
resolved reliably from that output. IOKit counter epochs use registry identity;
SMART uses reported identity when available. Strong persistent physical identity
and Disk Arbitration event-driven lifecycle enrichment remain future work.

Foundation capacity estimates, APFS purgeability estimates, NFSv4.1 supplementary
tables, NFS server/provider metrics, active probes and diagnostics are not enabled.
No trace, repair, unmount, self-test or write probe is started by polling. The
configured diagnostic/write-probe switches are rejected if enabled. Native
NSTATUS has been compiled and tested for unavailable/gone cases; successful
NSTATUS against a real NFS mount, Intel macOS, signed packaging and installed
launchd operation still require target-environment verification.
