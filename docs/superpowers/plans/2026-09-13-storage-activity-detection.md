# Storage Activity Detection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox syntax for tracking.

**Goal:** Implement the approved first security slice: per-native-driver sustained
read/write deviations with durable evidence, coverage, and explicit relearning.

**Architecture:** A pure typed state machine consumes newly accepted native
collection attempts inside the existing SQLite ingestion transaction. Separate
findings and source-status reads feed independent Security view requests.

**Tech Stack:** Rust, serde, Axum, SQLite/sqlx, Preact/htm, Node test runner.

**Spec:** `docs/superpowers/specs/2026-09-13-storage-activity-detection-design.md`

## Global Constraints

- macOS collection remains in ciderd; heartbeats remain five seconds.
- Native schema-2 `iokit.block` driver read/write byte counters only.
- Preserve exact unsigned-128-bit subtraction, identities, replay behavior,
  monotonic intervals, unavailable states, and partial field coverage.
- Baseline horizon 1,800 seconds; readiness 600 covered seconds and 60 intervals;
  1-15 second intervals; at most 1,800 baseline intervals.
- Weighted nearest-rank P95 B; H=max(3B,B+1,000,000 B/s), strict elevation for
  120 seconds; L=max(2B,B+500,000 B/s), recovery at/below L for 60 seconds.
- Pending/open/recovery observations never train; reference remains frozen.
- Missing telemetry never resolves a finding. Identity loss or deliberate
  relearning interrupts it. No findings is not healthy security.
- Baseline ring <=256 KiB, complete state <=512 KiB, <=8,192 active admitted
  source/direction states. Exceeding admission limits must remain visible.
- Closed finding retention 30 days; unresolved findings remain inspectable.
- Viewer reads only; relearning requires administrator authentication and the
  existing mutation replay/timestamp safeguards.
- Rebuild embedded assets. Update the shared Server Spec during this work using
  native formatting and verify the save/export; never claim a connector write
  without one.
- No commit, push, merge, package, deployment, or changes to running user state
  as part of this implementation. The earlier checkpoint commit is complete.

## Shared interfaces and JSON contract

The pure module exports `SourceIdentity`, `Continuity`, `Observation`,
`SourceState`, `SourceSummary`, `Finding`, and `Policy`, all relevant persisted
types deriving serde serialization/deserialization and Clone. Use
`crate::cider_wire::Decimal` for persisted exact counter and monotonic values.

```rust
// detection.rs; these are the integration entry points.
impl SourceState {
    pub fn new(source: SourceIdentity) -> Self;
    pub fn observe(&mut self, observation: Observation) -> Vec<Finding>;
    pub fn interrupt(&mut self, reason: &str, now_ms: i64) -> Option<Finding>;
    pub fn rebaseline(&mut self, reason: &str, now_ms: i64)
        -> Result<Option<Finding>, String>;
    pub fn summary(&self, now_ms: i64, owner_online: bool) -> SourceSummary;
}
```

`SourceIdentity` fields: `source_id`, `node_id`, `object_id`, `resource_id`,
`direction` (`read|write`), `metric` (native metric name), `collector`, `scope`,
and `attributes` (canonical ordered metric attributes). String IDs are UUIDs.
`Continuity` contains string `boot_id`, `agent_generation`, `agent_session_id`,
`clock_id`, `counter_epoch`, `source_version`, `adapter_version`, and policy
version. Source identity is independent of unrelated inventory revisions.
`Observation` contains `collection_id`, `continuity`, `finished_monotonic_ns`
(Decimal), `observed_at` (RFC3339), `received_at_ms` (i64), `counter`
(Option<Decimal>), `unavailable_reason` (Option<String>),
`age_at_receipt_seconds` (f64), and `stale_after_seconds` (f64).

`SourceState` exposes `source` and `baseline_revision` to persistence; the revision
is u64 internally and a decimal string in reads. Its summary JSON is:

