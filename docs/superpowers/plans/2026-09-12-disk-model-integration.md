# Disk and Hardware-Model Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Complete physical-disk/volume/metric integration and correct hardware models while preserving explicit enrollment and existing multi-node behavior.

**Checkpoint (2026-09-13):** Tasks 1–7 are implemented and reviewed. Task 8's automated/native/browser checks and shared-specification native save/export verification are complete. Two-physical-Mac acceptance remains pending; no connector write or connector sign-off is claimed. See [the completion checkpoint](../../disk-model-integration.md).

**Continuation (2026-09-13):** The isolated real native session is running at
`http://127.0.0.1:54618`; fresh startup/API checks passed. The working in-app
Browser completed the real and synthetic scenario checks. The final web suite
passed 81 tests and the default workspace build passed after the rack-label fix.
The synthetic fixture is stopped. The user confirmed no second physical Mac is
available. Follow the [continuation handoff](../../handoff-disk-model-integration.md)
for current runtime, browser/export evidence and the remaining hardware gate.

**Architecture:** ciderd supplies native identity and correlation evidence; the server preserves acquisition context and builds an authoritative disk view plus an exact hardware classification. The browser renders canonical storage objects, selected-disk metrics and a paged rack from these additive contracts.

**Tech Stack:** Rust workspace, macOS IOKit/CoreFoundation/Disk Arbitration, SQLite, Axum, static Preact/Three.js modules, Rust tests and Node's test runner.

**Spec:** [Disk and hardware-model integration design](../specs/2026-09-12-disk-model-integration-design.md). Read both documents before execution. Baseline findings: [functional verification](../../functional-verification.md).

## Global Constraints

- Target Apple Silicon macOS; keep the current Rust workspace and static JavaScript frontend.
- Keep heartbeats at 5 seconds. Keep visible core polling at 5 seconds after completion and hidden polling at 30 seconds.
- Keep explicit TLS configuration and enrollment. Do not add LAN discovery or automatic enrollment.
- Run native collection only in ciderd, never in the central server.
- Preserve enrolled node UUIDs, resource UUIDs, boot/session/generation boundaries, exact u128 counter handling, and idempotent batches.
- Missing, ambiguous, unavailable, and stale observations must not become measured zeroes.
- Do not sum APFS shared pools, repeated mounts, or shared NFS capacity twice.
- Preserve existing raw inventory, metric, authentication, and pagination contracts; new read fields/routes are additive.
- Keep the rack rendering budget at 24 models per page; make every enrolled node reachable.
- Web assets are embedded at compile time; rebuild the server after web changes.
- Do not commit, push, package, deploy, or interrupt the user's running test session as part of planning.
- During API implementation, update the shared Server Spec in native Google Docs formatting; a local document is not its substitute.

Execution also preserves the existing uncommitted changes and local excluded
`AGENTS.md`; its staged tracked-file removal must not be reversed. This plan
does not authorize commits or replace the running application. Test new binaries
with disposable state/ports and verified TLS, then provide a separate handoff.

---

## Sequence, boundaries and file ownership

```text
1. Receiver acquisition context ───────┬──> 5. Disk read projection ──> 6. Storage UI
2. Optional hardware + catalog ────────┤                              │
3. Native correlation evidence ──> 4. Derived topology ───────────────┘
2. Optional hardware + catalog ───────────> 7. Models and rack paging
1–7 ────────────────────────────────────> 8. End-to-end checks and shared spec
```

Tasks 1 and 3 can run independently. Task 2 can run beside them, coordinating
shared `runtime.rs`, platform and server read files. Task 7 can start once Task 2's
contract is fixed. Integrate server/collector changes before Task 6's live checks.
Review each deliverable and its targeted tests before proceeding; keep test
results distinct from the final physical-hardware gates.

| New file | Responsibility |
| --- | --- |
| `crates/ciderd/src/hardware.rs` | Optional hardware observation attributes and pure failure handling |
| `crates/ciderd/src/topology.rs` | Relationship-only correlation and lifecycle reconciliation |
| `server/src/hardware.rs` | Exact identifier catalog lookup and node hardware projection |
| `server/data/apple-hardware-models.json` | Versioned identifiers, families, display names and primary sources |
| `server/src/disk_view.rs` | Pure validated disk index, typed edges and physical membership |
| `web/js/storage.js` | Canonical storage presentation and selected-disk history |
| `web/js/storage-views.js` | Storage groups, rows, raw details and selected-disk panel |
| `web/js/rack.js` | Pure stable ordering/paging and explicit family selection |
| `web/tests/storage.test.mjs`, `web/tests/rack.test.mjs` | Browser-independent state, topology and interaction-model regressions |
| `scripts/smoke-storage.mjs` | Disposable authenticated end-to-end integration harness |

