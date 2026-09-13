# Requested functionality verification — 2026-09-12

This review distinguishes code that exists from behavior exercised against the
built application. It makes no production changes. The user's running test
session remains separate from the disposable verification server.

## Findings

| Requested behavior | Implementation | Verification and missing work |
| --- | --- | --- |
| 1. Several daemons on one head node; inspect each daemon's storage and metrics | Per-node enrollment, object ownership, inventory, summaries and raw metric views exist. Overview uses server aggregate capacity and sampled driver rates. | **Verified working for per-node views and aggregates; the complete per-physical-disk view is partial.** Two real daemon processes had separate inventories and metrics. There is no unified physical-disk dashboard or individual disk history. |
| 1. No daemons connected | Empty-cluster and offline-node states exist. Missing capacity/rates remain unknown, rather than becoming zero. | **Verified working for both an empty server and two stopped collectors.** Stopped hosts become degraded at 30 seconds and offline at 90 seconds after the last heartbeat; their rack entries remain visible. An unselected node is a different case: Overview remains the aggregate view regardless of host selection. |
| 2. Group several volumes beneath one disk without duplicating the disk | The backend has part of the relationship graph and separately deduplicates capacity. | **The requested grouped UI is not implemented.** Inventory and filesystem rows are flat; physical devices, driver observations, containers, volumes and mounts remain separate records. |
| 3. Add a joining node to topology | Enrolled nodes enter the host list through polling, and changed node membership rebuilds the rack. | **Verified working for enrolled-node membership.** The rack and host list changed from one to two without a reload. Enrollment/configuration is required; joining the LAN alone does not discover or enroll a daemon. The rack shows up to 24 enrolled hosts in an illustrative grid, not actual network links. |
| 3a. Identify MacBook versus Mac mini and use the appropriate model | A manually supplied model string containing “MacBook” selects a laptop mesh. | **Native detection and correct Mac mini rendering are not implemented.** Native ciderd sends no hardware model. All other model strings, including “Mac mini” and unknown models, select the iMac mesh. |

## Live verification

A separate headless server ran on loopback with a private temporary CA. Two
real `ciderd` processes enrolled independently, each with separate credentials,
state and node identity, and five-second heartbeats. Authenticated reads used
verified TLS. Browser checks used a temporary loopback proxy that also verified
the upstream TLS certificate. The user's existing server and collector were
not used as test fixtures or stopped.

The following checks completed:

- **Empty server:** authenticated `/nodes` returned an empty list; `/cluster`
  reported zero nodes and null/unknown capacity and rates. The browser showed
  an empty rack, enrollment guidance and unknown aggregates with 0/0 coverage.
- **Two real daemon instances:** each delivered at least two distinct accepted
  heartbeats and real native inventory/measurements. Node IDs were distinct;
  their object ID sets were disjoint and each inventory belonged to its node.
- **Aggregates:** reads sharing the same committed snapshot cursor showed that
  cluster read/write rates equalled the sum of available online contributions.
  This exact comparison captured partial 1/2 rate coverage; the browser later
  showed complete 2/2 coverage. Cluster local capacity equalled the two node
  capacities added together.
- **Visible joining:** Safari initially displayed one host and one rack model.
  After the second enrollment, the same page displayed two online hosts and
  two rack models without a browser reload. Partial I/O coverage was visible
  while the new node was still establishing rate samples, then became complete.
- **Individual views:** both host links opened distinct node URLs with their own
  summaries and inventory. Expanding a driver record exposed byte/operation/error
  counters under the correct node. A physical `disk0` record separately showed
  “No metrics reported for this object.” This verifies raw observation access
  and directly demonstrates the missing physical-disk/driver integration.
- **Grouping and model gaps:** the live view showed separate disk, partition,
  APFS container, volume, mount and driver entries; it did not nest volumes
  under disks. Both native nodes reported “Model not reported” and rendered
  using the iMac-style fallback.

- **No online collectors:** after stopping both daemons, the browser showed two
  degraded hosts and then two offline hosts as heartbeat timeouts elapsed.
  Authenticated reads and the browser agreed on zero online/two offline, no
  capacity contributors, both nodes excluded, and unknown aggregate capacity
  and rates. Both enrolled nodes stayed in the rack and host table. The API
  retained their prior capacity values marked stale; current UI summaries did
  not present them as live numbers.

**Scope limit:** these were two daemon processes on **one physical Mac**, not
two independent computers. Separately enrolling two full collectors on the
same Mac counts its physical capacity twice in the cluster aggregate. The
server separates enrolled identities; it does not deduplicate repeated
enrollment of the same physical host. The deployment therefore assumes one
collector identity per physical node. Multi-Mac LAN operation was not exercised.

