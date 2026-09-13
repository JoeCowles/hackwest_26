# Configured NFS user quotas

Orchard reads user quota observations from explicitly configured NFS servers with
classic ONC RPC rquota v1 GETQUOTA over UDP. Collection belongs to ciderd. The
central server provides authenticated views of these observations and never
queries remote filesystems. No export discovery, user enumeration or quota
mutation is performed by the collector.

## Configuration and server requirements

```toml
[nfs_quotas]
interval_seconds = 15
timeout_seconds = 3

[[nfs_quotas.targets]]
server = "nfs.internal.example"
export_path = "/srv/inference"
uid = 501
display_label = "Inference service"
port = 875
```

`export_path` is the server-local absolute path understood by rquotad. An NFSv4
pseudoroot mount path can differ; configure the actual server path. The configured
server spelling and export path form the subject identity together with UID.
Optional `port` pins a UDP endpoint; otherwise ciderd performs one bounded
portmapper GETPORT lookup for program 100011, version 1, UDP on that same server.
At most 32 distinct server/export/UID subjects are allowed. UIDs are canonical
nonnegative signed-32-bit values, paths at most 1024 bytes, interval 15–3600
seconds and request deadline 1–10 seconds. The default is no subjects.

Enable filesystem user quotas on the NFS server and run its rquotad service.
Use a deployment firewall to permit the configured client to reach the chosen
UDP rquotad port (and portmapper when used). This is trusted-network ONC RPC:
AUTH_SYS carries identity claims and the server is not cryptographically
authenticated. Use only the explicitly configured trusted service. Client
credentials are the process's **real** UID/GID and up to 16 real supplementary
groups, with truncation reported. Querying a different UID does not impersonate
that user; the server decides whether to allow the read. Server rules may deny
other users to an ordinary ciderd process.

Each subject has an isolated subprocess in ciderd's existing remote-worker pool.
The outer deadline adds one second to the network budget to bound DNS resolution
and process overhead. Per-resource admission remains one worker at a time, output
is bounded, and timed-out workers retain the existing pending-exit supervision.
Heartbeats stay at five seconds. No local NFS mount is required to collect an
explicitly configured quota. Removing configuration removes that quota resource
on the next ciderd startup; configuration is not hot-reloaded.

## Observation semantics

Resources have `resource_type=nfs_user_quota`. Metrics use `storage.nfs.quota.*`
with collector `nfs.rquota`: `status`, `active`, `block_size_bytes`, `used_bytes`,
`block_soft_limit_bytes`, `block_hard_limit_bytes`, `used_inodes`,
`inode_soft_limit`, `inode_hard_limit`, `block_grace_seconds_raw`, and
`inode_grace_seconds_raw`.

Block counts are multiplied by the positive signed-32-bit block size using
checked exact arithmetic. JSON integers stay decimal strings. This preserves the
wire observation exactly; classic rquota's 32-bit counts can constrain what the
server can represent. These are filesystem user quotas, potentially spanning
several exports. Multiple UIDs also do not partition total filesystem capacity.
Never sum quota rows into local, shared or cluster capacity.

`available` means an actual Q_OK response. `active` reports whether enforcement
is active. A zero limit in that response explicitly means unlimited; an inactive
quota record still displays its reported limits and inactive enforcement. Q_NOQUOTA
is `no_quota`, never zero or unlimited. `permission_denied`, `timeout`,
`unsupported`, `unavailable`, and `parse_error` are distinct results. Old values
retain their original source dates after failed attempts and never supply current
limit conclusions. No configured subjects and unknown collector configuration
are separate empty-state cases.

Grace wire fields are unsigned 32-bit values. Their signed-32-bit interpretation
is relative seconds at acquisition; zero means no active deadline. Negative
values represent an already expired deadline. The view retains raw and signed
values, reports the acquisition date, and subtracts observation age for the
remaining-time estimate. Clock-based deadline timestamps are source-clock
estimates. Grace is not a forecast of drive failure or storage availability.

## Read API

`GET /api/v1/quotas` uses the existing viewer/admin bearer authentication, read
budgets and frozen pagination. Optional filters are `node_id`, `uid`, `limit`,
and `cursor`. UID must be canonical and in 0..2147483647; invalid filters return
400, missing/invalid credentials 401, and normal reads 200. Read-budget and
snapshot-capacity failures preserve the existing API error contracts.

