# Storage activity detection

Implementation, automated verification, independent code review and rendered
desktop/mobile acceptance are complete in the current checkout. Server Spec
section 20 was published through native Google Docs UI, confirmed Saved to Drive
and checked against a fresh export. This is native UI evidence, not a connector
write or connector sign-off. No feature commit, push, package or deployment is
part of this work.

## Product boundary

Cider identifies sustained increases in native driver read or write activity
against that source's own observed baseline. A finding supplies dated evidence for
the administrator; it does not establish unauthorized file access or compromise.
The rule uses accepted schema-2 ciderd `iokit.block` byte counters independently
for each driver and direction. Five-second heartbeats and native collection stay
in ciderd. Legacy v1 input, per-file/process attribution, seasonal models, Twilio,
alert acknowledgement and automated response remain outside this slice.

A usable interval requires two available/live counter endpoints, exact u128
subtraction, unchanged continuity and 1–15 seconds of monotonic duration. Admission
also checks conservative acquisition age plus receipt/envelope clock skew.
Failed/empty attempts and missing fields break pending/recovery continuity and
never become zero activity. Replays and nonadvancing acquisitions add no evidence. Timing-ineligible
observations cannot replace canonical continuity or interrupt an open finding;
they break streaks while preserving baseline, revision and unresolved evidence.

## Rule and lifecycle

Training covers a previous 1,800-second source-time window, capped at 1,800
intervals and 256 KiB. Readiness requires 600 covered seconds and 60 intervals.
Let B be duration-weighted nearest-rank P95. Elevation is strictly above
`H = max(3B, B + 1,000,000 bytes/s)` for 120 observed seconds. Recovery is at or
below `L = max(2B, B + 500,000 bytes/s)` for 60 continuous usable seconds.

The pending/open reference is frozen; candidate, open and recovery observations
do not train it. Missing telemetry does not resolve an open finding. Proven source
removal, continuity change or administrator relearning interrupts the old episode.
Normal server restart preserves the episode, baseline and observation watermark.
Closed findings expire after 30 days; unresolved findings remain inspectable.

## Implementation and API

- `server/src/detection.rs`: typed deterministic state machine and evidence.
- `server/src/detection_store.rs`: atomic native mapping, compact summaries,
  rebaseline audit/change log and retention.
- `server/migrations/003_detection.sql`: persisted sources, findings and audit;
  schema version 3 upgrades inside the startup transaction.
- `server/src/cider_api.rs`: evaluates new accepted attempts inside the receipt
  transaction, including failed or empty acquisitions.
- `server/src/read_api.rs` and `server/src/api.rs`: viewer/admin reads and
  administrator-only revision-guarded relearning.
- `web/js/security.js` and `web/js/security-views.js`: independent read state,
  source readiness, generated findings and exact interval evidence.

Routes:

| Route | Role | Result |
| --- | --- | --- |
| `GET /api/v1/findings` | Viewer/admin | Findings; defaults to open. |
| `GET /api/v1/findings/{finding_id}` | Viewer/admin | One durable finding. |
| `GET /api/v1/detectors/storage-activity/sources` | Viewer/admin | Source statuses, policy and coverage. |
| `POST /api/v1/detectors/storage-activity/sources/{source_id}/rebaseline` | Admin | Updated source summary. |

Relearning requires fresh UUID/timestamp transport headers, a canonical decimal
`expected_baseline_revision`, and reason `planned_workload_change` or
`operator_reassessment`. It preserves the watermark and historical evidence,
advances the revision, and records an audit/change-log entry atomically. The viewer
UI does not expose administrator credentials or a mutation control.

Source/finding/node/object IDs are UUIDs; native resource, collection, boot,
session, clock and epoch identifiers are opaque strings. Source IDs exclude
unrelated inventory revisions. Counter and monotonic values in finding evidence
remain exact decimal strings.

The server admits at most 8,192 active source/direction states and keeps otherwise
valid telemetry ingestion working when detector capacity is exhausted. Excluded
known sources are explicit `capacity_limited` records. Source reads use compact
cached summaries, not training rings. `baseline.as_of` dates cached readiness and
coverage; observation freshness is evaluated independently at read time. An open
finding only supplies a core security warning when its source is active,
supported and currently observed. Other cases retain unknown security assessment.

Pagination freezes data, time, policy and coverage for 300 seconds, with limits
of 500 rows/page, 128 cached traversals and 32 MiB cache. Source selection is
bounded at 10,000 after node/object filtering (`503 source_limit`); findings above
10,000 require narrower filters (`400 invalid_query`). Core warning reads are
independent of the historical-source selection limit.

## Verification evidence

Focused persistence verification completed during implementation: 11 real SQLite
cases plus 19 existing native compatibility cases passed. Coverage includes
schema-2 migration, actual 8,192-state admission exhaustion, replay/rollback,
empty failed attempts, revision/audit/reopen, source removal, timing uncertainty,
a full injected-time 600/120/60-second lifecycle, retained unresolved findings,
closed retention boundaries and narrowed bounded reads.

