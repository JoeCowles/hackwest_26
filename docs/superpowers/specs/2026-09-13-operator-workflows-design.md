# Operator workflows design

The user approved implementation of evaluation priorities 1, 2, 3 (NFS user
quotas), and 5, and MIT licensing. Work extends the current uncommitted detection
and reliability implementation. No commit, push, production deployment, automatic
remediation, or unsolicited live SMS is part of this implementation run.

## Architecture and shared constraints

ciderd remains the only node collector. Five-second heartbeats are unchanged.
The central server persists attention episodes, evaluates observations, exposes
authenticated reads, and sends configured notifications outside database locks.
All new reads reuse the existing viewer/admin authentication and read budgets.
Mutation routes use the existing admin authenticator and replay protection.
The browser has a separate explicit admin-action credential entry, held only in
memory and cleared after the action; viewer credentials never gain write scope.
Stable evidence pagination and live discovery are separate concerns.

Unknown, unavailable, stale, partial, unsupported and unconfigured remain
distinct. Missing evidence cannot resolve episodes or supply zero activity.
No APFS/NFS capacity double counting. Never attribute driver throughput to each
filesystem. Three days is a planning target, not a failure prediction promise.

## Attention and Twilio

Persist a unified attention queue for existing activity/reliability findings,
capacity pressure, node observation loss, and passive filesystem failure evidence.
Rows preserve the originating finding/evidence, current observation state,
severity, timestamps, acknowledgement and acknowledgement note. Acknowledgement
does not resolve the source condition or relearn any detector. New severity or a
new episode is independently actionable. Live reads order unresolved concerns
first; an auto-refreshing summary exposes new issues while detail pages stay frozen.

A SQLite outbox deduplicates issue transitions, survives restart, and tracks
queued, sending, accepted, delivered, failed and uncertain outcomes. The sender
uses Twilio's HTTPS Messages API from Rust, bounded requests and response sizes,
no redirects, no credential logging. HTTP acceptance is not handset delivery.
Poll returned message SIDs for delivery status so no public callback endpoint is
required. Retry only explicit retryable rejections; ambiguous transport outcomes
are marked uncertain to avoid silently generating duplicate SMS. Crash recovery
of an in-flight request is also uncertain. Disabled/missing configuration never
looks delivered. Bound retries, sending rate, queue size and retention.

Configuration loads a private .env on startup. TWILIO_ACCOUNT_SID plus
TWILIO_SECRET is the primary account authentication pair supplied by the user.
Also support TWILIO_SID as an account alias or SK-prefixed API key (the latter
requires TWILIO_ACCOUNT_SID). Sender may default from TWILIO_FROM. Recipient,
sender and enabled state are configurable through an administrator-only
dashboard action backed by private SQLite settings with an expected revision.
No browser receives credentials. Viewer reads show only masked phone numbers and
configuration/delivery state. Settings default disabled; enabling requires valid
E.164 sender and recipient. Secrets and unmasked phone numbers never enter logs.
Tests use local HTTP protocol fixtures; live sending requires a concrete
user-authorized target. .env and .env.* are already ignored; preserve those rules.

## History and capacity runway

Expose object metric history from existing raw samples and 5-minute/hourly rollups,
with strict time windows, bounded rows, series identity, state and coverage.
Exact raw integers remain strings. Never merge different sources/labels/boot or
inventory generations. Gaps remain visible. The UI provides a dated chart and
evidence table and does not imply unavailable data has been backfilled.

Capacity forecast applies to one eligible capacity-bearing object, avoiding
cluster-level aggregation across changing contributors. Use compatible, fresh
used/total history with unchanged capacity and identity. Require at least eight
distinct usable samples spanning one hour; reject long gaps and insufficient or
unstable trends. Report positive growth, estimated exhaustion, a scenario range,
observation span/count, assumptions and uncertainty. Flat/decreasing, stale,
ambiguous, changed-capacity, or inadequate data returns explicit reasons and null
dates. Forecasts are capacity exhaustion only, never drive-failure predictions.

## NFS user quotas

Choose classic read-only ONC RPC rquota v1 GETQUOTA from configured NFS servers.
Each configuration identifies server, export path, UID, optional display label,
and optional fixed rquotad port; portmapper lookup is read-only and bounded.
Use the process's real AUTH_SYS identity, never impersonate another query user.
The target UID is a query argument; access is decided by the server. Restrict
configuration size and query concurrency. No export discovery, account scanning,
quota mutation, or public network probing. Transport is trusted-network ONC RPC,
with its unauthenticated-server limitation clearly documented.

Run queries in isolated ciderd workers, preserving timeout/permission/no-quota/
unsupported distinctions. Publish per-export/per-UID quota resources containing
used bytes, block/inode soft/hard limits, grace information and observation state.
Normalize block units with checked exact arithmetic. Zero limits mean no limit
only when an available server response explicitly reports them. No quota or a
failed query is unknown. Display configured user quota rows separately from APFS
volume quotas. A test NFS server in Docker/Podman is authorized; use a dedicated
container/volume and never change host exports or mount existing user data.

## Diagnostics and filesystem evidence

Expose per-disk SMART and IOKit readiness, including disabled, waiting,
unsupported, failed and stale checks. Derive passive filesystem concerns from
explicit NFS dead/not-responding and failed current capacity/mount attempts,
preserving source dates and attribution. Accessibility evidence is not corruption
proof. Do not treat a legitimate read-only mount as an error. Existing native
collector state is the source; no active scan/repair/self-test is introduced.
Overall assessment must report incomplete coverage independently of warning
severity and must not claim full health solely from node availability.

## Interfaces and ownership

Root owns server read/mutation integration, schema registration, main/worker
wiring, frontend integration, shared documentation and license. Worker modules
return JSON data/metadata through documented functions; root supplies envelopes,
auth, read limits and frozen paging. Only root edits server/src/read_api.rs,
server/src/api.rs, server/src/store.rs, server/src/main.rs, server/src/workers.rs,
server/src/lib.rs and shared web app/session/views files.

## Verification

Use focused behavior tests with an observed failing phase, then passing phase.
Test idempotence/restart, unavailable evidence, authorization, malformed protocol
responses, wide quota values, rate limits, delivery ambiguity, identity/gap
history boundaries, forecast rejection and uncertainty, and browser age/refresh
behavior. Run workspace tests/check, headless check, embedded-assets build and
all web tests. Verify rendered desktop/mobile operator views using production
assets. Record real NFS and Twilio validation separately from local mocks.
Publish full route/auth/field/status/example contracts in the shared Server Spec
through available native Google Docs UI and verify an export; do not call this a
connector write. MIT applies to project code; preserve all third-party notices.
