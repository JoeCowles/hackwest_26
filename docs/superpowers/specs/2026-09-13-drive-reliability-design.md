# Drive reliability design

Approved in conversation on 2026-09-13: persistent passive assessment of SMART,
reported sector defects, IOKit errors/retries, and degraded read/write service.
The user explicitly authorized implementation after reviewing the scope.

## Boundary

Native acquisition remains in ciderd; heartbeats and I/O sampling remain five
seconds. SMART remains an optional configured tool, sampled every 300 seconds by
default. No active surface scan, write probe, SMART self-test, remediation, SMS
delivery, production installation, commit, push or packaging is included.
Durable findings are available for a future notification consumer. Failure dates
remain unknown; the three-day planning objective is not a detector threshold.

Preserve existing uncommitted storage-activity detection. Work in the current
non-main checkout and retain a source-only baseline for review. All changes are
additive except targeted source-validation fixes required for reliable evidence.

## Collector evidence

IOKit already supplies per-direction bytes, operations, errors, retries, and
driver-accounted nanoseconds. Their driver-instance epochs and exact decimal
integers are retained. They do not measure application latency, queue depth, or
physical bad-block addresses.

Extend the SMART catalog/parser with integer gauges in sectors:
`storage.ata.reallocated_sectors`, `storage.ata.current_pending_sectors`, and
`storage.ata.offline_uncorrectable_sectors`. Accept only recognized ATA IDs
5/197/198 with conservative corresponding names and unambiguous exact raw
counts. Missing/vendor-specific/packed fields cannot silently become counts.
Pending/uncorrectable counts may decrease; they are not monotonic counters.
Retain source fields, identity confidence, and bounded SMART provenance.

Validate SMART's reported device locator against the admitted physical resource
when supplied. Carry a private hash-based device-identity continuity marker on
all SMART readings, including gauges. Do not publish serial/WWN values. Identity
changes reset comparisons and interrupt old episodes. An absent identity is
explicitly weak; locator reuse is not durable hardware identity.

SMART tool failures (exit bits 0-2, timeout, permission, malformed output) are
telemetry failures; parsed device health readings remain independently usable.
Higher exit bits distinguish current SMART failure/threshold reports from
historical error/self-test-log entries. NVMe namespace scope remains explicit.

## Reliability rules

Implement deterministic rules in a new pure server module, independent of the
security activity detector. Policy identifiers and thresholds are exposed with
evidence. Initial rules are engineering heuristics, not efficacy claims.

* Current SMART overall failure and decoded NVMe warnings open immediately on
  admissible fresh evidence. Preserve whether the device reports depleted spare,
  temperature, degraded reliability, read-only media, backup failure or an
  unknown warning bit. An overall failure is a reported health condition, not a
  claim every I/O has failed.
* Report pending/uncorrectable ATA sectors currently present. Preserve existing
  reallocated-sector history; new increases are deterioration evidence.
* NVMe spare below its reported threshold and endurance used >=100% are
  replacement-planning evidence. Endurance used is a vendor estimate, not a
  failure date. Declining spare and increasing media errors are explicit trends.
* New IOKit read/write errors and NVMe media-error increments are observed error
  evidence. New retries are deterioration evidence. A first lifetime total >0
  is preexisting history, not an error occurring at receipt time. Error-log entry
  totals and unsafe shutdowns remain context, not standalone media-failure rules.
* Performance uses matched per-direction bytes/operations/accounted-time
  deltas. Intervals require unchanged continuity, live endpoints, 1-15 seconds,
  >=20 operations, and positive accounted time. Compare the same transfer-size
  and operation-rate bucket; otherwise readiness remains explicit. Bucket
  transfer size at <=4 KiB, <=16 KiB, <=64 KiB and larger; operation rate uses
  bounded powers-of-two bands. Keep at most 720 intervals per direction in a
  preceding 3600-source-second window. Readiness requires 600 covered seconds
  and 60 comparable intervals. Duration-weighted P95 B defines elevation
  strictly above max(3B, B+1,000,000 ns/op), sustained for 120 observed seconds.
  Recovery requires 60 comparable usable seconds at/below max(2B,B+500,000 ns/op).
  Freeze the reference during pending/open episodes; do not train on those
  observations. Label the result driver-accounted service-time degradation;
  workload/caching/controller effects remain possible causes.

