# Drive reliability — passive detection

Implemented 2026-09-13. Orchard now persists source observations and findings for SMART/NVMe health, reported ATA sector defects, IOKit errors/retries, and workload-qualified read/write service-time degradation. Collection runs in ciderd; the central server only validates and evaluates accepted telemetry. Heartbeats and I/O sampling remain five seconds.

## Collection and meaning

SMART is opt-in: configure an absolute, verified `tools.smartctl` path in the node's ciderd configuration. `collection.smart_seconds` defaults to 300. Existing IOKit collection supplies errors, retries, operations, bytes, and driver-accounted time. Missing, unsupported, failed, stale, and partial observations remain explicit. No active scan, SMART self-test, write probe, remediation, or Twilio delivery is introduced.

The catalog adds integer gauges in sectors: `storage.ata.reallocated_sectors`, `storage.ata.current_pending_sectors`, and `storage.ata.offline_uncorrectable_sectors`. Server projection aliases are `ata_reallocated_sectors`, `ata_current_pending_sectors`, and `ata_offline_uncorrectable_sectors`. Recognized ATA IDs 5/197/198 require corresponding names and unambiguous exact raw counts. Packed/vendor values are omitted. A malformed or duplicate ATA table is omitted as a unit while independently usable SMART/NVMe observations survive as partial acquisition. These are reported counts, not a surface scan or bad-block addresses.

SMART readings retain `source_field`, private `device_identity`, `device_identity_confidence` (reported_wwn, reported_serial, caller_epoch_only), and fixed `smartctl_exit_status` / `smartctl_exit_status_class` provenance. Raw serial/WWN values are not emitted. A supplied device locator must equal the admitted path. Native namespace scope and weak identity remain explicit. Tool failure and device-reported failure are distinct.

## Rules and episode lifecycle

Current SMART failure, decoded NVMe warning bits, ATA pending/uncorrectable sectors, spare below its reported threshold, and endurance used >=100% produce immediate findings. Spare decline, new reallocated sectors, new media errors and IOKit read/write errors or retries produce trend/delta evidence. The first nonzero lifetime total is historical context, not a newly dated error. NVMe historical error-log entries and unsafe shutdowns are not standalone media-failure rules.

Direct and counter episodes require two distinct usable clear acquisitions to resolve. Missing/stale/failed fields break recovery and retain the open finding. Boot, session, device, source or counter continuity changes interrupt comparisons and old episodes; interruption does not assert recovery. Receipt replay and repeated collection IDs do not refresh evidence. State, findings and immutable heartbeat receipts commit in one SQLite transaction and survive restart.

Performance compares exact same-epoch per-direction counter deltas. An interval must cover 1–15 source seconds, at least 20 operations, and positive driver-accounted time. Comparison buckets use mean transfer size (<=4 KiB, <=16 KiB, <=64 KiB, larger) and powers-of-two operation-rate bands. Each direction retains at most 720 intervals over 3600 source seconds. A reference requires 600 covered seconds and 60 comparable prior intervals and uses duration-weighted P95 B.

A finding opens after 120 observed seconds above max(3B, B+1,000,000 ns/op), and resolves after 60 comparable seconds at or below max(2B, B+500,000 ns/op). The pending/open reference is frozen; candidate/open intervals do not train it. Gaps, idle intervals, missing timing, and changed workloads cannot claim recovery. This is driver service-time evidence, not application latency, disk utilization, or a hardware benchmark.

## Authenticated read routes

GET `/api/v1/reliability/sources`: optional `node_id`, `object_id`, `limit`, `cursor`.

GET `/api/v1/reliability/findings`: optional `node_id`, `object_id`, `status`, `from`, `to`, `limit`, `cursor`. Status is open by default; allowed values are open, resolved, interrupted, all. The optional RFC3339 range is half-open on `first_seen_at` (from inclusive, to exclusive), with from < to. Absent bounds retain old open episodes.

GET `/api/v1/reliability/findings/{finding_id}`: one finding, including resolved/interrupted findings; no query parameters.