The ordinary `{data,meta}` envelope contains rows with `quota_id`, `node_id`,
`node_name`, `server`, `export_path`, `uid`, `display_label`, `nfs_source`,
`protocol`, `scope`, `state`, `reason`, `observed_at`, `age_seconds`,
`last_attempt`, `active`, `query_identity`, `metrics`, `limits`, `grace`,
`linked_mount_ids`, `linkage_state`, and `uncertainty`. `metrics` retains normalized
values and acquisition metadata. `limits` has `block_soft`, `block_hard`,
`inode_soft`, `inode_hard`, each with unknown/limited/unlimited state, exact value
and unit. Current limit conclusions require every field from the same available
collection. Missing, mixed, stale or failed observations remain uncertain.

Mount linkage requires an active NFS mount on the same node whose source string
exactly equals the configured `server:export_path`. A different alias or NFSv4
pseudoroot does not establish this link. `configured_source_only` means the
configuration names the source but there is no exact local mount observation.
User quotas are displayed separately from APFS volume quotas.

## Reproducible isolated fixture

The fixture in `scripts/nfs-quota-fixture` creates a private 128 MiB ext4 loop
filesystem, enables user quota accounting, sets explicit test limits, creates
small owned files, and starts NFS/rquotad. Privilege is required **inside the
isolated Linux VM/container** for loop mounts and nfsd. It does not bind-mount host
data or change host exports. The dedicated Podman VM uses 1 CPU and 1 GiB RAM;
its sparse disk must be at least the base-image size (the tested image needs 10 GiB; a dedicated 12 GiB disk was used). Docker Desktop's kernel on
the validation host lacks CONFIG_QFMT_V2, so its ext4 quota mount is unsupported.
The [runner and cleanup instructions](../scripts/nfs-quota-fixture/README.md) are provided alongside the fixture scripts.

Validation completed on 2026-09-13 with the isolated Podman fixture. The runner's
`up`, `verify`, and `down` paths were executed. No host export or host NFS mount
was changed. The quota collector reported exactly the server accounting below:

| Test UID | Used bytes | Block soft / hard bytes | Used inodes | Inode soft / hard |
| --- | ---: | ---: | ---: | ---: |
| 501 | 66,560 | 1,048,576 / 2,097,152 | 2 | 10 / 20 |
| 502 | 132,096 | 4,194,304 / 8,388,608 | 2 | 30 / 40 |
| 503 | 1,024 | 0 / 0 (explicit unlimited) | 1 | 0 / 0 |

Each Linux query ran as the actual corresponding test UID. Independent `repquota`
agreed exactly, including allocated directory blocks. UID 501 querying UID 502
received Q_EPERM; a nonexistent export returned Q_NOQUOTA. The production macOS
ciderd worker, running as real UID 501/GID 20, matched UID 501 accounting and
returned permission-denied/no-quota for those same cases. A bounded nonresponding
loopback endpoint returned timeout; a closed UDP port returned unavailable.

A real NFSv3 client mount inside the disposable container linked
`127.0.0.1:/exports/quota` to the tested filesystem. Reads by UIDs 501 and 502
produced the expected SHA256 digests for the 64 KiB and 128 KiB payloads.
This verifies fixture NFS linkage; no macOS host NFS mount was created.

Static/protocol coverage includes malformed/truncated replies, exact wide integer
conversion, explicit zero versus no-quota, RPC/portmapper rejection, deadline
behavior, source-context and observation-age boundaries, and real recorded RPC
reply decoding. The final ciderd suite passed 118 tests, with three existing
subprocess-helper cases intentionally ignored. Server quota view/API/replay tests
passed 11 tests combined. The static contract validator passed 85 metric
definitions and 21 rejected-input cases.

A separate replay test enrolled synthetic nodes, submitted the actual recorded
quota values through authenticated native heartbeat ingestion, repeated the
heartbeats idempotently, and read `/api/v1/quotas`. It verified exact values,
explicit unlimited/unknown limits, and exclusion from filesystem/capacity views.
Its browser fixture uses synthetic node IDs and replay acquisition dates; it is
not a live production-cluster capture. Shared documentation and full browser
integration verification are tracked by the coordinating task.

Coverage reports unknown if any selected node's current quota configuration is
missing, stale or from another boot/session. A UID filter yielding no rows is an
empty result, never evidence of complete available coverage.

## Protocol references

- [Linux quota-tools rquota protocol](https://kernel.googlesource.com/pub/scm/utils/quota/quota-tools/+/e73c5b48e12c3f02e532864a1107cdc8a4feafc3/rquota.x)
- [Linux quota-tools rquota client conversion](https://kernel.googlesource.com/pub/scm/utils/quota/quota-tools/+/refs/tags/v4.07/rquota_client.c)
- [Apple NFS sources](https://github.com/apple-oss-distributions/NFS), and the
  installed macOS SDK `usr/include/rpcsvc/rquota.x`.

The implementation is original Rust code implementing the protocol, not copied
quota-tools implementation code.
