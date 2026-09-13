# 22. Operator workflows HTTP contract

Implemented scope: attention, administrator acknowledgement/SMS configuration, stored metric history, capacity-exhaustion scenarios, configured NFS user quotas, and passive diagnostics. Orchard observes and notifies; no repair, isolation, quota mutation, or hardware-failure prediction is introduced. Heartbeats remain five seconds. This section supplements the measurement/collector contracts in Server Spec sections 17 and 19.

## 22.1 Transport, authentication, envelopes, errors

All routes below are same-origin JSON. GET requires `Authorization: Bearer` with an unexpired viewer or administrator credential; active node credentials receive 403. Mutations require an administrator Bearer, `Content-Type: application/json`, fresh UUID `X-Request-ID`, and RFC3339 `X-Request-Timestamp` within 300 seconds of server time. Viewer/node credentials cannot mutate. Maximum JSON body: 1,048,576 bytes. Transport request IDs and expected object revisions serve different purposes.

Successful responses are 200:

```json
{"data":[],"meta":{"api_version":"1","server_time":"2026-09-13T12:00:00.000Z","request_id":"00000000-0000-4000-8000-000000000001","snapshot_cursor":"42","next_cursor":null}}
```

Dates are RFC3339; IDs/revisions are strings; exact measurement byte/count integers are decimal strings; coverage/count metadata are numbers. Nullable values remain unknown. Common measurements retain value, kind, unit, state, source, scope, observed_at, received_at, age_seconds, boot_id, inventory_generation, labels, ciderd acquisition metadata, derived_rate_per_second, and derivation_state (sections 17/19). Missing values are never zero. Examples below show data projections; field lists specify complete objects.

Attention/quota/diagnostic lists accept `limit=1..500` (default 100) and opaque `cursor`. Keep filters and limit unchanged through a traversal. Pages freeze data and metadata for 300 seconds; refresh the initial route separately for new observations. Read budget: 120/minute, burst 20; page cache: 128 traversals/32 MiB. Responses use no-store.

Errors have `error:{code,message,fields:[{field,message}],request_id}`. Statuses: 400 invalid_query/invalid_payload/history_too_large; 401 missing/invalid/expired credential; 403 wrong credential scope; 404 not_found; 409 conflict or request_replayed; 410 cursor_expired; 413 payload_too_large; 429 rate_limited/backpressure (Retry-After: 1); 503 bounded-selection unavailable/attention_limit/source_limit; 500 internal_error. Unknown/repeated read parameters, malformed UUIDs and unsupported enums are rejected. Unavailable measurements normally remain 200 data with explicit states.

## 22.2 Attention and acknowledgement

`GET /api/v1/attention` optionally filters node_id, object_id, status=open|resolved|all (default all), kind=activity|reliability|capacity|node_loss|filesystem|drive_removal, severity=info|warning|critical, and acknowledged=true|false. Open concerns sort first, then critical/warning/info, newest updated_at, descending ID. Selection is capped at 10,000 rows/32 MiB stored evidence and summaries; oversized selection returns 503. Budget, rows and joins use one SQLite read snapshot. Metadata adds matched, truncated=false, maximum_episodes, and reconciliation.

Each row contains id, source_key, kind, node_id, nullable object_id, status, severity, observation_state, reconciliation_state, summary, evidence, revision, first_seen_at, updated_at, nullable resolved_at, acknowledgement, notification_intent, and notification. Acknowledgement is null or `{at,note,actor_hash}`. Notification is null or `{state,attempts,provider_status,reason,updated_at}`; absence does not imply delivery. Notification intent is null or `{state,transition_revision,settings_revision,created_at,updated_at,reason}`. Pending/terminal intents also project into notification with zero attempts; dates are RFC3339.

```json
{"kind":"capacity","status":"open","severity":"critical","observation_state":"ok","reconciliation_state":"current","summary":"Observed capacity pressure","acknowledgement":null,"notification":{"state":"accepted","attempts":1,"provider_status":"queued","reason":null,"updated_at":"2026-09-13T12:00:05.000Z"}}
```

`GET /api/v1/attention/summary` accepts no parameters. Data contains total, open, unacknowledged, notifications (settings below), delivery (retained outbox counts by state plus pending_admission intents), observed_at (response time), and reconciliation: `{state,last_successful_at,last_attempted_at,error,stale_after_seconds:15}`. Reconciliation is unknown before first success, current within 15 seconds, stale afterwards/clock rollback, or failed after a subsequent failed attempt. Error is null or reconciliation_failed. Source observation_state remains independent; reading does not refresh reconciliation.