```json
{
  "source_id":"uuid", "node_id":"uuid", "object_id":"uuid",
  "resource_id":"uuid", "direction":"read", "metric":"storage.device.read_bytes_total",
  "rule_id":"storage.activity.high_rate.v1", "policy_version":"1",
  "baseline_revision":"1", "active":true, "support_state":"supported",
  "baseline":{"state":"learning","covered_seconds":0,"interval_count":0,
    "required_seconds":600,"required_intervals":60,
    "reference_rate_bytes_per_second":null,"high_threshold_bytes_per_second":null,
    "recovery_threshold_bytes_per_second":null},
  "episode":{"state":"quiet","finding_id":null,"elevated_seconds":0,"recovery_seconds":0},
  "observation":{"state":"unavailable","reason":"no_observations","observed_at":null,
    "received_at":null,"age_seconds":null,"stale_after_seconds":15,
    "rate_bytes_per_second":null}
}
```

Observation states are current, unavailable, stale. Baseline states are learning,
ready. Episode states are quiet, pending, open. Interrupted/removed source records
remain visible as inactive; their last observations do not become current.
Capacity-limited sources use `support_state=capacity_limited` and a reason.

Finding JSON includes UUID `finding_id`, `source_id`, `node_id`, `object_id`,
`resource_id`; `direction`, `rule_id`, `status=open|resolved|interrupted`,
`severity=warning`, `summary`; RFC3339 `first_seen_at`, `last_seen_at`, `opened_at`,
`updated_at`, nullable `ended_at`; nullable `reason`; `baseline_revision` string;
`policy`; `baseline` with frozen reference/thresholds/coverage; and `evidence` with
`first`, `latest`, `peak` interval records plus `elevated_seconds` and
`recovery_seconds`. Each interval record includes `collection_id`,
`previous_collection_id`, exact string `counter_start`, `counter_end`,
`monotonic_start_ns`, `monotonic_end_ns`, numeric `interval_seconds`,
`rate_bytes_per_second`, RFC3339 `observed_at`, `received_at`, and `continuity`.
No file/process identity or probability is inferred. An immutable rule policy
object exposes all thresholds and timing parameters above.

Persistence exports:

```rust
pub async fn observe_collection(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    hb: &crate::cider_wire::Heartbeat, resource: &crate::cider_wire::Resource,
    collection: &crate::cider_wire::Collection, now_ms: i64)
    -> crate::error::ApiResult<()>;
pub async fn reconcile_sources(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    node_id: &str, boot_id: &str, generation: &str, session_id: &str,
    active_resources: &std::collections::BTreeMap<String, crate::cider_wire::Resource>,
    now_ms: i64) -> crate::error::ApiResult<()>;
pub async fn source_summaries(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    now_ms: i64) -> crate::error::ApiResult<Vec<serde_json::Value>>;
pub async fn rebaseline(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    source_id: &str, expected_revision: u64, reason: &str,
    actor_hash: &str, now_ms: i64) -> crate::error::ApiResult<serde_json::Value>;
pub async fn retain(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    now_ms: i64) -> crate::error::ApiResult<()>;
```

Tables: `detection_sources` has source/node/object IDs, active/admission status,
state JSON, updated time. `detection_findings` has finding/source/node/object IDs,
status, first_seen_at, updated_at, ended_at integer milliseconds and finding_json.
An audit table records administrator rebaseline actions. Final schema details
and any necessary interface refinements are announced to sibling workers before
dependent changes.

## Task 1: Pure detector and behavioral tests

**Files:** Create `server/src/detection.rs`, focused module tests or
`server/tests/detection.rs`. Root registers the module in `server/src/lib.rs`.
**Consumes:** Exact native observation interface above; no I/O.
**Produces:** Persistable state transitions and the typed summary/finding contract.