Use existing files for receiver state, read routing, scheduling and app wiring.
Do not refactor unrelated server APIs, NFS operations, metric arithmetic or styles.

## Task 1: Preserve resource and relationship acquisition context

**Files:** Modify `server/src/cider_api.rs`; test `server/tests/cider_compat.rs`.

**Interface:** Add a versioned, serde-defaulted context map to persisted `Graph`.
Keys follow the existing resource/relationship entity keys. Each accepted upsert
records the following JSON representation; all fields describe that acceptance,
not a later projection:

```json
{
  "schema_version": 1,
  "boot_id": "boot-a",
  "agent_generation": "2",
  "agent_session_id": "session-a",
  "observed_at": "2026-09-12T12:00:00Z",
  "received_at": "2026-09-12T12:00:01Z",
  "source": "diskutil.list.physical",
  "source_generation": null
}
```

`source` and `source_generation` are nullable unless established by the resource
or correlation evidence. Projection emits `properties.ciderd_acquisition` and
per-edge acquisition metadata. An absent old context stays unknown. The receiver
continues using its existing SQL transaction and persisted JSON; no separate
schema migration or identity migration is needed for these additive maps.

- [x] Add receiver regressions for retained resources across boot/generation changes, duplicate heartbeat replay, actual fresh upserts, tombstones and old persisted JSON without the new map. Express the key assertion with real heartbeat fixtures:

  ```text
  accept generation 1 / boot A containing disk D observed at T1
  accept generation 2 / boot B without a new observation for D
  project inventory
  assert D's acquisition boot is A and observed_at is T1
  assert D cannot authorize a current boot-B physical association
  replay the second heartbeat
  assert context and object identity are unchanged
  ```

- [x] Run `cargo test --package orchard-server --locked --test cider_compat` and confirm the new provenance assertions expose the missing behavior.
- [x] Capture context only when `accept_version()` accepts an entity upsert. Remove it on confirmed tombstones; preserve original observation times on unchanged/replayed resources. Add `#[serde(default)]` fields and explicitly handle legacy missing context.

  ```text
  if accept_version(entity):
      graph[entity] = accepted value
      graph.acquisition[entity] = context from this accepted upsert
  project_inventory:
      copy graph.acquisition[entity] if present
      otherwise expose unknown provenance
  ```

- [x] Verify that same-node boot/session checks, idempotence, exact counter derivation and existing capacity tests still pass with `cargo test --package orchard-server --locked`.
- [x] Review the diff specifically for accidental current-heartbeat timestamp substitution. Deliver retained evidence that can safely feed Task 5.

## Task 2: Acquire optional hardware identity and classify it once

**Files:** Create `crates/ciderd/src/hardware.rs`, `server/src/hardware.rs`,
`server/data/apple-hardware-models.json`; modify `crates/ciderd/src/lib.rs`,
`crates/ciderd/src/platform/mod.rs`, `crates/ciderd/src/platform/macos/mod.rs`,
`crates/ciderd/src/runtime.rs`, `server/src/lib.rs`, `server/src/read_api.rs`;
test `crates/ciderd/tests/runtime.rs`, `server/tests/cider_compat.rs`, and unit
tests in both new modules. If the safe native sysctl adapter requires a C export,
keep that export inside existing `platform/macos/native.c`.

**Interfaces:**

```rust
// ciderd::hardware; Attributes is the existing model map.
pub fn hardware_attributes(
    model: Result<String, String>, observed_at: &str
) -> crate::model::Attributes;

// server::hardware; JSON output is the design's exact node.hardware contract.
pub fn hardware_view(
    attributes: &serde_json::Value,
    acquisition: &serde_json::Value,
    current_boot: Option<&str>
) -> serde_json::Value;
```

Store optional model result in `SystemInfo`. The mandatory boot-ID result remains
independent. Initial host attributes use `hardware_attributes`; no strict
`AgentInfo`, credential, identity-file or enrollment changes occur.

