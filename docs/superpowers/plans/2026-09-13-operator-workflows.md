# Operator Workflows Implementation Plan

> **For agentic workers:** Use superpowers:subagent-driven-development. Each task
> below is independently owned; root performs integration and task/final review.

**Goal:** Make Orchard's observations actionable with attention/SMS, history and
capacity runway, NFS user quotas, diagnostics readiness and MIT licensing.

**Architecture:** Preserve the existing collector/receiver split. New pure or
persistent server modules feed shared authenticated reads and the web console.

**Tech Stack:** Rust, Axum, SQLite/sqlx, reqwest, native isolated ciderd workers,
ONC RPC/XDR, Preact, Node tests, Docker/Podman test server.

**Spec:** ../specs/2026-09-13-operator-workflows-design.md

## Global constraints

- Five-second heartbeats, bounded resources, exact counter identities and honest
  observation states remain unchanged.
- Existing uncommitted work is the baseline; no commits or pushes in this run.
- Root owns shared server/router/runtime and shared frontend files.
- Raw telemetry/secrets do not go into shared documentation.
- The shared Server Spec must document new APIs and verification limits.

## Task 1: Attention and Twilio

- [ ] Add behavioral tests for episode deduplication/recovery/acknowledgement,
  retries/uncertain delivery, restart and configuration; observe failures.
- [ ] Implement server/src/attention.rs, server/src/notifications.rs,
  server/migrations/005_attention.sql and focused tests.
- [ ] Expose these integration interfaces (root supplies HTTP/auth):
  ```rust
  attention::reconcile(state: &AppState, now: i64) -> ApiResult<()>;
  attention::list(state: &AppState, query: &BTreeMap<String,String>, now: i64)
      -> ApiResult<(Value, Value)>;
  attention::summary(state: &AppState, now: i64) -> ApiResult<Value>;
  attention::acknowledge(tx: &mut Transaction<'_, Sqlite>, id: &str,
      revision: &str, note: &str, actor: &str, now: i64) -> ApiResult<Value>;
  notifications::settings(state: &AppState) -> ApiResult<Value>;
  notifications::configure(tx: &mut Transaction<'_, Sqlite>, request: &Value,
      actor: &str, now: i64) -> ApiResult<Value>;
  notifications::run(state: AppState, stop: CancellationToken);
  ```
- [ ] Notification configuration is loaded once by the worker; status persisted
  without secrets and exposed through attention summary. Sender never holds DB
  transaction across HTTP. Root invokes reconcile every five seconds.
- [ ] Credentials are TWILIO_ACCOUNT_SID plus TWILIO_SECRET from private .env.
  Persist sender/recipient/enabled separately through admin settings; viewer
  settings responses mask phone numbers. Default disabled; changes use expected
  revision, and enablement requires E.164 sender/recipient. Root builds dashboard
  settings and wires GET/PUT /api/v1/notifications/settings with admin auth for PUT.
- [ ] Record full payload examples and validation in docs/operator-attention.md.
- [ ] Run focused tests and return report; root reviews and integrates.

## Task 2: Metric history and capacity runway

- [ ] Add tests proving series isolation, raw precision, rollup state/coverage,
  bounded selection and forecast rejection before implementing behavior.
- [ ] Implement server/src/history.rs with interfaces:
  ```rust
  history::read(state: &AppState, object_id: &str,
      query: &BTreeMap<String,String>, now: i64) -> ApiResult<(Value,Value)>;
  history::forecast(state: &AppState, object_id: &str, now: i64)
      -> ApiResult<Value>;
  ```
- [ ] GET /api/v1/objects/{object_id}/history uses metric, from, to and
  resolution (raw/5m/1h) parameters; max 2000 points, reject rather than silently
  truncate. GET /api/v1/objects/{object_id}/capacity-forecast has no parameters.
- [ ] Keep history series separate and test linear/nonlinear/no-growth histories,
  capacity changes, old owners, wide integers, gaps and insufficient data.
- [ ] Document source/retention/forecast assumptions in docs/metric-history.md.
- [ ] Run focused tests and return report; root reviews and integrates.

## Task 3: NFS quota collection and real server fixture

- [ ] Add rquota protocol/config/collector tests before implementation.
- [ ] Implement configured read-only rquota client, isolated worker scheduling,
  quota resources/metrics in ciderd, schemas/catalog changes, and fixtures.
- [ ] Provide server/src/quota.rs with:
  ```rust
  quota::list(state: &AppState, query: &BTreeMap<String,String>, now: i64)
      -> ApiResult<(Value,Value)>;
  ```
- [ ] GET /api/v1/quotas accepts node_id, uid, limit and cursor. Root supplies
  authentication/pagination and integrates any required aliases/resource maps.
- [ ] Provision isolated Docker/Podman NFS+rquotad test server; prove actual user
  usage/limits, failed/denied responses, distinct users, and NFS source linkage.
- [ ] Do not alter host exports or unrelated containers/volumes. Remove test
  infrastructure after recording results unless needed for final browser QA.
- [ ] Document config/server setup and exact verification in docs/nfs-user-quotas.md.

## Task 4: Integration, diagnostics, UI, MIT and shared spec

- [ ] Add failing API tests for new routes/auth/acknowledgement and unknown states.
- [ ] Register module/migration 5; integrate attention every five seconds and
  notification background task. Reuse read budgets/envelopes and admin replay
  protection. Add routes and actual capabilities.
- [ ] Add passive filesystem evidence and per-disk diagnostics-readiness module
  and tests. Correct overall health completeness semantics with regression tests.
- [ ] Add operator views and client tests: refreshed unresolved attention summary,
  stable evidence pages, explicit admin acknowledgement, object history/runway,
  configured NFS user quotas, readiness/source failure context. No secret storage.
- [ ] Add MIT LICENSE and package metadata, preserving third-party licenses.
- [ ] Run all required checks and desktop/mobile rendered QA. Review every task
  and the full new diff against the preserved baseline; fix material findings.
- [ ] Update native shared Server Spec with all routes/auth/fields/statuses and
  examples; export and verify. Update public README/runbooks with final evidence.

## Execution rulings and recovery

The user's explicit implementation request approves these features; no repeated
design approval is needed. Work stays in the existing feature checkout because
new subsystems depend on its uncommitted code. A complete source baseline is
saved under .codex-staging/operator-workflows-baseline. Independent implementers
may work in parallel with exclusive file ownership; shared edits are serialized
by root. This overrides skill defaults that would commit, repeatedly request
approval or prohibit useful independent parallel work. Progress and reports live
under .codex-staging/operator-workflows/.