All three are GET-only, have no request body, and require `Authorization: Bearer <viewer-or-administrator-credential>`. Node credentials receive 403; missing/invalid/expired credentials receive 401. The existing read budget is shared across routes: burst 20, refill 2 requests/second. `X-Request-ID` may supply a UUID; otherwise the server generates it. Responses include `Cache-Control: no-store` and `X-Request-ID`.

Success is HTTP 200 with `{data,meta}`. List data is an array; detail data is one object. Meta contains `api_version` ("1"), `server_time` (UTC RFC3339), `request_id` (UUID), `snapshot_cursor` (decimal change ID string), `next_cursor` (opaque string or null). List limit defaults to 100 and permits 1–500. Frozen pages expire after 300 seconds and bind the credential, route, filters and page size. Later ingestion cannot change an existing traversal.

GET errors use the existing `{error:{code,message,fields,request_id}}` envelope: 400 invalid_query (unknown/duplicate parameters, invalid UUID/enum/time/range/limit, cursor mismatch, or >10000 selected findings); 401 unauthenticated; 403 forbidden; 404 not_found for absent detail; 410 cursor_expired; 429 rate_limited with Retry-After: 1; 503 unavailable/source_limit for bounded read/admission pressure; 500 internal_error for persistence failure. Non-GET methods receive the framework method-not-allowed response (405). Source selection over 10000 requires narrower filters. Reliability read selections have a 32 MiB JSON budget checked before body materialization. Frozen-page cache retains its existing 128-page/32 MiB aggregate bounds.

## Source, signal and finding fields

A source contains string IDs `source_id`, `node_id`, `object_id`, `resource_id`; collector (`iokit.block` or `smartctl`), scope (driver, device or nvme_namespace), active boolean, policy_version, policy, device_identity_confidence, observation, signals, findings, replacement_forecast, support_state (supported/capacity_limited), acquisition and optional support_reason. An unresolved object_id is null. Acquisition contains boot_id, agent_generation and agent_session_id. IDs identify source evidence, not an assumed physical-drive association. Training rings are never returned.

Each observation contains state (current/stale/unavailable/unknown), reason (string or null), age_seconds and stale_after_seconds (number or null), observed_at and received_at (UTC timestamps or null). Source and individual signal ages are independent. A fresh partial attempt cannot restamp an absent field. Source freshness honors the admitted native stale threshold and owner availability.

Each signal contains rule_id, dimension (media_health/performance), state, severity, reason, finding_id (nullable), observation and evidence. Rule states include unknown, historical_or_initial_observation, no_current_warning, warming_up, workload_not_comparable, pending, warning and recovering. Evidence is rule-specific and versioned: exact counter endpoints/epochs/collection IDs/deltas, reported health values and bounded provenance, decoded warning bits, or performance workload/reference/threshold/coverage/episode details. Unsupported fields are unknown rather than zero.

A finding contains string finding_id, source_id, node_id, object_id, resource_id, rule_id, dimension, classification, severity, status, summary, policy_version, first_seen_at, last_seen_at, opened_at, updated_at; nullable ended_at and reason; and evidence. Classification distinguishes reported_health_condition, replacement_planning, observed_error, deterioration and service_time_degradation. Exact integer quantities remain decimal strings. Open means an unresolved episode; it is not by itself proof that supporting evidence is still current. Read source/signal observation states alongside it. Raw finding APIs do not invent a current flag.

The source-list meta also returns policy and coverage: known_sources, active_sources, supported_sources, capacity_limited_sources, current_sources, assessment (unknown) and inventory_completeness (unknown). Policy exposes version, source byte/admission/retention limits, baseline horizon/count/duration, percentile, onset/recovery thresholds and durations, interval/operation minima, required_clear_observations and endurance_used_threshold_percent. Values are those described above; source size is 524288 bytes, active admission 8192, closed retention 2592000 seconds. Open findings are retained; resolved/interrupted findings expire 30 days after ended_at. Admission or state-size exclusion is explicit and does not reject an otherwise valid heartbeat.

## Physical-disk and health integration