- [x] Add exact-mapping tests for `Mac14,7` → `macbook`, `Mac14,3` and `Mac14,12` → `mac_mini`, a verified Apple Silicon iMac entry → `imac`, and an unknown identifier → `unknown`. Include an absent model and failed model query. A shared identifier prefix must not determine family.

  ```rust
  let unknown = serde_json::json!({
      "model_identifier": "Mac999,1", "hardware_state": "ok",
      "hardware_source": "sysctl.hw.model"
  });
  let context = serde_json::json!({"boot_id":"boot-a"});
  let view = hardware_view(&unknown, &context, Some("boot-a"));
  assert_eq!(view["machine_family"], "unknown");
  assert_eq!(view["model_identifier"], "Mac999,1");
  assert_eq!(view["reason"], "identifier_unmapped");
  ```

- [x] Run the targeted hardware tests and confirm the new contract is absent before implementing it.
- [x] Acquire `hw.model` with a bounded native query. Limit the accepted identifier to a nonempty UTF-8 string of at most 128 bytes. Missing/invalid/native-failure results produce `hardware_state=unavailable` and `hardware_reason=model_query_failed`; they do not fail enrollment or delay heartbeats with retries.
- [x] Build the exact versioned catalog from the Apple sources linked in the design, including the current Apple iMac identifier list. Record source URLs in the catalog, reject duplicate identifiers and family values outside the four-value contract. Unknowns keep their raw identifier.

  ```json
  {
    "version": "2026-09-12.1",
    "models": {
      "Mac14,7": {"machine_family":"macbook","display_name":"MacBook Pro (13-inch, M2, 2022)","source":"https://support.apple.com/en-us/108052"},
      "Mac14,12": {"machine_family":"mac_mini","display_name":"Mac mini (2023)","source":"https://support.apple.com/en-us/102852"}
    }
  }
  ```

  These are seed examples, not the complete supported-model catalog. Include all
  Apple Silicon entries in the four supported product lists available at execution.

- [x] Project `node.hardware` on list/detail reads using Task 1's context. Retain `node.model`; do not interpret legacy arbitrary names as a verified family. Add source-state/boot-mismatch tests and old-collector round trips.
- [x] Run `cargo test --package ciderd --locked` and `cargo test --package orchard-server --locked`. Confirm a failed model query still produces an otherwise valid initial heartbeat.
Shared-specification native writing/export verification for this contract completed under Task 8. No connector write or connector sign-off is claimed.

## Task 3: Acquire exact native driver and local-mount evidence

**Files:** Modify `crates/ciderd/src/platform/macos/native.c`,
`crates/ciderd/src/platform/macos/mod.rs`, `crates/ciderd/src/platform/mod.rs`,
`crates/ciderd/build.rs`, `crates/ciderd/src/collectors/inventory.rs`,
`crates/ciderd/src/collectors/mounts.rs`, `crates/ciderd/src/collectors/mod.rs`,
`crates/ciderd/src/scheduler.rs`; add fixtures under
`crates/ciderd/tests/fixtures/` and tests in `crates/ciderd/tests/collectors.rs`
and `crates/ciderd/tests/command.rs`.

**Interface:** Optional IOKit evidence becomes source resource attributes, not
new counters or invented physical resources:

```json
{
  "whole_media_candidates": [
    {"bsd_name":"disk0","registry_entry_id":"4294968000","whole":true}
  ],
  "media_mapping_state": "ok"
}
```

Add `WorkerRequest::MountIdentity { path: String, fsid: [i32; 2], source: String }`
and strict operation `mount-identity` input validation. Worker output contains
`fsid`, `source`, nullable `volume_uuid`, `media_bsd_name`, `media_registry_id`,
`state` and `reason`. Scheduler attaches the existing mount generation and
rejects late results. The mount parser places valid evidence on the existing
mount resource. Network mounts are excluded before scheduling.

- [x] Add fixtures named `iokit-whole-media.plist`, `mount-identity.json`, and `mount-identity-race.json`. Cover direct whole media, a nested partition, virtual media, two candidates, duplicate UUID and before/after fsid mismatch. Preserve exact maximum-width counter fixtures.

  ```text
  driver D has direct whole child disk0 and nested partition disk0s2
  assert evidence reports disk0; nested partition does not become ownership
  repeat with failed media lookup and unchanged driver counters
  assert counter values and source epoch are byte-for-byte unchanged
  lookup local mount at fsid A; receive result for fsid B
  assert result is rejected and no mount relationship is authorized
  ```

- [x] Run the collector and command tests to establish failures for the new evidence fields and worker operation.
- [x] Extend direct IOMedia enumeration in the existing bounded IOKit worker. Read each child's own properties, release all IOKit/CoreFoundation handles on every path, keep 4 MiB/4096-record bounds, and mark mapping failure independently from statistics.

  ```text
  for each existing IOBlockStorageDriver:
      collect existing statistics unchanged
      inspect direct IOService-plane children conforming to IOMedia
      record own Whole, BSD name and registry ID as optional evidence
      never perform recursive property lookup to assign ownership
  ```

