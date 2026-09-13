# USB demo alerts implementation plan

> **For agentic workers:** Use the task ownership and review boundaries below.

**Goal:** Implement observed USB disappearance and negotiated-link degradation
concerns that reach the existing attention/SMS pipeline.

**Architecture:** A bounded native IOKit snapshot supplies explicit presence and
negotiated link evidence; a persistent server state machine evaluates identity-
scoped transitions and exposes readiness in the existing disk view.

**Tech Stack:** Rust, SDK-owned macOS C/IOKit, SQLite, JavaScript/Preact.

**Spec:** `docs/superpowers/specs/2026-09-13-usb-demo-alerts-design.md`

## Global constraints

- Heartbeat and I/O default stay at five seconds.
- Central server performs no native collection.
- Missing, partial, failed, stale and unsupported observations remain explicit.
- No real SMS, commit, push, package, deployment, or raw telemetry publication.
- Preserve existing driver counter identities, exact integer values and wire replay.
- USB link warnings are transport observations, not media-health forecasts.
- Shared spec updates and final verification are required, with their limits stated.

## Task 1: Native USB connection snapshots

Owner: collector agent. Files: new `crates/ciderd/src/device_snapshot.rs`;
`lib.rs`, `collectors/mod.rs`, `collectors/inventory.rs`, `scheduler.rs`,
`platform/macos/native.c`; focused collector/contract/scheduler tests.

- [x] Add failing tests for snapshot propagation, complete empty enumeration,
      enum mapping, malformed optional USB evidence preserving counters, scoped
      identity continuity/collision, and heartbeat serial redaction.
- [x] Implement the exact public structs in the spec and bounded validation.
- [x] Implement bounded native USB ancestry and explicit lookup states.
- [x] Publish the typed snapshot on the host collection, including empty scans.
- [x] Run focused collector tests and a bounded native snapshot (redacted checks).

## Task 2: Durable connection-watch policy

Owner: server agent. Files: new `server/src/device_watch.rs`,
`server/migrations/007_device_watch.sql`, `store.rs`, `lib.rs`, `cider_api.rs`,
`attention.rs`, `read_api.rs`, new `server/tests/device_watch.rs`.

Consumes `ciderd::device_snapshot::{DeviceSnapshot, UsbDevice}` and host extension
`usb_device_snapshot`. Produces ingestion, attention-reconciliation and disk
projection functions with signatures selected locally and reported to root.
Disk field and projection keys must match the spec exactly.

- [x] Write failing tests for two-observation transitions and stale/replay/context
      breaks; verify existing source cannot satisfy the new behavior.
- [x] Implement migration, bounded persistence, transactional ingestion and
      independent watch conditions through existing outbox machinery.
- [x] Project current uniquely associated watches onto disk summaries.
- [x] Test reconnect baseline continuity, absence, weak identity, collisions,
      recovery, restart, admission bounds and source freshness.
- [x] Add authenticated wire-level tests of receipt/collection replay and outbox
      admission without launching a real provider worker.

## Task 3: Connection evidence in existing disk UI

Owner: frontend agent. Files: new or existing `web/js` helper, `storage-views.js`,
optional `web/css/live.css`, `web/tests/storage.test.mjs` or dedicated tests.
Consumes `disk.device_watch` exactly as specified. Keep Attention generic.

- [x] Write failing render/aging tests independent of I/O readiness.
- [x] Render present/baseline/link-warning/unsupported/unknown with exact bitrates,
      source age, identity scope and expandable evidence.
- [x] Failed refreshes/sleep cannot keep current labels indefinitely.
- [x] Run all web tests and syntax checks.

## Task 4: Integration review, documentation and acceptance

Owner: root. Review each implementation and fix concrete cross-component gaps.

- [x] Verify actual native worker output carries a complete empty/nonempty USB
      snapshot without raw identity publication; no write workloads.
- [x] Run workspace tests and checks; build embedded web assets.
- [x] Use synthetic production-asset browser examples for disappearance and slower
      reconnection, and verify loopback outbox behavior with no actual SMS.
- [x] Update README and local USB demo runbook with config, timing, identity and
      hardware limits. Record exact passed checks and remaining real hardware gate.
- [x] Update shared Server Spec natively and verify saved export where available;
      distinguish native UI publication from unavailable connector write.
- [x] Run final independent review and record verified results and remaining limits.

Baseline: clean `8da81b3`; immediately preceding assessment ran 315 Rust tests
(three ignored helpers) and 118 web tests successfully. No source changed between
that baseline and branch creation. Implementation and verification completed locally on `codex/usb-demo-alerts`; changes remain uncommitted.

Final verification: 345 workspace Rust tests, 128 web tests, workspace/default and headless checks, rebuilt binaries, static contract validation and verified-TLS synthetic smoke passed. The explicit native USB probe also passed. Physical unplug, 5 Gb/s to 480 Mb/s reconnect, and restoration to 5 Gb/s passed on the same reported enclosure identity. All temporary rehearsal processes and private state were cleaned up. Shared Server Spec section 23 was saved natively and export-verified, preserving 2,592 preceding paragraphs and matching 34 new paragraphs with native heading styles. No real SMS, commit, push, package or deployment was performed.