Existing GET `/api/v1/nodes/{node_id}/disks` adds a `reliability` field to each DiskSummary. Existing authentication, filters, pagination, status codes and topology metadata remain as documented for that route. The new field contains assessment (unknown/no_current_warning/warning/critical), observation_state, source_count, current_source_count, sources, findings, unknown_dimensions, and replacement_forecast. Findings in this disk projection add a boolean current derived from their supporting signal and attribution.

Only active supported sources with current matching boot/session/generation and unique normalized ownership attach to a physical disk. IOKit additionally requires one resolved driver association. Counts describe these selected attributable sources, not full device coverage; unassigned and capacity-limited observations remain visible through the source API. Reused locators and old acquisitions cannot transfer a finding to a replacement drive. Unobserved open findings prevent a reassuring no_current_warning assessment. Only current attributable findings raise node/cluster media_health or performance warnings. Other health dimensions remain independent.

The disk panel shows evidence, scope, confidence, baseline readiness and dates. Browser elapsed time and failed-refresh state age the display without changing frozen evidence. Unknown replacement_forecast is `{state:"insufficient_data",estimated_failure_at:null}`. Three days is a replacement-planning objective, not a prediction horizon or alert threshold. Capabilities now advertise drive_reliability and reliability_findings true; replacement_forecasts, alerts and twilio_notifications remain false.

## Examples

Synthetic requests: `GET /api/v1/reliability/sources?node_id=22222222-2222-4222-8222-222222222222&limit=100`; `GET /api/v1/reliability/findings?status=all&from=2026-09-13T00:00:00Z&to=2026-09-14T00:00:00Z`; `GET /api/v1/reliability/findings/11111111-1111-4111-8111-111111111111`.

An empty findings response is explicit:

```json
{"data":[],"meta":{"api_version":"1","server_time":"2026-09-13T12:00:00.000Z","request_id":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","snapshot_cursor":"0","next_cursor":null}}
```

The DiskSummary reliability field when no source can be attributed:

```json
{"assessment":"unknown","observation_state":"unknown","source_count":0,"current_source_count":0,"sources":[],"findings":[],"unknown_dimensions":["media_health","performance"],"replacement_forecast":{"state":"insufficient_data","estimated_failure_at":null}}
```

For counter interpretation, an initial error total of "184467440737095516160" remains history. A later distinct usable observation of "184467440737095516161" with unchanged epoch produces exact delta "1" and observed_error evidence. The source and collection IDs and timestamps identify that comparison. Repeating either receipt or collection ID produces no new comparison or fresh acquisition.

## Verification and operation

238 Rust workspace tests passed; three subprocess helpers are intentionally ignored. All 104 web tests, workspace check, headless workspace check, desktop workspace build, and JavaScript syntax checks passed. Passive verified-TLS smoke observed four IOKit reliability sources and one physical disk on this Mac, two distinct five-second heartbeats, embedded reliability modules and source aging after stopping the collector. Optional SMART was disabled in that real-Mac smoke; synthetic parser and ingestion tests verify SMART/ATA/NVMe failure conditions. Synthetic desktop and 390×844 browser checks verified current/unknown/stale states, failed disk refreshes, exact retained counters, readable labels and responsive layout. These tests are not detection-efficacy validation on failing hardware.

The supplied Python validator validates the 74-entry catalog, schemas, examples and 21 invalid-input cases, then fails the pre-existing fixture request-deadline invariant. The saved pre-change baseline fails the same invariant. The Rust contract suite passes. No production installation, service registration, packaged-app rebuild, commit or push was performed.

```sh
bash scripts/cargo-local.sh test --workspace --locked --no-fail-fast
bash scripts/cargo-local.sh check --workspace --locked
bash scripts/cargo-local.sh check --workspace --locked --no-default-features
bash scripts/cargo-local.sh build --workspace --locked
node --test web/tests/*.test.mjs
node scripts/smoke-reliability.mjs
```

Shared Server Spec section 21 was updated through native Google Docs editing and confirmed Saved to Drive. A post-save DOCX export matches the prepared content, retains native Heading 1/2 styles, and preserves all 2472 previous nonempty paragraphs. This is verified native UI publication, not a connector write or connector sign-off.