- [x] Link DiskArbitration in `build.rs` and implement the supervised local-only mount worker using `DADiskCreateFromVolumePath` and `DADiskCopyDescription`. Check fsid/source before and after acquisition; reuse scheduler deadlines/output limits and mount-generation rejection. Do not query network paths.
- [x] Run a bounded native probe against this Mac's boot snapshot and one ordinary local volume. Capture only redacted identity relationships in a new fixture `mount-identity-boot-snapshot.json`. Compare output to the current physical/APFS graph; reject guessed suffix matches and nonunique UUID matches.
- [x] Close the native proof gate only if both mount classes resolve through authoritative evidence. If boot-snapshot resolution fails, record the exact failed evidence and revise this adapter before approving full boot-volume support; do not mask the failure with a guessed parent.
- [x] Run `cargo test --package ciderd --locked --test collectors` and `cargo test --package ciderd --locked --test command`; verify the optional lookup cannot block the five-second heartbeat scheduler.

## Task 4: Reconcile derived associations without changing source identities

**Files:** Create `crates/ciderd/src/topology.rs`; modify
`crates/ciderd/src/lib.rs`, `crates/ciderd/src/state.rs`,
`crates/ciderd/src/scheduler.rs`; add `crates/ciderd/tests/topology.rs` and lifecycle
cases in `crates/ciderd/tests/state.rs`.

**Interface:** Define `TopologyInput` in the new module with node/boot/session
identity, existing `Resource` values, source freshness, and accepted source/mount
generations. Define `TopologyResult` as derived `Relationship` values plus
resource-keyed association states/reasons. Expose:

```rust
pub fn reconcile(input: &TopologyInput) -> TopologyResult;
```

The state/scheduler adapter supplies accepted evidence and replaces only the
derived relationship group. It must not claim or remove another source's resources.
Relationship attributes carry `source=derived.storage`, mapping method,
evidence resource IDs, evidence generations, and acquisition state/time.

- [x] Build fixture-driven tests in `topology.rs` for all six arrival orders of physical, APFS and driver inventories; confirmed removal; failed refresh; stale boot/session; same BSD replacement; mount-generation change; and multi-store APFS.

  ```text
  physical source contains unique disk0 P; driver D reports whole media disk0
  reconcile in every source arrival order
  assert exactly one D --attached_to--> P edge
  replace P with a new physical identity using the same BSD name
  assert old edge is removed and the association continuity changes
  assert D's unchanged raw counter epoch is not reset
  add conflicting current driver D2
  assert disk I/O attribution is ambiguous, with no totals fanned out
  ```

- [x] Run `cargo test --package ciderd --locked --test topology` and the relevant state tests before implementing reconciliation.
- [x] Implement exact candidate joins, endpoint/boot/session validation, unique-match requirements and cycle-safe relationship output. Link local mounts to a unique confirmed filesystem; preserve the existing APFS graph and multi-store backing.

  ```text
  group physical candidates by exact BSD locator within current source context
  join a driver only when its whole-media evidence and candidate are unique
  resolve mount by authoritative volume/media identity plus source generation
  on failed source refresh: retain dated edge with stale association state
  on confirmed removal or changed identity: invalidate and replace the edge
  ```

- [x] Integrate after successful source-state updates and failure/removal transitions, with no placeholder endpoints and no resource ownership transfer. Increment relationship revision only when its accepted content changes; preserve batch retry idempotence.
- [x] Run `cargo test --package ciderd --locked --test topology`, `cargo test --package ciderd --locked --test state`, and the full ciderd suite. Verify unchanged counters/rates across identical mapping refreshes and no late mount result creating a relationship.
Shared-specification native writing/export verification for this contract completed under Task 8. No connector write or connector sign-off is claimed.

## Task 5: Add the authoritative disk projection and read contract

**Files:** Create `server/src/disk_view.rs`; modify `server/src/lib.rs`,
`server/src/read_api.rs`, `server/src/cider_api.rs`; add unit tests inside
`disk_view.rs`, contract cases in `server/tests/cider_compat.rs`, and read-route
cases in `server/src/tests.rs`.

**Interfaces:** Keep JSON compatibility with existing read objects while keeping
the graph algorithm in one focused module:

