# Drive Reliability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Execute all approved tasks without another permission checkpoint; do not commit or deploy.

**Goal:** Persist and explain passive drive failure/deterioration evidence.
**Architecture:** Extend SMART evidence, evaluate pure rules within accepted native ingestion, and project durable source findings onto verified physical disks. Keep reliability separate from the existing security detector.
**Tech Stack:** Rust, ciderd wire model, Axum, SQLite/sqlx, Preact ES modules.
**Spec:** docs/superpowers/specs/2026-09-13-drive-reliability-design.md

## Global Constraints

macOS native collection stays in ciderd. Five-second heartbeats/I/O; optional
300-second SMART. Exact counters and observation/identity boundaries persist.
No active scan/self-test/write probe, remediation, SMS, commit, push or package.
Preserve the existing uncommitted security feature and the baseline snapshot.
All new APIs require shared Server Spec publication and qualified evidence.

## Task 1: SMART evidence and continuity

Files: ciderd SMART parser, metric catalog, scheduler only for admitted locator
context, collector fixtures/tests and collector guide. Own no server/web files.

Interface: existing wire Metric with ATA names/spec semantics. SMART reading
extensions carry `device_identity` (private fixed-size hash),
`device_identity_confidence`, and fixed reported-status provenance. Gauges and
counters carry the same marker. Maintain existing parse_smart compatibility;
introduce a locator-aware entry point only if the scheduler needs one.

- [ ] Add behavior tests to existing parse_smart surface first: hand-written ATA
  IDs 5/197/198 output exact gauges, absence is absent, unsupported packed values
  are not interpreted, duplicate rows cannot become a valid measurement.
- [ ] Run `bash scripts/cargo-local.sh test -p ciderd --test collectors --locked`
  and record failures caused by missing ATA metrics/identity provenance.
- [ ] Extend catalog and parser with strict shape/value/name validation; add
  admitted locator validation and private identity markers without raw secrets.
- [ ] Verify health exit bits preserve parsed health conditions and tool errors
  remain acquisition errors; test replacement marker change and missing identity.
- [ ] Run focused collector/contract tests and submit the scoped diff for review.

## Task 2: Pure reliability engine

Files: new server/src/reliability.rs and server/tests/reliability_rules.rs.
Root owns registration in lib.rs. No persistence, API or collector edits.

Interfaces (public, serde Serialize/Deserialize for persisted types):
`SourceIdentity { source_id,node_id,object_id,resource_id,collector,scope: String }`;
`Observation` carries collection/boot/generation/session/clock/source-generation/
source-version/adapter identifiers, finished_monotonic_ns: Decimal,
observed_at: String, received_at_ms: i64, age_at_receipt_seconds: f64,
stale_after_seconds: f64, status: String and metrics: Vec<cider_wire::Metric>.
`SourceState::new(SourceIdentity) -> Self`;
`SourceState::observe(Observation) -> Vec<Finding>`;
`SourceState::interrupt(reason: &str, now_ms: i64) -> Vec<Finding>`;
`SourceState::summary(now_ms: i64, owner_online: bool) -> serde_json::Value`.
SourceState exposes source and active. `Policy::default()` is serializable.
Finding exposes finding_id/source_id/node_id/object_id/resource_id/rule_id,
dimension/classification/severity/status/summary/policy_version,
first_seen_at/last_seen_at/opened_at/updated_at/ended_at/reason/evidence.
All times are UTC strings, ended_at/reason nullable, evidence is JSON retaining
exact values. Signal summaries expose rule_id, dimension, state, severity,
reason, finding_id, observation and evidence; observation contains state,
age_seconds, stale_after_seconds, observed_at/received_at.

- [ ] Create test targets with minimal public stubs only as required to compile.
  Assert literals: no finding on initial lifetime errors=7; next same-epoch
  errors=8 yields a finding containing delta="1". Run and record RED.
- [ ] Implement direct state and counter rules from the spec; test missing
  telemetry never clears, two new clear acquisitions resolve, epoch changes
  interrupt, duplicate/nonadvancing observations do nothing.
- [ ] Implement workload buckets and bounded duration-weighted baselines. Feed
  600 seconds of comparable 1 ms/op data, then >4 ms/op for 120 seconds;
  assert onset at the measured boundary and recovery after 60 usable seconds.
- [ ] Test mismatched workload, idle/zero timing, clock/skew, partial readings,
  frozen references, restart serialization and bounded buffers.
- [ ] Run focused rule tests and request review against the spec.

## Task 3: Persistence, reads and physical attribution

Root owns new reliability_store.rs, migration 004, lib.rs/store/workers/cider_api/
read_api integration, and API/persistence tests. Reuse established transaction,
role, frozen-page and disk-index helpers rather than duplicating ingestion.

- [ ] Add failing HTTP test: viewer GET reliability/sources returns data and
  coverage; node-role denied, unknown query rejected. Run RED before route edit.
- [ ] Add migration, source table bounded state/summary, finding table and indexes.
  `observe_collection(tx,hb,resource,collection,now)` builds Observation and calls
  the pure engine only for native IOKit/SMART. `reconcile_sources` handles removal
  and new boot/session. Publish all changes in the receipt transaction.
- [ ] Add read helpers returning compact summaries with read-time freshness,
  bounded source selection and durable findings. Retention keeps open findings.
- [ ] Register three approved routes; use existing read authorization/paging.
  Extend disk summaries with the exact reliability JSON shape in the spec.
  Preserve source scope and require current unique topology for IOKit/disk joins.
- [ ] Test actual accepted schema-2 heartbeat replay/rollback, failed collection,
  positive counter transition, SMART state, reopen/restart, source removal,
  first historical totals, schema upgrade and frozen cursor ownership/expiry.
- [ ] Test core warning propagation, unresolved association, stale topology,
  unsupported/no sources and retained finding evidence with unknown currentness.

## Task 4: Disk reliability evidence UI

Files: new web/js/reliability.js and reliability-views.js; storage-views.js,
live.css; new web/tests/reliability.test.mjs. Root embeds new assets in read_api.
Consume the spec disk.reliability object; no additional polling/mutation needed.

- [ ] Write model/render behavior tests with literal disk reliability fixtures:
  exact large error counters, clear/current vs partial/unknown, stale evidence,
  source links and missing forecast. Run RED.
- [ ] Implement pure aging/model helper and ReliabilityPanel. Include a source
  inventory/readiness section and expandable exact finding evidence with rule,
  dates, scope and confidence. No fabricated zeroes or blanket healthy label.
- [ ] Mount panel in existing disk page, use snapshotAgeMs/current and inherited
  topology matching. Preserve retained dated evidence through refresh failures.
- [ ] Verify new and existing web tests/syntax. Render desktop/mobile with
  production assets and clearly synthetic failure/unknown/degradation cases.

## Task 5: Review, contract and native verification

- [ ] Review each completed task and the integrated feature against source-only
  baseline; fix actionable findings and verify scoped regressions.
- [ ] Run cargo test/check workspace, headless check/build, desktop build,
  all web tests and JS syntax, contract validator and diff whitespace checks.
- [ ] Build before passive verified-TLS smoke. Use disposable owned state,
  real ciderd, two heartbeats and reliability reads; stop only owned processes.
- [ ] Publish full route/auth/fields/status/examples and implementation limits
  in shared Server Spec using available native Docs UI if no connector exists.
  Verify save/export and distinguish native evidence from connector sign-off.
- [ ] Record actual test/render/smoke results in docs/drive-reliability.md and
  update README schema/count claims. Preserve all uncommitted changes.