- [x] Write tests for baseline duration/weighted P95, exact wide-counter deltas,
  strict onset boundary, recovery hysteresis, frozen reference, candidate abort,
  missing/partial fields, duplicates/order, timing/identity discontinuity,
  serialization restart, and revision-preserving rebaseline watermark.
- [x] Run failing tests using a compiling minimal empty state-machine seam;
  record failure showing the behavior is absent, not a typo.
- [x] Implement deterministic transitions and bounds; return each finding change
  for transactionally persisting historical as well as active episodes.
- [x] Verify targeted tests and self-review.

Hand-derived acceptance example:

```text
counter0; 120 intervals * 5 seconds at 2,000,000 B/s -> ready, B=2e6 H=6e6 L=4e6
23 intervals * 5 seconds at 8,000,000 B/s -> pending, no finding
one more interval -> exactly one open finding
12 intervals * 5 seconds at 4,000,000 B/s -> resolved
```

## Task 2: Native ingestion and durable state

**Files:** Create `server/src/detection_store.rs`,
`server/migrations/003_detection.sql`, `server/tests/detection_ingestion.rs`;
modify `server/src/cider_api.rs`, `store.rs`, `workers.rs`.
**Consumes:** Task 1 public types and methods.
**Produces:** Persistence interface above, migration 3, atomic native evaluation.

- [x] Write failing transaction tests using actual SQLite and schema-2 fixtures
  for replay/rollback, empty failed acquisition, direction-specific failure,
  restart/recovery, identity removal, rebaseline revision guard and audit.
- [x] Add transactional migration and update the version ceiling/runner to 3.
- [x] Map every newly projected native collection attempt into independent read
  and write observations; reject stale/timing-unknown data for training without
  weakening ingestion. Use only the current accepted resource identity.
- [x] Persist source and emitted finding changes in the existing transaction.
  Reconcile successful removals/generation changes without treating a failed
  inventory refresh as disappearance. Bound active admission and state sizes,
  expose excluded known sources, and retain unresolved evidence.
- [x] Verify targeted tests and record schema/DTO details for the API owner.

## Task 3: Finding/status reads and administrator rebaseline

**Files:** Root owns `server/src/read_api.rs`, `api.rs`, `lib.rs`, new
`server/src/detection_api.rs` if useful, and `server/tests/detection_api.rs`.
**Consumes:** Task 2 persistence and findings tables.
**Produces:** Three read routes and one mutation from the approved spec.

- [x] Add boundary tests with real router/SQLite for 200/400/401/403/404,
  stale revision 409, replay refusal, cursor expiry 410, and frozen pages.
- [x] Register `/api/v1/findings`, `/api/v1/findings/{finding_id}` and
  `/api/v1/detectors/storage-activity/sources`; reuse existing read auth/budgets,
  filtering/envelope and bounded pagination instead of a parallel auth system.
- [x] Implement administrator-only POST rebaseline with strict body fields,
  UUID/timestamp transport headers, revision guard, and atomic audit/change log.
- [x] Expose policy in source-page meta, exact DTO fields, coverage and narrow
  capabilities. Only current open findings support a security warning; quiet or
  unavailable coverage never establishes healthy security.
- [x] Verify read filtering, source/finding ownership, and old endpoints unchanged.

## Task 4: Security view and frontend behavior

**Files:** Create `web/js/security.js`, `security-views.js`,
`web/tests/security.test.mjs`; modify `app.js`, `session.js`, `views.js`,
`data.js`, and `web/css/live.css`. Root updates embedded-asset registration.
**Consumes:** Fixed source/finding JSON above and the read routes in Task 3.
**Produces:** Read-only source coverage, readiness, evidence and collector events.

- [x] Write and run failing behavior tests for unknown/learning/ready states,
  frozen evidence, exact counters, invalid/late responses, status/finding errors,
  independent reads and keeping collector events distinct.
- [x] Fetch findings and source pages independently from the core poll using the
  existing client/cursor patterns. Avoid extra fetches per displayed source.
