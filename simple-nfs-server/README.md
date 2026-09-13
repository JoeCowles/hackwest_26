# simple-nfs-server

Basic macOS NFS export and client mount management using the system `nfsd`.
There is no telemetry collection, metrics upload, or monitoring daemon.

This crate is a standalone workspace. It builds independently; the root workspace
and existing Orchard server functionality are unchanged.

## Requirements

- macOS on server and client
- Rust 1.88+ (`rustup`; the repo's `scripts/cargo-local.sh` convention also works)
- Admin rights — every subcommand that touches system state requires `sudo`
- A directory to export, on an APFS volume

## Build

```bash
cd simple-nfs-server
cargo build --release
# binary at target/release/simple-nfs-server
```

## Quick start

```bash
cp config.example.toml config.toml
$EDITOR config.toml
```

On the machine with the drive attached:

```bash
sudo target/release/simple-nfs-server setup-server
```

On each client:

```bash
sudo target/release/simple-nfs-server mount
```

`--config <path>` (or `SIMPLE_NFS_SERVER_CONFIG`) selects the config; it defaults to
`./config.toml`.

## Subcommands

| Command | Root | Does |
| --- | --- | --- |
| `setup-server` | yes | Write the export to `/etc/exports`, enable and start `nfsd` |
| `teardown-server [--stop-nfsd]` | yes | Remove the export; optionally stop `nfsd` if nothing else is exported |
| `mount` | yes | Mount the configured export, verify its source, and report observed mount flags |
| `unmount` | yes | Unmount it |

## Layout

| Path | Role |
| --- | --- |
| `src/main.rs` | CLI and dispatch |
| `src/config.rs` | TOML config, validation, export-line and mount-option rendering |
| `src/exports.rs` | `/etc/exports` management with backup, validation, rollback |
| `src/mount.rs` | Client mount and `mount -t nfs` parsing |
| `src/sys.rs` | Platform/root checks and subprocess execution |

## How `/etc/exports` is handled

`setup-server` only ever rewrites its own marked block:

```
# >>> orchard-nfs managed block >>>
...
# <<< orchard-nfs managed block <<<
```

The block markers retain their original name so existing exports can still be
updated or removed. Hand-written exports outside the block are preserved. The file is backed up to
`/etc/exports.orchard-backup.<timestamp>` before every change, the result is
validated with `nfsd checkexports`, and the original content (or original file
absence) is restored if validation fails or the validator cannot run. Malformed,
unclosed, or duplicate managed blocks stop the operation before rewriting the
file. Unmanaged text retains its original bytes. `teardown-server` removes only the block and refuses
`--stop-nfsd` while other exports remain.

The config will not load without an explicit client restriction
(`allowed_network` + `allowed_mask`, or `allowed_hosts`). There is deliberately
no default that exports to everyone. Empty host restrictions, malformed subnet
masks, relative paths, path traversal components, and multiple options hidden
inside a single option string are rejected. Export path spaces and quotes are
escaped for the native exports-file format. Numeric IPv4 and IPv6 subnet masks
are supported. Repeated separators and trailing slashes are normalized when the
config loads, without accessing remote paths. Configure client restrictions and
identity mapping through their dedicated fields. Extra options must be single
tokens using letters, digits, `-`, `_`, `.`, `=`, or `:`.

## Mount options

The client defaults to `vers=3,resvport,rw,hard,intr`. Adjust `nfs_vers` and
`mount_opts` in the configuration as needed. Both mount and unmount verify the
configured source before using an occupied mountpoint. Source matching is exact;
use the same hostname or address as the existing mount. The mount command checks
that the requested export is present after the command succeeds, then prints
the flags reported by `mount`. These flags do not establish the negotiated NFS
version or transport.

## Tests

```bash
cargo test
cargo clippy --all-targets
```

Unit tests cover the pure logic: export-line rendering, config validation,
managed-block validation, byte preservation, rollback on validator failure,
idempotence, mount-line parsing, and source matching. Rollback tests use temporary
files; no test changes `/etc/exports`, runs `nfsd`, or mounts a filesystem. Operations that
need root or a live `nfsd` require manual integration verification.

## Teardown

```bash
sudo target/release/simple-nfs-server unmount           # on each client
sudo target/release/simple-nfs-server teardown-server   # on the server
```

## Troubleshooting

| Symptom | Check |
| --- | --- |
| `showmount -e` lists nothing | `sudo nfsd checkexports`, then `sudo nfsd status` |
| Mount hangs | Server reachable on `2049/tcp`? `nc -vz <server> 2049` |
| `Operation not permitted` on mount | `resvport` missing from `mount_opts` |
| `Permission denied` on files | `map_mode` / `map_identity` vs. the export's on-disk ownership |