```rust
pub struct DiskIndex {
    pub disks: Vec<serde_json::Value>,
    pub objects: Vec<serde_json::Value>,
    pub topology_revision: String,
    pub disk_inventory: serde_json::Value,
}
pub fn build_disk_index(
    node: &serde_json::Value, objects: &[serde_json::Value]
) -> DiskIndex;
```

`objects` contains the existing rows plus the design's normalized typed edges,
physical roots and topology states. `disks` is exactly the design's DiskSummary
contract. All inputs come from one committed read snapshot; the helper performs
no I/O or native collection. Reuse the index across routes within that snapshot.

- [x] Add tests for one disk/two volumes, two disks/one pool, multiple mounts of one filesystem, independent HFS, network and virtual records, missing/stale/ambiguous driver evidence, cycles, wrong-node edges and missing persisted acquisition context.

  ```text
  P0 and P1 back shared APFS pool C containing volumes V0 and V1
  build_disk_index(node, objects)
  assert exactly two disk summaries and one C object
  assert C.physical_disk_ids equals [P0, P1]
  assert both disk summaries reference C in topology.shared_pool_ids
  assert C's capacity is not assigned twice as exclusive disk capacity
  assert unresolved driver linkage produces null rates, never node totals
  ```

- [x] Establish failing tests with `cargo test --package orchard-server --locked --lib disk_view::`.
- [x] Implement typed-edge normalization to node-namespaced object UUIDs, bounded traversal and canonical physical roots. Accept physical backing only from authoritative source evidence. Give empty-success, unsupported legacy, unavailable and stale physical inventory distinct `disk_inventory` metadata.
- [x] Select exactly one confirmed driver source per disk; reuse stored Measurement/rate results and provenance. Preserve exact strings, tiny nonzero rates and failed/reset source states.

  ```text
  confirmed unique driver + valid acquisition context -> existing driver rates
  absent/ambiguous/stale linkage -> Unknown Measurements with reason
  continuity_key = digest(node, disk, boot, agent generation/session,
                          driver, counter epoch, semantic association identity)
  topology_revision = digest(structural identities and ownership evidence)
  exclude ordinary metric values and refreshed timestamps from both digests
  ```

- [x] Add `/api/v1/nodes/{node_id}/disks`, inventory `disk_id` filtering and metadata from the design. Route dispatch must distinguish the full subresource path; `/disks` must not fall through to the node-detail case. Preserve generic inventory access and object IDs.
- [x] Extend frozen page storage with the new metadata; test that every page has identical structural stamps and that cursors remain bound to owner/path/query. Test correct disk-node ownership, malformed UUID, old generation, wrong role, expiry and bounded cache failures. Advertise the new route/feature in capabilities without claiming metric history or alerts.
- [x] Run `cargo test --package orchard-server --locked` and `cargo check --workspace --locked`. Confirm existing local/shared-capacity and counter tests remain green.
Shared-specification native writing/export verification for this contract completed under Task 8. No connector write or connector sign-off is claimed.

## Task 6: Render grouped disks and per-disk metrics with independent loading

**Files:** Create `web/js/storage.js`, `web/js/storage-views.js`,
`web/tests/storage.test.mjs`; modify `web/js/api.js`, `web/js/session.js`,
`web/js/app.js`, `web/js/views.js`, `web/js/model.js`, `web/css/live.css`,
`server/src/read_api.rs`, `web/README.md`; extend
`web/tests/integration.test.mjs` and `web/tests/metrics.test.mjs` where applicable.

**Interfaces:** Export these pure/state boundaries; define their object shapes
in module JSDoc using the design's wire contracts:

```javascript
// storage.js
export function storageVM(disks, inventory) { /* canonical display graph */ }
export function appendDiskHistory(history, sample, maxPoints = 180) { /* history */ }
// session.js
export async function readDisks(client, node) { /* { data, meta, arrival clocks } */ }
export class StorageInventory {
  constructor(client, nodeId, onChange) { /* one frozen traversal */ }
  async refresh(generation) { /* start replacement snapshot */ }
  async continue() { /* budgeted page continuation */ }
  snapshot() { /* rows, meta, complete, loadedCount, error, observation dates */ }
  close() { /* abort and ignore obsolete responses */ }
}
// storage-views.js
export function NodeStorage(props) { /* canonical groups and selected disk */ }
export function DiskMetricsPanel(props) { /* rates, history, source details */ }
```

These comments describe each public boundary, not bodies to paste as an
implementation. Task steps below define the required algorithms and assertions.
The storage view model returns `{disks, sharedPools, virtual, network, unresolved}`;
each entry has `objectId`, `children`, and `references`, with one canonical entry
per object. A fixture helper `canonicalIds(vm)` in the test file walks `children`
across all five sections, ignoring references.