## What the implementation actually provides

### Per-node observations and aggregates

The receiver namespaces object IDs by enrolled node ID, so separate daemon
identities do not overwrite each other's objects. The read layer selects each
node's owned objects before computing its summaries. Host detail requests that
specific node's inventory and displays its raw source measurements. See
[object identity](../server/src/cider_api.rs#L312),
[node summaries](../server/src/read_api.rs#L508), and
[host detail](../web/js/views.js#L72).

Cluster aggregates include eligible current observations and expose contributor
coverage. Offline/degraded owners are excluded from current cluster totals;
their retained observations become stale. With no valid contributor, the
aggregate value is null with state `unknown`. Zero host counts are valid; they
do not imply measured zero-byte capacity or zero I/O. See
[aggregation](../server/src/read_api.rs#L273) and
[owner availability](../server/src/read_api.rs#L530).

This is limited storage observation and availability assessment. Most health
dimensions remain unknown; there is no implemented alert engine or complete
disk-health diagnosis.

### Disk and volume grouping

The native diskutil/APFS collectors describe physical disks, partitions or
physical stores, APFS containers and volumes. The server exposes those links as
`parent_ids`. This is useful underlying data, but it is not the requested UI.
Host detail calls `vm.inventory.map(...)` and prints parent UUIDs inside each
independent object. The filesystem table also renders each record independently.
See [projection](../server/src/cider_api.rs#L358) and
[flat inventory rendering](../web/js/views.js#L77).

Mount observations and IOKit driver records are not fully joined to the physical
disk graph. Both physical disks and IOKit drivers are projected as `device`
objects. Stable IDs prevent duplicate insertion of the same source object, but
do not merge several representations into one user-facing physical disk.

Capacity accounting is a separate, implemented safeguard: APFS shared capacity
is counted at eligible container level, and one coherent observer is selected
for a shared identity. All five focused capacity regression tests passed in
this review. That does not establish visual disk deduplication.

### Joining nodes and hardware models

The visible core poll runs approximately every five seconds after completion
(thirty seconds while hidden). Node IDs, state, kind and name form a rack key;
changes rebuild the scene. No browser reload should be necessary for an enrolled
node to appear. The complete host table is separate from the 24-node rack cap.
See [rack updates](../web/js/app.js#L30),
[poll cadence](../web/js/app.js#L116), and
[rack limit](../web/js/stage.js#L65).

The model classifier was exercised directly with these controlled inputs:

| Reported model | Display label | Selected renderer |
| --- | --- | --- |
| null | Model not reported | iMac |
| MacBook Pro | MacBook Pro | MacBook |
| Mac mini | Mac mini | iMac — incorrect |
| Mac14,7 | Mac14,7 | iMac — no identifier mapping |

The exact rule is `/macbook/i.test(model) ? 'macbook' : 'imac'`.
The renderer implements only an iMac-style desktop and a laptop. There is no Mac
mini mesh and no end-to-end native hardware-family detection. See
[classification](../web/js/model.js#L71) and
[renderer branches](../web/js/stage.js#L155).

The missing native input is explicit: macOS `system_info()` reads the boot UUID,
OS version/build and architecture target, while the initial host resource has
only OS/build/architecture attributes. The server obtains `node.model` from
that host resource's `properties.model`, which native ciderd does not populate.
See [system information](../crates/ciderd/src/platform/macos/mod.rs#L80),
[host resource](../crates/ciderd/src/runtime.rs#L81), and
[model projection](../server/src/read_api.rs#L521).

## Evidence and limits

The focused capacity command was:

```sh
cargo test --package cider-server --locked --lib read_api::capacity_tests::
```

It passed five tests with no failures. The existing collector fixture test also
passed in this review:

```sh
cargo test --package ciderd --locked --test collectors physical_and_apfs_topology_keep_capacity_pools_separate -- --exact
```

That fixture checks two distinct physical disks, two volumes sharing one APFS
container, consistent partition/store IDs and nonduplicated pool accounting.
It verifies the underlying representation, not a grouped UI.

Read-only source reviews independently traced the collector, receiver and
frontend. Concise runtime checks are retained under ignored `.codex-staging/`
in `dual-node-qa-evidence.json` and `dual-node-qa-aggregate.json`; targeted logs
are `functional-capacity-tests.log` and `functional-topology-test.log`.
`dual-node-qa-validation.json` records the explicit assertions and cleanup.
Private test credentials/state were removed. Raw telemetry and credentials are
not included in this report.

The disposable verification processes were stopped after the checks. The
original user test session at `http://127.0.0.1:54276` remained running; a final
authenticated node read returned HTTP 200 with one online collector. No
production code was changed during this verification.
