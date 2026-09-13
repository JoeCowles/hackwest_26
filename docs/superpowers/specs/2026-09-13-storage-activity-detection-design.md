# Storage activity detection: first security slice

Status: approved for implementation, 2026-09-13. The user approved the design and
authorized revisions as issues are found. The previous implementation was committed as `5a496f1` before this
design work began. The user selected sustained unusual storage activity against
each source's own baseline, correcting an earlier selection of protected-file
access.

## Purpose and boundary

Surface sustained increases in a native driver's read or write activity, explain
the comparison and its coverage, and preserve a finding for the administrator.
The finding is an observation requiring assessment. It does not identify a file,
user, process, unauthorized action, or compromised node.

Use existing schema-2 ciderd telemetry. Native collection and five-second
heartbeats remain in ciderd. Evaluate read and write separately for each native
IOKit driver. Never compare different nodes to infer expected behavior, sum driver
observations for detection, or require all nodes to have the same responsibilities.

This slice includes the detector, durable findings, source readiness, a viewer
dashboard, and an administrator-only per-source relearning operation. Twilio
delivery, general alert acknowledgement, per-file monitoring, seasonal/workload
models, historical metric queries, and automated response are outside this slice.
The existing collector security-event view remains available separately.

## Existing implementation and shared specification

The Server Spec is the shared API source of truth:
https://docs.google.com/document/d/1JnsZHlYPXQ1IRsMSEHeboICzqqviKJylI7gRsuUK13E/edit?tab=t.bclfm0yxwd6r

The current Server Spec tab was read through Chrome and exported for read-only
analysis during this work. Sections 4.8 and 14 describe broader planned security
and alert behavior. Sections 16-19 describe the implemented read subset and disk
integration. In particular, `/events` currently exposes collector events and
returns `details:{}`. New findings need their own typed contract; changing the
meaning of collector events would conceal the origin of generated evidence.

This draft does not change or replace the shared contract. Implementation must
update the shared document in the same work with full routes, authentication,
fields, errors, examples, and actual verification status. No shared-document
write has been performed for this feature. A native UI save/export must be
reported as such; it cannot establish a successful connector write.

Relevant current code:

- `crates/ciderd/src/collectors/inventory.rs`: native controller resources and
  `iokit.block` counters, scoped to node, boot, and registry identity.
- `server/src/cider_api.rs`: authenticated immutable collection acceptance,
  exact counter deltas, monotonic intervals, and one ingestion transaction.
- `server/src/store.rs` and `server/migrations/`: SQLite version 2 and startup
  migration handling.
- `server/src/read_api.rs`: read authentication, frozen pagination, current
  health assessment, and collector events.
- `web/js/session.js`, `app.js`, and `views.js`: independent reads and the
  present security-event panel.

## Approach selection

| Approach | Benefit | Cost or limitation |
| --- | --- | --- |
| Rolling upper percentile, selected | Direct explanation in rates and durations; bounded state; includes a source's observed burstiness. | No knowledge of workload purpose or daily schedules. |
| Median plus median absolute deviation | Resistant to isolated extreme observations. | Idle and bursty sources often need additional spread/floor rules. |
| Exponentially weighted mean and deviation | Constant state and gradual adaptation. | Requires careful adaptation gates to avoid absorbing sustained concerning activity. |

The chosen rule uses a duration-weighted upper percentile. Parameters below are
initial engineering defaults. They are not calibrated sensitivity, false-positive
rates, statistical confidence, or evidence that a particular attack is detectable.

## Observation admission and identity

Only schema-2 resources with `resource_type=controller`, native driver scope,
source `IOBlockStorageDriver`, and collector `iokit.block` are eligible.
Signals are `storage.device.read_bytes_total` and
`storage.device.write_bytes_total`, with kind `counter`, unit `bytes`, and a
nonempty counter epoch. Legacy v1 sources remain unsupported by this detector;
their existing telemetry and views continue to work.

A source ID identifies node, native resource/object, collector, direction, scope,
and canonical metric attributes. Its continuity identity additionally includes
boot, agent generation/session, monotonic clock, counter epoch, and source/adapter
versions. A change to those continuity fields starts relearning. An unrelated
inventory revision or disk-association refresh does not reset an unchanged
driver's baseline.

Only newly accepted, advancing collections may affect detector state. Identical
receipt/collection replay has no effect. Old or out-of-order observations cannot
advance a duration or erase newer evidence. Detection begins with forward
observations; it does not generate retrospective findings from stored history.

