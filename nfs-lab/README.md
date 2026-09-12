# orchard-nfs

Node-side NFS provisioning and telemetry collection for Orchard, as a single
Rust binary. Stands up a generic NFS export on macOS, mounts it on clients, and
runs a monitor daemon that samples every NFS mount and the kernel's per-operation
RPC counters.

This crate is a **standalone workspace**, deliberately not a member of the root
`Cargo.toml`. It builds independently and adds nothing to the central server, per
`AGENTS.md`: no node collection code lives in `server/`.

## Requirements

- macOS on server and client
- Rust 1.88+ (`rustup`; the repo's `scripts/cargo-local.sh` convention also works)
- Admin rights — every subcommand that touches system state requires `sudo`
- A directory to export, on an APFS volume

## Build

```bash
cd nfs-lab
cargo build --release
# binary at target/release/orchard-nfs
```

## Quick start

```bash
cp config.example.toml config.toml
$EDITOR config.toml
```

On the machine with the drive attached:

```bash
sudo target/release/orchard-nfs setup-server
```

On each client:

```bash
sudo target/release/orchard-nfs mount
sudo target/release/orchard-nfs install-daemon
```

See what the monitor sees without installing it:

```bash
target/release/orchard-nfs monitor --once | jq .
```

`--config <path>` (or `ORCHARD_NFS_CONFIG`) selects the config; it defaults to
`./config.toml`.

## Subcommands

| Command | Root | Does |
| --- | --- | --- |
| `setup-server` | yes | Write the export to `/etc/exports`, enable and start `nfsd` |
| `teardown-server [--stop-nfsd]` | yes | Remove the export; optionally stop `nfsd` if nothing else is exported |
| `mount` | yes | Mount the export and report the *negotiated* options |
| `unmount` | yes | Unmount it |
| `monitor [--once]` | no | Sample all NFS mounts and counters, emit newline-delimited JSON |
| `install-daemon` | yes | Register `monitor` as a launchd LaunchDaemon |
| `uninstall-daemon` | yes | Remove it |
| `daemon-status` | no | Report whether the LaunchDaemon is loaded |

## Layout

| Path | Role |
| --- | --- |
| `src/main.rs` | CLI and dispatch |
| `src/config.rs` | TOML config, validation, export-line and mount-option rendering |
| `src/exports.rs` | `/etc/exports` management with backup, validation, rollback |
| `src/mount.rs` | Client mount and `mount -t nfs` parsing |
| `src/monitor.rs` | Sampler: time-bounded probes, `nfsstat` JSON embedding, snapshot count |
| `src/daemon.rs` | launchd plist rendering and lifecycle |
| `src/sys.rs` | Root check, bounded subprocess execution |

## How `/etc/exports` is handled

`setup-server` only ever rewrites its own marked block:

```
# >>> orchard-nfs managed block >>>
...
# <<< orchard-nfs managed block <<<
```

Hand-written exports outside the block are preserved. The file is backed up to
`/etc/exports.orchard-backup.<timestamp>` before every change, the result is
validated with `nfsd checkexports`, and the backup is restored automatically if
validation fails. `teardown-server` removes only the block and refuses
`--stop-nfsd` while other exports remain.

The config will not load without an explicit client restriction
(`allowed_network` + `allowed_mask`, or `allowed_hosts`). There is deliberately
no default that exports to everyone.

## NFS version

`nfs_vers` defaults to **"3"**. macOS's NFS *client* handles v4.1 well, but Apple's
`nfsd` *server* is far less exercised on v4 and is the more common source of a mount
that silently fails or downgrades. Version 3 is the dependable default for a
macOS-to-macOS export.

Set `nfs_vers = "4"` to try v4. Confirm what you actually got rather than what you
asked for — `mount` prints the negotiated mount line, and macOS falls back without
saying so.

`resvport` is in the default `mount_opts` because a macOS-served export requires
clients to use a reserved port. Removing it produces a permission failure that
looks like an access-control problem and is not.

## Monitor output

One JSON object per sample, printed to stdout and appended to `metrics_log`:

```json
{
  "ts": "2026-09-12T21:14:03Z",
  "host": "node-01",
  "mounts": [
    {
      "source": "192.168.1.10:/Volumes/easystore/nfs-export",
      "mount_point": "/Users/Shared/nfs/cluster",
      "options": "nfs, nodev, nosuid",
      "state": "ok",
      "capacity_kb": 1953514584,
      "used_kb": 421337152,
      "available_kb": 1532177432
    }
  ],
  "nfs_client": { "Client Info": { "NFSv3 RPC Counts": { "Read": 18134, "...": "..." } } },
  "nfs_server": { "Server Info": { "RPC Counts": { "Read": 0, "Write": 0, "Remove": 0, "Rename": 0, "...": "..." } } },
  "apfs_snapshots": 3
}
```

`nfs_client` and `nfs_server` are `nfsstat -f JSON` embedded verbatim. All 21
per-operation server counters are kept rather than summarized — the ratio between
Read, Write, Remove and Rename is the signal that separates bulk copy from mass
deletion from encryption in place (see `docs/threat-model.md` §2.2), and collapsing
it here would throw that away before anything downstream can use it.

Set `metrics_endpoint` to also POST each sample to an HTTP collector. Unset, the
daemon stays entirely local.

Two properties worth preserving if this is extended:

- **Every probe is time-bounded** (`probe_timeout_secs`, default 5). A dead NFS
  mount blocks `df` forever, so an unbounded probe wedges the daemon exactly when it
  finally has something worth reporting. A timed-out mount reports
  `"state": "not_responding"`.
- **Unreadable is not zero.** A mount that cannot be read reports `null` capacity,
  never `0`. Whatever consumes this must be able to tell "zero free space" from
  "could not read free space".

## Tests

```bash
cargo test
cargo clippy --all-targets
```

Unit tests cover the pure logic: export-line rendering, config validation,
managed-block rewriting and idempotence, mount-line and `df` parsing, snapshot
counting, plist rendering. Anything that needs root or a live `nfsd` is exercised
by running the binary, not by the test suite.

## Teardown

```bash
sudo target/release/orchard-nfs uninstall-daemon
sudo target/release/orchard-nfs unmount           # on each client
sudo target/release/orchard-nfs teardown-server   # on the server
```

## Troubleshooting

| Symptom | Check |
| --- | --- |
| `showmount -e` lists nothing | `sudo nfsd checkexports`, then `sudo nfsd status` |
| Mount hangs | Server reachable on `2049/tcp`? `nc -vz <server> 2049` |
| `Operation not permitted` on mount | `resvport` missing from `mount_opts` |
| `Permission denied` on files | `map_mode` / `map_identity` vs. the export's on-disk ownership |
| Daemon not running | `orchard-nfs daemon-status`, then `/usr/local/var/log/orchard-nfs/stderr.log` |