`POST /api/v1/attention/{episode_id}/acknowledgement` requires:

```json
{"expected_revision":"revision-from-read","note":"Investigating the reported condition"}
```

It returns the updated episode/new revision (without the list-only notification join). Note is required, may be empty, and is limited to 2,048 bytes; unknown fields fail. Missing episode: 404; stale revision/already resolved: 409. Acknowledgement suppresses queued SMS and pending admission, not the source condition, detector baseline, or an in-flight send.

Repeated observations retain an episode; fresh recovery resolves it; recurrence creates another ID. Fresh severity transitions clear acknowledgement. Missing/stale/interrupted observations cannot resolve. Shared-filesystem observer changes preserve identity and update attribution. Capacity thresholds are 90% warning/95% critical with coherent physical/shared observers. Node loss follows goodbye/revocation or 90 seconds without heartbeat; recovery requires nonnegative age under 30 seconds. Future heartbeat timestamps are unknown; clock rollback cannot select a resolved episode over an active one. Observation loss does not establish disk failure.

## 22.3 SMS configuration and delivery

`GET /api/v1/notifications/settings` accepts no parameters. `PUT /api/v1/notifications/settings` requires expected_revision and boolean enabled; optional sender/recipient are full E.164 strings (+ and 2–15 digits, first digit nonzero). Omission preserves a phone; empty clears only while disabled. Both are required when enabling. Unknown fields fail; stale revision is 409. Both routes return:

```json
{"enabled":false,"sender":"***0100","recipient":"***0101","revision":"settings-revision","credential_state":"configured","worker_seen_at":"2026-09-13T12:00:00.000Z","admission_limited":false,"updated_at":"2026-09-13T12:00:00.000Z"}
```

Phones expose last four digits or null. Never round-trip masks. Example PUT:

```json
{"expected_revision":"initial","enabled":true,"sender":"+15555550100","recipient":"+15555550101"}
```

Defaults are disabled. Credentials load once from environment/private .env: TWILIO_ACCOUNT_SID plus TWILIO_SECRET; TWILIO_SID supports an AC alias or SK key requiring Account SID. TWILIO_FROM may initialize the sender. credential_state is not_loaded/configured/missing_or_invalid; configured proves syntax, not provider authentication. Credentials never enter SQLite/browser responses. Dashboard settings are persisted privately; changes suppress old queued and pending settings revisions. Enabling covers future episode/severity transitions, not disabled backlog.

Outbox states: queued, sending, accepted, delivered, failed, uncertain, suppressed. Accepted is not handset delivery. HTTPS Twilio Messages POST/GET uses Basic auth, no redirects, five-second connect/ten-second request deadlines, and 16 KiB responses. Only explicit send HTTP 429 retries (60/120/240 seconds; four attempts). Ambiguous transport/success, 5xx, or interrupted sends are uncertain and never automatically resent. Explicit terminal rejection fails.

Claims commit before HTTP, releasing database locks. If outcome persistence fails, the next serial step marks the orphan claim uncertain/outcome_not_recorded without resending. A separate persisted send timestamp allows one send/30 seconds; due sends get priority, other five-second cycles poll. Each accepted SID polls at most once/minute; polling failures never queue duplicate sends. Queued expiry is 24 hours; delivery deadline becomes uncertain after 24 hours. At 1,000 outbox records, oldest terminal/suppressed history may be evicted; active entries are protected. Full active capacity retains one pending_admission intent per episode, keyed to the transition/settings revisions, with reason outbox_full. Reconciliation/notification steps retry oldest intents idempotently; admission_limited stays true while pending. ACK, recovery, superseding transitions and settings changes suppress pending intent; after 24 hours it fails with admission_expired. Cancellation/expiry remains visible until a later notification transition. Disabled transitions never form a pending backlog. Resolved episodes/terminal history otherwise expire after 30 days; unresolved episodes remain, capped at 10,000.

## 22.4 Object history