An interval requires two usable counter endpoints in the same continuity domain.
Subtract exact unsigned-128-bit counters before conversion to bytes per second.
Counter decrease, nonfinite rate, or an interval outside 1-15 seconds makes the
interval unusable. The individual field must be available and live; a partial
collection can admit read while write is unavailable.

For current-activity eligibility, acquisition age at envelope creation plus the
absolute difference between server receipt time and envelope creation time must
be no greater than the lesser of 15 seconds and the collector stale policy.
Excessive delay or clock skew yields `timing_unknown`. This conservative gate
does not assert that the node's wall clock or telemetry is independently trusted.

Process unsuccessful collection attempts too, including an empty metric array
or an absent direction. They break that direction's pending/recovery continuity
and do not train a baseline. Repeated retained collector-state references are
not new attempts. A missing heartbeat is also not a zero-rate observation.

## Rule and initial policy

| Parameter | Initial value |
| --- | --- |
| Baseline horizon | Previous 30 minutes on the source monotonic clock |
| Learning requirement | At least 600 seconds and 60 accepted baseline intervals |
| Stored baseline bound | At most 1,800 intervals per source/direction |
| Baseline statistic `B` | Duration-weighted nearest-rank 95th percentile |
| Start threshold `H` | `max(3 * B, B + 1,000,000 bytes/second)` |
| Opening condition | Rate strictly above `H` for 120 continuous observed seconds |
| Recovery threshold `L` | `max(2 * B, B + 500,000 bytes/second)` |
| Recovery condition | Rate at or below `L` for 60 continuous observed seconds |

Calculate the reference from prior admitted intervals before evaluating the
current interval. For the weighted percentile, sort by rate and select the first
rate whose cumulative interval duration reaches 95% of total duration. Clip an
interval's weight at the baseline window boundary. Time with no usable
observation contributes no weight. Include genuine measured zero activity.

Freeze `B`, `H`, `L`, baseline coverage, and reference dates when the first high
interval starts a pending episode. Pending, open, and recovery observations do
not train the baseline. Aborted high candidates also remain excluded from
training. A brief peak therefore neither opens a finding nor silently inflates
the reference used for the next candidate.

The absolute excess floor deliberately excludes small absolute increases even
when their ratio to an idle baseline is large. It is a visible policy limitation,
not a claim that activity below the floor is safe. First-release policy values
are versioned and returned by the API; there is no silent automatic retuning.

## State and finding lifecycle

Keep baseline readiness, episode state, and observation freshness separate.
An open finding can coexist with unavailable telemetry.

1. **Learning:** collect usable baseline intervals and expose covered duration
   and count. A baseline describes observed activity, not verified authorization.
2. **Monitoring:** compare each new interval with the prior baseline. Train on
   ordinary observations while the reference remains sufficiently covered.
3. **Pending:** freeze the reference and accumulate only continuous qualifying
   source intervals. A nonqualifying interval or unavailable interval clears
   this pending streak. Pending activity is visible as pending, not a finding.
4. **Open:** after 120 seconds, create one durable finding for this source and
   direction. Later intervals update its last and peak evidence, not its ID or
   frozen thresholds. Continue using that reference if activity lasts beyond
   the ordinary 30-minute baseline horizon.
5. **Returned below threshold:** 60 continuous usable seconds at or below `L`
   resolve the finding with that precise reason. Values between `L` and `H`
   keep it open and reset recovery progress. Resume baseline maintenance using
   any still-unexpired ordinary observations; relearn if coverage is insufficient.
6. **Unavailable:** missing, failed, stale, or late observations cannot resolve
   a finding. Preserve it with dated evidence and unavailable monitoring state.
7. **Interrupted:** proven source removal, identity change, or an explicit
   rebaseline ends the old episode with the corresponding reason, without
   asserting recovery or benign activity. Start a new baseline when appropriate.

Ordinary server restarts preserve baseline and episode state. A gap greater
than 15 seconds breaks pending/recovery continuity, including after restart.
The restart itself cannot reopen a duplicate finding. Collector restart changes
the continuity identity and starts relearning.

## Persistence and transaction boundary

Introduce SQLite migration `003_detection.sql` and raise the supported database
version to 3. Persist source state, exact prior counter/time, observation
watermark, baseline ring, frozen episode reference, policy/baseline versions,
and findings. Finding evidence includes exact counter endpoint strings,
collection identities, interval duration/rate, source and receipt dates, baseline
statistic/coverage, thresholds, elevated duration, and first/latest/peak evidence.
History remains inspectable after resolution, interruption, or relearning.