- [x] Add a complete synthetic storage snapshot fixture directly in `storage.test.mjs`: disk P, container C, volumes V1/V2, mount M and driver D; then a two-disk shared-pool variant. Assert physical disk count, canonical object uniqueness and correct typed nesting.

  ```javascript
  const ids = canonicalIds(storageVM(disks, inventory));
  assert.equal(ids.filter(id => id === 'P').length, 1);
  assert.equal(ids.filter(id => id === 'C').length, 1);
  assert.equal(new Set(ids).size, ids.length);
  ```

  Also assert that every accessible raw storage object appears once as a canonical
  entry or raw-detail source, and that references do not contribute capacity.

- [x] Add loader tests with fake clients: mismatched stamps; changing generation; frozen-page metadata mismatch; 410 and 429; delayed obsolete-node responses; five pages for 2,048 objects; an initial incomplete traversal; and unchanged topology with newer disk metrics. Count requests to prove no per-disk fan-out.
- [x] Run `node --test web/tests/storage.test.mjs web/tests/integration.test.mjs` to expose missing behavior.
- [x] Implement the canonical view model using server physical roots and typed edges. Place multi-backed pool subtrees in `sharedPools`; reference them from member disks. Prefer mount→filesystem→container nesting; use stable UUID ordering to break display-parent ties. Detect cycles/dangling endpoints without fabricating backing. Preserve raw source details and labeled unassigned sections.

  ```text
  create one canonical entry per object UUID
  choose a typed display parent only within its server-assigned ownership section
  shared-pool roots have no exclusive disk parent
  all extra relationships become references to canonical entries
  physical groups display hardware size; pools display pool capacity
  volumes display usage/quota; no rendered-layer sum constructs node totals
  ```

- [x] Implement disk-summary reads and metadata-preserving inventory pagination independently of the core poll. Use pages of 500, one traversal, at most eight topology page requests per 30 seconds, and Retry-After backoff. Continue incomplete frozen snapshots within their 300-second TTL. Cache structure by node/boot/topology revision; update raw observations at most every 30 seconds or by explicit Refresh. Keep source dates visible.

  ```text
  timely disk summary with same cached topology stamp -> update live disk rates
  new topology stamp -> load a replacement frozen inventory independently
  complete matching traversal -> atomically replace structure
  mismatch/partial/error -> retain dated context; never publish mismatched rates
  obsolete route, node, boot or closed request -> discard result
  ```

- [x] Extend route parsing to `#node/{node_id}/disk/{object_id}` with existing fragments preserved. Wire selection/expansion into HostDetail; retain selection on reorder and show removal explicitly. Add canonical host/disk links to the Filesystems table. Expose keyboard controls and compact responsive nesting without hiding unavailable objects.
- [x] Implement selected-disk history with existing SI formatting, exact raw counters and at most 180 points. Use monotonic browser arrival clocks; retain source times separately. Split on identity/boot/session/relevant generation/continuity changes, errors, stale data and long pauses. Never append a retained poll as a fresh success. Test a tiny nonzero rate, an unavailable interval, counter reset and reassociation.
- [x] Add both storage modules to the server's embedded asset allowlist. Run all web tests and syntax checks, then `cargo build --workspace --locked` before browser verification. Automated tests verify independent core polling during delayed storage loading; rendered checks completed under Task 8.

## Task 7: Render correct hardware families and page the rack

**Files:** Create `web/js/rack.js`, `web/tests/rack.test.mjs`; modify
`web/js/model.js`, `web/js/stage.js`, `web/js/app.js`, `web/js/views.js`,
`web/css/live.css`, `server/src/read_api.rs`; extend existing web tests.

**Interfaces:**

```javascript
// rack.js: no Three.js dependency, suitable for Node tests.
export function rackFamily(hardware) { /* exact allowed family or unknown */ }
export function rackPage(nodes, pageIndex, selectedNodeId = null, pageSize = 24) {
  /* { nodes, pageIndex, pageCount, rangeStart, rangeEnd, total } */
}
```

`stage.js` exposes four internal factories with the existing materials:
`makeMacbook`, `makeMacMini`, `makeIMac`, `makeUnknown`. `bootStage` receives only
the selected page, removes its internal first-24 truncation, and retains proper
geometry/material/event disposal on rebuild.