For direct conditions and counter-error episodes, require two distinct usable
clear observations to resolve. Repeated heartbeat delivery never counts as a
second observation. Missing/failed/stale/timing-ineligible evidence cannot clear
an episode and breaks pending/recovery streaks. No current concern means only
that supported observed rules found none, not that unobserved dimensions are
healthy. Boot/session/source/device-identity changes and proven removals
interrupt rather than resolve old episodes.

## Persistence and identity

Source identity is node + native resource + collector (driver directions remain
separate rules within a source). Preserve opaque native identifiers and UUID
API identifiers. Store states and findings in new reliability tables in schema
4. Evaluate new accepted collections, including empty failed attempts, in the
existing immutable heartbeat transaction. Receipt failure rolls back detector
state and findings. Replays/nonadvancing acquisitions add no evidence.

Store bounded source state (512 KiB/source, maximum 8192 admitted active sources)
and compact read summaries. Capacity exclusion remains visible and does not
reject otherwise valid telemetry. Retain unresolved findings; expire closed
findings after 30 days. Read paths must use summaries, not training rings.

Each finding includes ID, source/node/object identity, rule and policy version,
dimension, classification, severity, status, first/last/open/end dates, reason,
and exact evidence. Historical source association must never be rewritten to a
replacement disk. Source findings remain inspectable when physical association
is unresolved. Current disk attribution requires unique, authoritative,
boot/session-compatible topology; namespace evidence retains namespace scope.

## Read API and console

Add viewer/admin reads using existing authentication, budgets and envelopes:

* GET /api/v1/reliability/sources (node_id, object_id, limit, cursor).
* GET /api/v1/reliability/findings (node_id, object_id, status, from, to, limit, cursor).
* GET /api/v1/reliability/findings/{finding_id}.

List defaults: 100 rows, max 500/page, frozen 300-second traversals. Reject unknown
and duplicate parameters, invalid UUIDs/status/time ranges; bound selected rows
to 10000. Existing owner/path/query-bound cursors and cache byte limits apply.
Finding default status is open; all/resolved/interrupted are supported.

Add `reliability` to physical-disk summaries:

```json
{
  "assessment": "unknown",
  "observation_state": "unknown",
  "source_count": 0,
  "current_source_count": 0,
  "sources": [],
  "findings": [],
  "unknown_dimensions": ["media_health", "performance"],
  "replacement_forecast": {"state": "insufficient_data", "estimated_failure_at": null}
}
```

Assessment values are unknown/no_current_warning/warning/critical. Observation
states are current/stale/unavailable/unknown; field-level rule states/freshness
remain visible. Only current, attributable active findings supply media_health
or performance warnings in core summaries. No implicit healthy upgrade.

The existing disk page shows source coverage, reported conditions, deterioration,
service-time readiness/evidence, exact counters, dates and source links. Browser
latency/sleep ages currentness; retained evidence remains dated and inspectable.
No new navigation or browser mutation control is needed. Add the read-only
findings/source APIs for complete inspection and future notification consumers.

Update the shared Google Docs Server Spec during API work with complete fields,
routes/auth/status/examples and verification boundaries. Native UI save/export
evidence must remain distinct from connector write/sign-off. A local report is
not a substitute for the shared contract.

## Validation

Test-first parser fixtures and pure rule timelines; real SQLite/API replay,
rollback, restart, retention, bounded admission, filters/frozen cursors and
topology attribution tests. Verify unsupported SMART, partial source failure,
huge exact counters, historical totals, device replacement, stale clocks, normal
idle/workload changes, service-time onset/recovery and gaps. Run workspace tests
and checks, desktop/headless builds, web tests/syntax and diff checks. Rebuild
embedded assets before passive verified-TLS native smoke and desktop/mobile
synthetic fault rendering. No failing physical drive or second Mac is assumed.

Primary source basis: Apple SDK IOBlockStorageDriver.h cumulative statistics;
smartmontools nvmeprint.cpp critical warning/SMART log fields and atacmds.h
vendor attribute definitions. Collector capabilities vary by drive/bridge.