- [x] Render readiness and policy, dated generated findings and expandable
  evidence, with node/source links. Source object details may use the implemented
  object route; do not invent a disk association. Viewer exposes no admin action.
- [x] Keep pagination errors and retry/expiry states visible. Do not label
  empty findings or stale observations safe/healthy. Escape data via Preact.
- [x] Run web tests and syntax checks, then report files and DTO assumptions.

## Task 5: Integration, review, and runtime verification

**Files:** Focused test/harness additions under `server/tests/`, `scripts/`, or
ignored `.codex-staging/`; corrections remain with owning implementation files.

- [x] Independently review each task's spec compliance and code quality after
  targeted checks. Fix confirmed issues with regression cases.
- [x] Run full workspace tests/check, headless check, web suite/syntax, whitespace,
  then build workspace to embed final assets.
- [x] Exercise synthetic protocol inputs against disposable SQLite/router using
  injected time for 600/120/60-second transitions; no shortened production policy.
- [x] Run verified-TLS native admission/learning smoke in isolated state.
- [x] Complete production-assets synthetic finding browser interactions at desktop
  and actual 390x844 sizes; record synthetic fixture limits.
- [x] Inspect auth/paging through automated tests and browser loss/recovery with
  one-page snapshot controls. Stop only
  diagnostic processes created for this work; do not operate old user sessions.
- [x] Broad final review and focused fix verification; record any remaining
  limitations without claiming measured detection efficacy.

## Task 6: Shared contract and delivery evidence

**Files:** Update root/web README and a durable feature evidence record; shared
Server Spec section 20 using native Google Docs formatting.

- [x] Prepare complete final request/response schemas and parseable synthetic
  examples for all four routes, auth roles, statuses, caps, retention, lifecycle,
  policy and available/unavailable capabilities.
- [x] Read the live Server Spec, preserve existing sections, append the final
  contract using native headings/tables/code formatting, and verify Saved to
  Drive plus an exported comparison. Record native UI evidence honestly.
- [x] Update approval/implementation/verification status and this plan's task
  checkboxes from actual evidence. Report files, tests, remaining limitations,
  and any shared-doc access blocker. No release or commit action is implied.

## Execution decisions

Work in the user's current checkout on `codex/storage-activity-detection` after
the requested clean checkpoint. New files have disjoint owners, so the pure
detector, persistence preparation, and frontend may proceed in parallel while
root integrates APIs. Dependent compile/test steps wait for interfaces; no
agent commits or edits another owner's files. This uses the developer-authorized
parallel delegation workflow. The approved design is authoritative; record
necessary refinements in the execution ledger and keep working.

## Current handoff

Implementation and independent code review are complete. Full workspace tests
passed (188 tests; three helper entrypoints intentionally ignored), workspace and
headless checks passed, and the real verified-TLS smoke admitted six native
driver-direction sources into learning and verified stale status after stopping
its collector. Final web suite: 97 passed. Final embedded-asset rebuild is recorded
in the evidence report.

Desktop and actual 390x844 browser acceptance passed using production assets and
clearly synthetic observations. A capacity-limited admission label was corrected,
regression tested and rechecked in both layouts. The one-page fixture verified
disabled end-of-snapshot controls; multi-page/auth semantics remain automated-test
evidence. Native Google Docs reported Saved to Drive for section 20. A fresh
export preserved all 1,536 prior nonempty paragraphs, matched all six JSON
examples, and verified native heading/table/code formatting. No connector write
or connector sign-off is claimed.
The current branch is codex/storage-activity-detection; feature changes remain
uncommitted. The earlier requested checkpoint is 5a496f1.

All diagnostic processes started for this detection work were stopped. The older
user-owned storage-browser.mjs session (parent PID 88103, port 54573) was observed
running and left alone. Temporary QA tabs were closed and emulation cleared.