- [x] Add explicit-family tests and a 57-node fixture in stable enrolled-time/UUID order. Verify 24/24/9 page sizes, a join on a later page, selected-host page navigation, page clamping after removal and zero nodes.

  ```javascript
  assert.equal(rackFamily({ machine_family: 'mac_mini' }), 'mac_mini');
  assert.equal(rackFamily({ model_identifier: 'Mac14,12' }), 'unknown');
  const page = rackPage(nodes, 1);
  assert.equal(page.nodes.length, 24);
  assert.deepEqual([page.rangeStart, page.rangeEnd, page.total], [25, 48, 57]);
  ```

- [x] Run `node --test web/tests/rack.test.mjs` before implementing the paging/family logic.
- [x] Replace model-string guessing with `node.hardware.machine_family`. Use the laptop mesh for `macbook`, a low desktop enclosure without a monitor for `mac_mini`, the display/stand for `imac`, and a neutral enclosure for `unknown`. Keep raw model identifier and availability separate in visible labels.
- [x] Implement stable ordering, page preservation and selected-host navigation in `rackPage`. Have the app build the stage key from the supplied page and each node's ID, name, availability and explicit family; changes after a successful poll rebuild only the necessary scene state.
- [x] Place Previous/Next controls and range text outside the canvas's noninteractive overlay. Preserve complete host-list access and existing automatic join behavior; no discovery or enrollment changes.
- [x] Embed `rack.js`; run all web tests and rebuild embedded assets. Automated tests cover explicit families, 57-host paging and selection; visual/keyboard cases completed under Task 8.

## Task 8: Verify the complete flow and finish the shared specification

**Files:** Create `scripts/smoke-storage.mjs`; modify `scripts/smoke-ciderd.mjs`
only for reusable fixture-safe harness support, `web/README.md`,
`docs/functional-verification.md`, and `docs/integration-review.md` with new
evidence; update the shared Google Doc. Keep private captures and temporary
credentials out of tracked files and shared documentation.

**Harness contract:** `node scripts/smoke-storage.mjs` uses built binaries,
ephemeral loopback ports, a private CA with verification enabled and disposable
state. It cleans up only children/state it owns in success, failure and signal
paths. Controlled daemon identities and recorded storage graphs prove integration
semantics; they must be labeled synthetic/same-host as applicable.

- [x] Implement the harness using the existing smoke script's verified-TLS enrollment/read pattern. Verify an empty server, explicit enrollment of the real local collector, disk/mount/hardware reads, embedded assets, two distinct five-second heartbeats and degraded/offline retention. Multi-disk/shared-pool, ambiguity and 57-host cases are covered by the automated suites; this live harness runs on one physical Mac.

  ```text
  empty server -> zero current contributors; null/unknown capacity and rates
  one real local Mac -> one physical disk, one resolved driver, ten linked mounts
  disk/inventory reads -> coherent topology and embedded storage modules
  two heartbeats -> distinct five-second observations
  stop the harness collector -> degraded/offline retained host; unknown current totals
  ```

- [x] Complete the native boot-snapshot and ordinary-volume identity gate from Task 3. Both uniquely matched the physical/APFS graph; the boot snapshot required direct IOKit parent evidence.
- [x] Complete final authenticated browser coverage against rebuilt assets. The actual MacBook (`Mac15,6`) overview/host/disk0 deep link, live disk rates and unknown fields passed. The actual production app with synthetic data passed all four meshes, Tab/Enter/Space paging at 24/24/9, host 57 mesh selection/page retention, host 58 joining through normal polling, shared-pool uniqueness/exact counters, 503/429 retained context with independent core polling, recovery gaps/isolated points, and disk/host removal/restoration. Actual 390 × 844 browser surface/DOM checks covered disk/rack layouts; final desktop/hover labels passed at 1280 × 900. Both sessions had no browser warning/errors. Dense-rack overlap was fixed in `stage.js`, `rack.js` and `console.css` with five added geometry tests; meshes, identities, paging and pointer/camera behavior were preserved. See the checkpoint's `browser-*.png/txt` evidence. Fixture rendering does not establish hardware acceptance.
- [x] Run final appropriate checks after integrated changes. Prior run: 154 Rust tests passed with three subprocess-helper entry points intentionally ignored; 76 web tests and default/headless checks/builds passed. Fresh 2026-09-13 run after the label-only fix: **81 web tests**, JavaScript syntax/diff checks and default workspace build passed. No Rust behavior changed, so the prior Rust suite/headless matrix and stopped-collector workload were not repeated. Fresh logs: `.codex-staging/browser-final-{web-tests,build,native,live-verify}.log`.

  ```sh
  cargo test --workspace --locked --no-fail-fast
  cargo check --workspace --locked
  cargo check --package orchard-server --locked --no-default-features
  cargo build --workspace --locked
  cargo build --package orchard-server --locked --no-default-features
  node --test web/tests/*.test.mjs
  node --check web/js/storage.js
  node --check web/js/storage-views.js
  node --check web/js/rack.js
  node --check web/js/session.js
  node --check web/js/app.js
  node --check web/js/views.js
  node --check web/js/stage.js
  git diff --check
  node scripts/smoke-storage.mjs
  ```

  Default and headless builds can overwrite the same executable path; rebuild the
  desired feature set before its UI/native-shell check. Capture exact counts and
  explain intentionally ignored subprocess helpers; do not reuse older counts.