The pure module example in `server/examples/detection_example.rs` generates
synthetic DTOs; it does not produce a disk workload or establish measured detection
efficacy. The final verification below consolidates workspace, native smoke,
rendered browser and shared-document results.

Final verification on 2026-09-13:

| Check | Verified result |
| --- | --- |
| Rust workspace | 188 passed, zero failed; three subprocess-helper entrypoints intentionally ignored. |
| Feature cases | 18 pure detector, 11 persistence/ingestion and five API tests passed. Includes real schema-2 HTTP rejection of a mismatched clock without interrupting a finding. |
| Web | 97 passed after the browser admission-label correction, including request latency, paused-monotonic laptop sleep, frozen pages, exact evidence and capacity-limited admission. |
| Build | Workspace check, headless workspace check, JavaScript syntax and whitespace passed during final implementation verification. A fresh workspace build passed after the browser correction and rebuilt embedded assets. |
| Native runtime | Disposable verified TLS, real ciderd enrollment, two five-second heartbeats, six driver-direction sources learning, and no current source observations after stopping its collector. |
| Review | Independent final review closed all identified code issues. |
| Browser | Desktop and actual 390×844 viewport passed with production assets and clearly synthetic source/finding cases. Exact counters, expanded continuity/policy, source links, independent source-read 503 retention/recovery, local aging and contained mobile table overflow were inspected. Desktop/mobile console logs were empty. |
| Shared spec | Native Google Docs reported “Document status: Saved to Drive.” Fresh DOCX comparison preserved all 1,536 prior nonempty paragraphs and verified one section 20, ten native tables, one Heading 1, seventeen Heading 2, six matching JSON examples and 437 monospace code paragraphs. Native heading/table/code visuals were inspected. |

Evidence logs are under ignored `.codex-staging/`: `detection-workspace-tests-final.log`,
`detection-workspace-check-final.log`, `detection-headless-check-final.log`,
`detection-workspace-build-final.log`, `detection-web-tests-final.log`,
`detection-workspace-build-browser-final.log`, `detection-web-tests-browser-final.log`, and
`detection-native-smoke-final.log`. The initial cross-clock failure is retained in
`detection-cross-clock-regression.log`; the final workspace run verifies its fix.
Published contract structure and all six JSON blocks were validated; full success
DTOs match the compiled synthetic example. No raw telemetry or credentials were
added to the shared section.

The browser fixture had one page, including disabled pagination controls. Frozen
multi-page traversal, authorization and replay semantics were verified by
automated tests; they are not claimed as browser multi-page/auth acceptance.
Browser QA found that active capacity-limited records were labelled admitted.
The corrected label requires both active and supported state; capacity-limited
records now show “Not admitted,” with a regression test and rendered recheck.

Ignored screenshots capture the actual inspected states:
`detection-mobile-evidence.png`, `detection-mobile-source-coverage.png`,
`detection-desktop-source-final.png`, `detection-source-link.png`,
`detection-shared-spec-native.png`, `detection-shared-spec-table.png` and
`detection-shared-spec-code.png`.

Reproducible checks:

```sh
bash scripts/cargo-local.sh test --workspace --locked --no-fail-fast
bash scripts/cargo-local.sh check --workspace --locked
bash scripts/cargo-local.sh check --workspace --no-default-features --locked
node --test web/tests/*.test.mjs
bash scripts/cargo-local.sh build --workspace --locked
node scripts/smoke-detection.mjs
git diff --check
```

The shared [Server Spec](https://docs.google.com/document/d/1JnsZHlYPXQ1IRsMSEHeboICzqqviKJylI7gRsuUK13E/edit?tab=t.bclfm0yxwd6r)
remains authoritative. Section 20 was appended with native headings, tables and
monospace code. Publication evidence is
`.codex-staging/detection-spec-before-resumed.docx`, `detection-spec-after.docx`
and `detection-spec-publication-validation.json`. The comparison also verified
936 added nonempty paragraphs and all 39,229 non-whitespace section characters
against the published HTML/text. The earlier document paragraph prefix is
unchanged. The shared edit used native UI; no connector write or connector
sign-off is claimed.


## Delivery boundary

The requested pre-feature checkpoint is committed as `5a496f1`. Feature changes
remain uncommitted on `codex/storage-activity-detection`; no push, merge, package,
deployment or user-state migration was performed. All diagnostic processes started
for this detection work were stopped, including the resumed synthetic fixture.
Temporary QA tabs were closed and viewport emulation was cleared. The Server
Spec remains open at section 20. An older user-owned storage browser session
was left running unchanged. No browser or shared-spec work remains pending for
this slice. Production-duration efficacy and the broader product features listed
above remain outside the verified boundary.
