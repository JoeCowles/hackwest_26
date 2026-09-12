# nfs-lab

Standalone scripts to stand up a generic NFS export on macOS and run a small
monitoring daemon against it. Self-contained: nothing here imports from or is
imported by the rest of the repo, so it can be merged independently.

## Requirements

- macOS on both server and client
- Admin rights (every script that touches system state requires `sudo`)
- A directory to export, on an APFS volume

## Quick start

```bash
cd nfs-lab
cp config.env.example config.env
$EDITOR config.env
chmod +x bin/*.sh daemon/*.sh
```

On the machine with the drive attached:

```bash
sudo ./bin/server-setup.sh
```

On each client:

```bash
sudo ./bin/client-mount.sh
sudo ./daemon/daemon-ctl.sh install
```

Check what the daemon sees without installing it:

```bash
./daemon/nfs-monitor.sh --once
```

## Files

| Path | Role |
| --- | --- |
| `config.env.example` | Every tunable. Copy to `config.env`, which is git-ignored. |
| `lib/common.sh` | Shared helpers, sourced by the rest. |
| `bin/server-setup.sh` | Writes the export to `/etc/exports`, enables and starts `nfsd`. |
| `bin/server-teardown.sh` | Removes the export. `--stop-nfsd` also stops the service. |
| `bin/client-mount.sh` | Mounts the export and reports the *negotiated* options. |
| `daemon/nfs-monitor.sh` | Samples all NFS mounts, emits newline-delimited JSON. |
| `daemon/daemon-ctl.sh` | `install` / `uninstall` / `status` for the launchd job. |

## How `/etc/exports` is handled

`server-setup.sh` only ever rewrites its own marked block:

```
# >>> nfs-lab managed block >>>
...
# <<< nfs-lab managed block <<<
```

Hand-written exports outside that block are preserved. The file is backed up to
`/etc/exports.nfslab-backup.<timestamp>` before every change, the result is
validated with `nfsd checkexports`, and the backup is restored automatically if
the new configuration does not parse. `server-teardown.sh` removes only the
block, and refuses `--stop-nfsd` while other exports remain.

The script will not run without an explicit client restriction
(`ALLOWED_NETWORK` + `ALLOWED_MASK`, or `ALLOWED_HOSTS`). There is deliberately
no default that exports to everyone.

## NFS version

`NFS_VERS` defaults to **3**. macOS's NFS *client* handles v4.1 well, but Apple's
`nfsd` *server* is far less exercised on v4 and is the more common source of a
mount that silently fails or downgrades. Version 3 is the dependable default for
a macOS-to-macOS export.

Set `NFS_VERS=4` in `config.env` to try v4. Confirm what you actually got
rather than what you asked for - `client-mount.sh` prints the negotiated mount
line, and macOS will fall back without saying so.

`resvport` is in the default `MOUNT_OPTS` because a macOS-served export requires
clients to use a reserved port. Removing it will produce a permission failure
that looks like an access-control problem but is not.

## Daemon output

One JSON object per sample, appended to `METRICS_LOG` and printed to stdout:

```json
{
  "ts": "2026-09-12T21:14:03Z",
  "host": "node-01",
  "mounts": [
    {
      "source": "192.168.1.10:/Volumes/ClusterWD/nfs-export",
      "mount_point": "/Users/Shared/nfs/cluster",
      "options": "nfs, nodev, nosuid",
      "state": "ok",
      "capacity_kb": 1953514584,
      "used_kb": 421337152,
      "available_kb": 1532177432
    }
  ],
  "client_rpc": { "state": "ok", "requests": 18134, "timed_out": 0, "retries": 2 }
}
```

Set `METRICS_ENDPOINT` to also POST each sample to an HTTP collector. Left empty,
the daemon stays entirely local.

Two properties worth preserving if this gets extended:

- **Every probe is time-bounded** (`PROBE_TIMEOUT`, default 5s). A dead NFS mount
  blocks `df` and `stat` forever, so an unbounded probe wedges the daemon exactly
  when it finally has something worth reporting.
- **Unreadable is not zero.** A mount that times out reports
  `"state": "not_responding"` with `null` capacity, never `0`. A daemon that
  reports zero free space and a daemon that cannot read free space should not
  look the same to whatever consumes this.

## Teardown

```bash
sudo ./daemon/daemon-ctl.sh uninstall
sudo umount /Users/Shared/nfs/cluster     # on each client
sudo ./bin/server-teardown.sh             # on the server
```

## Troubleshooting

| Symptom | Check |
| --- | --- |
| `showmount -e` lists nothing | `sudo nfsd checkexports`, then `sudo nfsd status` |
| Mount hangs | Server reachable on `2049/tcp`? `nc -vz <server> 2049` |
| `Operation not permitted` on mount | `resvport` missing from `MOUNT_OPTS` |
| `Permission denied` on files | `MAP_MODE` / `MAP_IDENTITY` vs. the export's on-disk ownership |
| Daemon not running | `sudo ./daemon/daemon-ctl.sh status`, then `/usr/local/var/log/nfs-monitor/stderr.log` |