- [ ] Run the physical-hardware acceptance gate with one MacBook and one Mac mini explicitly enrolled to the same head node over verified TLS. Compare reported identifiers to each machine, inspect distinct disks/metrics and corresponding meshes, verify automatic membership and aggregate arithmetic, then stop only test collectors to check offline behavior. If either machine is unavailable, mark this gate pending; two local daemon processes do not satisfy it.
- [x] Complete shared Server Spec section 19 by native Google Docs UI save/export, covering final collector fields, relationship evidence, hardware, disk route/filter, frozen metadata, auth/status examples, UI/build behavior and actual verification. “Saved to Drive” was observed. Independent final DOCX export verification preserved sections 1–18, found exactly one section 19 with one Heading 1/eight Heading 2 headings and ten native tables, matched all 46,833 non-whitespace body characters and all ten parsed JSON examples, and confirmed 571 code/request paragraphs in Roboto Mono 9 pt with light shading. Evidence: `.codex-staging/server-spec-section19-published-validation.json`, `browser-spec-{before,after}-docx.txt`, and the prepared section19 HTML/TXT/inline fragment. This closes the native write/export task only; no connector write or successful connector sign-off is claimed.
- [x] Write [the local checkpoint](../../disk-model-integration.md) separating implementation, automated/native/browser verification and pending hardware acceptance, and document the smoke/startup workflow. Earlier sessions are stopped; current real runtime is port 54618 with fresh `.codex-staging/storage-browser-state.json` metadata. Synthetic fixture tabs were closed and its launcher exited successfully. The real session remains running for the user. No commits, pushes, packaging or deployment occurred.

## Acceptance matrix and deferred work

| Outcome | Tasks | Required evidence |
| --- | --- | --- |
| Independent node/disk views and correct per-disk source rates | 1, 3–6, 8 | Exact source mapping, same BSD on distinct nodes, unknown/ambiguous states, authenticated disk UI |
| Several volumes grouped with one physical disk | 3–6, 8 | Canonical-object tests plus actual native boot-volume and browser checks |
| Shared APFS pool shown/counted once | 4–6, 8 | Multi-store fixture, one pool row, no exclusive-capacity fan-out |
| Empty/unselected/offline aggregate behavior preserved | 5, 6, 8 | Empty and stopped-collector API/browser checks |
| Joining hosts visible beyond 24 | 7, 8 | 57-node paging fixture and live join without reload |
| MacBook/Mac mini identified and rendered correctly | 2, 7, 8 | Exact catalog tests, four visible mesh families, two physical Macs |
| Old collectors and cached snapshots remain safe | 1, 2, 5, 6 | Missing metadata, legacy raw inventory, frozen cursors, mismatch/expiry tests |
| Shared specification reflects implementation | 2, 4, 5, 8 | Saved native-format contracts, exported text review and honest write status |

Automatic LAN discovery/enrollment, historical metric storage, full alerts,
network-link discovery and automatic deduplication of multiple collectors on
one physical Mac are excluded. The separate duplicate-observer follow-up needs
an explicit administrator ownership/admission policy; deployment continues to
assume one enrolled collector identity per physical host. Nothing in this plan
merges identities or silently changes aggregate contributor policy.

## Planning review record

The design's four requested outcomes map to the acceptance rows above. Native
proof and two-Mac checks remain explicit gates. Contracts use one server hardware
catalog, one disk-ownership algorithm, consistent family/state/field names and
metadata-preserving traversals. Tasks 1–7 are complete. Automated verification and the one-Mac native TLS smoke
passed. The working in-app Browser completed the remaining scenario checks, and
native Google Docs save/export verification completed section 19. Two-physical-Mac
acceptance is the only remaining planned Task 8 gate; connector write/sign-off is
unclaimed separately. The real test session remains running as recorded in the
2026-09-13 continuation handoff.