`GET /api/v1/objects/{object_id}/history` requires metric, from, to; optional resolution=raw|5m|1h (default raw). Metric is 1–128 ASCII letters/digits/underscore/period. RFC3339 bounds must satisfy from < to <= now within 24 hours/30 days/365 days respectively. Endpoints include both bounds; rollups select bucket starts, so bucket_end may exceed to. No cursor/limit. Over 2,000 combined points returns 400 history_too_large; missing object 404; no data returns empty series.

```json
{"object_id":"00000000-0000-4000-8000-000000000002","metric":"used_bytes","resolution":"raw","from":"2026-09-13T11:00:00Z","to":"2026-09-13T12:00:00Z","series":[]}
```

Example request:

```text
GET /api/v1/objects/00000000-0000-4000-8000-000000000002/history?metric=used_bytes&from=2026-09-13T11:00:00Z&to=2026-09-13T12:00:00Z&resolution=raw
```

Series contain identity and points. Identity fields: object_id, boot_id, inventory_generation, metric, kind, unit, source, scope, labels (including native clock/epoch). Raw points: observed_at, received_at, exact value, state, derived_rate_per_second, derivation_state, gap_before, elapsed_since_previous_seconds. Rollup points: bucket_start/end, approximate min/max/mean, sample_count, valid_sample_count, coverage, state_counts, unclassified_state_count, state, and gap fields. Counter rollups summarize rates/second. Coverage compares valid to stored samples, not expected cadence. Historical ok describes acquisition, not freshness today. Only closed rollup buckets are materialized by maintenance. Missing buckets are not synthesized; raw gaps over one hour are flagged conservatively. Never join different identities.

Metadata: selected_points, max_points, retention_seconds, backfilled=false, raw_gap_threshold_seconds, rollup_numeric_precision=approximate_f64, rollup_counter_values=rates_per_second, coverage_basis=stored_samples_not_expected_cadence.

## 22.5 Capacity exhaustion scenario

`GET /api/v1/objects/{object_id}/capacity-forecast` accepts no parameters. Missing object is 404; insufficient evidence returns 200 unknown:

```json
{"object_id":"00000000-0000-4000-8000-000000000002","state":"unknown","reasons":["insufficient_data"],"estimated_exhaustion_at":null,"scenario_range":null,"basis":{"sample_count":2,"minimum_sample_count":8,"minimum_span_seconds":3600},"assumptions":["Capacity exhaustion only; no drive failure prediction","One storage object and one compatible observation series"]}
```

Eligible objects: APFS container/volume, mount, NFS mount. Require current owner within 15 seconds, active identity, eight distinct paired used/total byte observations spanning one hour, one compatible series, unchanged positive capacity, and no gap over one hour. Raw window: 24 hours/2,000 pairs. Native freshness follows original monotonic acquisition/collector threshold; retained failed data is rejected. Legacy capacity freshness: 90 seconds.

Estimated output adds future estimated_exhaustion_at and scenario_range earliest_at/latest_at. Basis exposes sample_count, observed_from/to, observation_span_seconds, capacity_bytes, used_bytes, remaining_bytes, series_identity, growth_bytes_per_second, fit_r_squared, and scenario_growth_bytes_per_second low/high. Exact differences precede floating-point fitting. All intervals must grow, slope spread <=3, R²>=0.9. Interval extremes with 25% margins yield scenarios, not confidence intervals; horizons beyond 365 days are unknown. Reason codes: ineligible_object, owner_unavailable, identity_changed, history_too_large, unusable_observation, invalid_capacity, ambiguous_series, insufficient_data, incompatible_observations, insufficient_span, history_gap, capacity_changed, freshness_unavailable, stale_capacity, capacity_exhausted, no_reliable_growth, unstable_growth, unreliable_horizon. Three days is not promised.

## 22.6 NFS user quotas

`GET /api/v1/quotas` optionally filters node_id and canonical uid=0..2147483647, plus common paging. Empty data is valid:

```json
{"data":[],"meta":{"coverage":{"configured_targets":0,"returned_targets":0,"available_targets":0,"selected_nodes":1,"configuration_known_nodes":1,"configuration_unknown_nodes":0,"state":"unconfigured"}}}
```