Evaluate a newly accepted native collection inside the existing ingestion
transaction, after immutable-ID validation and successful projection. The
receipt, accepted observations, detector state, findings, and change cursor
commit together. A replay, failed transaction, or server restart cannot create
duplicate opening transitions. Do not scan read snapshots or historical
`metric_samples` for detection: they do not preserve all admission provenance.

Bound each serialized baseline ring to 256 KiB and each complete source state to
512 KiB, with at most 8,192 active source/direction states per server. Admission
exhaustion appears as `capacity_limited` in coverage and in the status of known
sources that could not be admitted. It must not discard otherwise valid node
telemetry. Unknown or incomplete inventory stays distinct from an admitted
source count. Test these limits before enabling the feature.
Retain closed findings for 30 days; retain an open finding while unresolved.
Existing maintenance performs pruning. No second detection polling loop is added.

## API and administrator relearning

All reads use the existing administrator/viewer read authentication, standard
data/meta envelope, no-store policy, request IDs, bounded read budget, and
frozen pagination. Node credentials cannot read findings. Default page size is
100, maximum 500, and cursors expire after 300 seconds as in existing reads.

| Route | Purpose and query fields |
| --- | --- |
| `GET /api/v1/findings` | List findings; `node_id`, `object_id`, `status=open|resolved|interrupted|all` (default open), `from`, `to`, `limit`, `cursor`. Time filters apply to first observation and are optional so old open findings remain visible. |
| `GET /api/v1/findings/{finding_id}` | One finding with typed evidence and lifecycle reason; no query parameters. |
| `GET /api/v1/detectors/storage-activity/sources` | Source readiness and policy; `node_id`, `object_id`, `limit`, `cursor`. Include separate baseline, episode, and freshness states, reasons, coverage, and baseline version. |
| `POST /api/v1/detectors/storage-activity/sources/{source_id}/rebaseline` | Administrator-only explicit relearning for a legitimate workload change; requires existing mutation UUID/timestamp headers. Body contains `expected_baseline_revision` (canonical unsigned-64-bit decimal string) and `reason=planned_workload_change|operator_reassessment`. Viewer/node credentials cannot perform it. |

Successful reads and relearning return 200. Relearning atomically checks the
expected revision, records the action, interrupts an existing finding with
`administrator_rebaseline`, increments baseline revision, and clears training
and streaks. Preserve the most recent observation watermark so old collections
cannot seed the new reference. A stale expected revision returns 409; unknown
source/finding returns 404. Existing authentication, query, replay, rate-limit,
cursor-expiry, and unavailable errors apply and must be documented individually
in the shared contract. A fresh read resolves an uncertain relearning response;
repeating it must not silently perform another reset.

Relearning is an explicit administrator API action; the viewer console remains
read-only. It does not mark a prior finding benign. General acknowledgement and
per-source threshold editing are deferred.

The implementation plan must enumerate the serializers and complete JSON examples
for these routes. The public shared-spec update and tests must match the final
typed contracts. Source/finding/node/object IDs are UUIDs; native resource and collection IDs are opaque strings. Revisions and exact counters are
decimal strings, rates are finite numbers in bytes/second, and wall timestamps
are RFC3339 UTC. Findings use warning severity; this rule supplies no probability
of compromise or automatic escalation to critical.

## Dashboard behavior

Update the Security view to show detection coverage/readiness, generated findings,
and collector-reported security events in distinct sections. Show learning
progress, unavailable or unsupported sources, first/latest observation dates,
currentness, driver identity, read/write direction, baseline, threshold, observed
rate, duration, and the lifecycle reason. An empty finding list must not become a
healthy-security label.

Link every finding to its node and source object. Link to a physical disk only
where a current confirmed association supports it; otherwise explain that the
finding is scoped to the driver. Do not backfill a historical physical attribution
from an unrelated current mapping. Driver activity can include virtual/backing
traffic and does not identify application throughput or a file access.

Fetch source readiness and finding pages independently of core polling. Freeze
evidence while paging and preserve dated rows on fetch failure, with an explicit
unavailable banner. An active, current anomaly can supply a warning in the
security-activity dimension. No finding, unsupported coverage, and stale evidence
cannot establish that security is healthy.

Expose narrowly named capabilities for storage-activity detection and findings.
Keep general alert delivery and Twilio capabilities unavailable. Rebuild embedded
assets after changes and verify the actual served console.

## Intended implementation files

- New `server/src/detection.rs` for typed policy/state/evidence and deterministic
  transitions; split persistence into `server/src/detection_store.rs` if needed
  to keep the transition layer free of I/O.