Rows expose quota_id, node_id/name, server, export_path, uid, display_label, nfs_source, protocol=rquota-v1-udp, scope=server_filesystem_user, state, reason, observed_at, age_seconds, last_attempt, active, query_identity `{uid,gid,groups_truncated}`, metrics, limits, grace, linked_mount_ids, linkage_state, uncertainty. Metrics: active, used_bytes, block_soft_limit_bytes, block_hard_limit_bytes, used_inodes, inode_soft_limit, inode_hard_limit, block_grace_seconds_raw, inode_grace_seconds_raw, block_size_bytes; all retain measurement metadata. Limits block_soft/block_hard/inode_soft/inode_hard expose state/value/unit/basis. Grace block/inode retains raw/signed seconds, remaining estimate, deadline, observation date, basis, state.

Available requires one complete Q_OK collection. States distinguish no_quota, permission_denied, timeout, unsupported, unavailable, parse_error, unknown, stale, partial. Zero means unlimited only in available server evidence; no_quota is unknown. Active reports enforcement separately. Grace interprets signed 32-bit relative seconds. Configuration coverage distinguishes unconfigured, unknown, empty, available, partial. Policy metadata names collector/protocol, read_only, zero_limit, aggregation, authentication. Never sum UID/export rows or merge APFS quotas. Linkage requires exact configured NFS source; aliases remain unlinked.

## 22.7 Diagnostics, health, collector additions

`GET /api/v1/diagnostics` optionally filters node_id plus paging. Rows: node_id/name, object_id, kind=disk|filesystem, label, diagnostics. Disk diagnostics also appear on existing disk reads: checks `{collector,state,reason,observed_at,age_seconds,stale_after_seconds}`, assessment=readiness_only, integrity=unknown. SMART/IOKit states include disabled, waiting, ready, partial, unsupported, permission_denied, timeout, failed, stale, unknown. Filesystem diagnostics contain accessibility `{state,observation_state,reason,observed_at,evidence:{dead,not_responding}}`, capacity_collection, integrity=unknown. Explicit current NFS failure flags warn; both fresh clear flags establish recovery. Failed collection is not corruption; read-only mounts are not errors. List metadata states readiness_and_accessibility_only/unknown integrity.

Example diagnostic projection:

```json
{"kind":"disk","diagnostics":{"checks":[{"collector":"smartctl","state":"disabled","reason":"disabled_in_collector_configuration","observed_at":null,"age_seconds":null,"stale_after_seconds":null}],"assessment":"readiness_only","integrity":"unknown"}}
```

Health responses retain dimensions/unknown_dimensions and add assessment_state=complete|incomplete plus observed_severity=none|warning|critical. Incomplete nominally healthy overall becomes unknown; availability alone cannot prove healthy storage.

ciderd remains the sole collector. Configured rquota subjects (maximum 32) use read-only GETQUOTA with real process AUTH_SYS identity and bounded isolated workers; trusted-server UDP is not cryptographically authenticated. Targets configure server, absolute server-local export_path, uid and optional display_label/port; otherwise bounded portmapper lookup resolves the port. Interval is 15–3600 seconds; network timeout 1–10 seconds. Default: no targets; changes require collector restart. AUTH_SYS carries real UID/GID and at most 16 supplementary groups, with truncation reported. Collector nfs.rquota publishes nfs_user_quota resources and storage.nfs.quota.* metrics normalized to nfs_quota_* aliases. Current host diagnostics_configuration reports smart_enabled, nfs_path_refresh_enabled, nfs_quotas_enabled. Collector-state projection retains state, last_attempt, acquisition, reported_at. Acquisition preserves boot/generation/session, original received_at and age_at_receipt_seconds; repeated heartbeats cannot rejuvenate old attempts. Sections 17/19 retain full ingestion contracts.

## 22.8 Storage, capabilities, verification

SQLite schema 5 adds attention_episodes, notification_settings and notification_outbox, including bounded per-episode notification_intent_json, reconciliation timestamps/errors and independent last_send_claim_at. No secret is persisted. Existing raw/rollup tables back history. Console assets are compile-time embedded; web changes require rebuilding.

`GET /api/v1/capabilities` advertises alerts, twilio_notifications, nfs_user_quotas, diagnostics_readiness, capacity_forecasts, metric_history=true; notification_configuration=administrator_dashboard. Replacement forecasts, historical inventory, SSE, Prometheus and OpenAPI remain false. Capability means implemented support, not configured/complete evidence.

Verification: TBD — root will insert final workspace/check/headless/build, browser, quota-fixture and shared-document results after the current rerun. No real SMS/provider authentication/handset delivery is claimed. Local test fixtures and native shared-document publication are separate checks.