- New `server/migrations/003_detection.sql`; changes in `server/src/store.rs`,
  `lib.rs`, and `workers.rs` for migration, registration, and retention.
- `server/src/cider_api.rs` for accepted collection-attempt integration.
- `server/src/read_api.rs` and narrowly scoped detection API code for reads;
  reuse existing mutation authentication/replay logic for relearning without
  exposing administrator credentials to the browser.
- New `web/js/security.js` and `security-views.js` if needed to keep the feature
  isolated; small integration changes to `app.js`, `session.js`, `views.js`,
  `data.js`, styles, and the server's embedded-asset allowlist.
- Focused Rust/API/web tests, a synthetic protocol smoke, README/status updates,
  and the shared Server Spec. No collector wire-schema or native-probe changes.

## Acceptance and verification

Use deterministic synthetic observations and injected time for detector tests.
Do not generate an actual attack, alter protected files, or use an uncontrolled
disk workload as a test fixture.

| Case | Required result |
| --- | --- |
| First counter then 120 five-second intervals at 2 MB/s | Learning completes at 600 covered seconds; B=2, H=6, L=4 MB/s. |
| 8 MB/s for 115 seconds, then five more seconds | Pending at 115; exactly one finding at 120. |
| Repeated receipt/collection | No extra duration, training, finding, or reset. |
| Rate exactly 6 MB/s while pending | Does not satisfy the strict opening threshold. |
| Open finding then 4 MB/s for 60 seconds | Resolves at 60 with returned_below_recovery_threshold reason. |
| Brief spike, 20-second gap, missing field, failed empty collection | No fabricated zero; correct per-direction streak break and unavailable state. |
| Read is valid while write is unavailable | Independent read evaluation and write uncertainty. |
| Source A baseline 2 MB/s and B baseline 80 MB/s; both reach 80 MB/s | A qualifies and B does not; no cross-source comparison. |
| Idle baseline followed by 0.5 MB/s | Small nonzero activity stays visible but does not pass the excess floor. |
| Clock/boot/epoch/session change or source removal | No cross-boundary delta or recovery claim; prior episode interrupted. |
| Open episode, ordinary server restart, then receipt retry | Same persisted episode; no duplicate opening. |
| Elevated activity continues beyond 30 minutes | Frozen threshold does not adapt upward. |
| Explicit relearning after legitimate workload change | Prior evidence remains; revision advances once; new learning is explicit. |
| Wall-clock skew, late envelope, stale acquisition | Timing eligibility fails explicitly and adds no evidence duration. |
| Migration/restart, rollback, retention, source limits, read/write auth | Durable/atomic behavior; unavailable states rather than silent loss. |
| Browser loading/failure/recovery and frozen pagination | Core polling continues; readiness and dated evidence remain truthful. |

Run `cargo test --workspace --locked --no-fail-fast`, `cargo check --workspace
--locked`, headless check, all web tests, JavaScript syntax checks, and whitespace
checks. Rebuild embedded assets. Exercise authenticated finding/status reads and
relearning against a disposable server with synthetic schema-2 inputs, and verify
desktop/mobile browser behavior. A real native collector smoke establishes
source admission and learning only; synthetic time or shortened test policy
must never be described as a production-duration live detection result.

## Explicit limitations

- The first rule detects sustained upper-rate deviations, not every form of
  anomalous behavior. Low-rate misuse and activity present during learning can
  go undetected.
- Legitimate workload changes can produce findings; the administrator assesses
  context and deliberately requests relearning when appropriate.
- A source's observed baseline is not proof of benign behavior. Node-reported
  telemetry is not independent attestation of a compromised node.
- Legacy inputs, file access, NFS per-export attribution, seasonal patterns,
  notification delivery, and cross-reboot lifetime hardware trends remain
  outside this first slice.

## Implementation refinements

Compact persisted summaries expose `baseline.as_of`: baseline coverage/readiness
are as of that evaluation, while observation freshness ages independently on
reads and in the browser. This avoids loading baseline rings for dashboard polls.
Node/object filters apply before the 10,000-source read bound; core warnings use
a separate bounded query for current open episodes. The resolved reason is
`returned_below_recovery_threshold`. Native resource/collection identifiers are
opaque strings, while source/finding/node/object identifiers are UUIDs.

Local implementation and tests are recorded in `docs/storage-activity-detection.md`.
Rendered browser QA and native shared-spec save/export remain pending because
browser control was stopped during this turn; no final documentation sign-off
is claimed.
