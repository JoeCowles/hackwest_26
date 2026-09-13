# Operator attention and SMS delivery

Orchard records concerns for an administrator to assess. Acknowledging a concern
records an operator note; it does not resolve the source, rebaseline an activity
detector, repair a filesystem, or disconnect a node. This module does not predict
hardware failure dates. Capacity warnings use observed utilization (warning at
90%, critical at 95%), not a three-day prediction.

## Evidence and episodes

Attention combines existing activity and reliability findings, independently
selected capacity-bearing objects, lost node observations, and passive filesystem
observations supplied by the read layer. Source finding IDs and evidence remain
available in each episode. When a coherent shared-filesystem observer changes,
the existing episode keeps its identity and updates its node/object attribution. Capacity inputs reuse physical-backing and coherent
shared-observer selection; no new capacity summation occurs here.

An episode has an immutable ID and source key. Repeated observations update its
evidence without generating another notification. Explicit fresh recovery closes
it; recurrence creates a new ID. Missing, interrupted, stale, or unavailable
observations cannot establish recovery. A source that disappears becomes unknown
while its last evidence is preserved. A node is considered lost after 90 seconds
without a heartbeat, or after an explicit goodbye/revocation; observations younger
than 30 seconds establish recovery when their timestamps are not in the future. Future heartbeat evidence is unknown. Active episodes take precedence over older resolved rows even after clock rollback. This is loss of observation, not proof that
its drives failed. The collector heartbeat interval remains five seconds.

Explicit schema-2 removal of a previously observed physical drive now opens a
warning episode (`drive_removal`) in the same transaction as inventory ingestion.
It enters the existing notification queue when SMS is enabled. Repeated tombstones
and retried heartbeats do not create duplicate notifications. Omitted upserts,
IOKit driver removal, and cleanup of inventory from an earlier boot do not trigger
this alert. Both planned ejection and unexpected unplugging are reported: the
collector cannot distinguish operator intent. The confirmed removal remains a
recorded event even when later observations are missing. An explicit reappearance
of the same resource identity resolves the episode; another removal creates a new
one. A changed resource identity requires administrator review of the older
concern. Resource reappearance does not establish physical hardware continuity or
health. Only removals received after this server update can create these alerts;
past removals are not reconstructed. Deploy the updated server on the receiving
host, and configure Operations → Notifications for SMS delivery.

A revision guards acknowledgement. Acknowledgement changes the revision and
suppresses any still-queued notification and pending admission intent. A fresh severity transition clears the
acknowledgement and receives a new revision. An already in-flight SMS cannot be
recalled by acknowledging or changing settings. Source dates remain in evidence;
attention first_seen_at/updated_at are central-server episode timestamps.

Worker freshness is separate from source evidence. summary.reconciliation and
list metadata.reconciliation expose unknown before any successful evaluation,
current for a success within 15 seconds, stale after that window (or a backward
clock jump), and failed after an unsuccessful attempt following a prior success.
The last successful timestamp changes only in the successful reconciliation
transaction. last_attempted_at and a generic reconciliation_failed code identify
failures without exposing internal error text. A successful later run clears the
failure. Each row has reconciliation_state while retaining its source
observation_state; a new HTTP response timestamp does not make the worker current.

The bounded store retains at most 10,000 episodes and 1,000 notification records.
Resolved episodes and terminal notification records become eligible for deletion
after 30 days. At the 1,000-record notification bound, the oldest terminal or
suppressed history record can be evicted early to admit a current notification.
Queued, sending, and accepted records are never evicted for admission. Open concerns are not silently discarded. When admission is full,
settings expose admission_limited=true; new entries may be omitted. A list read
is limited to 10,000 matched rows and 32 MiB of stored evidence/summary bytes,
returning attention_limit (503) when the selection exceeds that budget. The root
HTTP layer freezes and pages that result; the module does not truncate it to the
requested page size.

## Integration contracts

The following public functions return JSON data, without HTTP envelopes. The
root router supplies authentication, authorization, replay protection, frozen
paging, and the usual data/meta envelope. HTTP route verification is documented
by the root integration task, separately from these module tests.

- attention::reconcile(&AppState, now_ms) runs once per five-second worker cycle.
  It gathers read/diagnostic inputs before taking the database writer guard.
- attention::list(&AppState, &BTreeMap<String,String>, now_ms) returns (rows,meta).
  Filters: node_id, object_id, status (open/resolved/all), kind
  (activity/reliability/capacity/node_loss/filesystem/drive_removal), severity
  (info/warning/critical), acknowledged (true/false); limit validates 1–500 for
  the root paginator. Unknown filters are rejected (400).
- attention::summary(&AppState, now_ms) returns counts, notification status, and
  reconciliation freshness.
- attention::record_reconcile_failure(&AppState, now_ms) records a generic failed
  attempt without modifying the last successful timestamp. The root five-second
  worker invokes it if reconcile returns an error.
- attention::acknowledge(&mut Transaction, id, revision, note, actor, now_ms)
  returns the updated episode. Missing IDs are 404; stale revision or resolved
  episode is 409; notes over 2,048 bytes are 400. Actor is stored as a hash.
- notifications::settings(&AppState) returns masked settings.
- notifications::configure(&mut Transaction, &request, actor, now_ms) returns
  masked settings; invalid fields are 400 and stale expected_revision is 409.
- notifications::run(AppState, CancellationToken) runs the bounded sender and
  delivery polling worker. Exactly one notification worker runs per AppState.

The root router implements GET /api/v1/attention, GET
/api/v1/attention/summary, POST /api/v1/attention/{id}/acknowledgement, and GET/PUT
/api/v1/notifications/settings. Reads require Authorization: Bearer with a valid
viewer or administrator credential. Mutations require an administrator Bearer
credential plus X-Request-ID (fresh UUID) and X-Request-Timestamp (RFC3339 within
300 seconds of server time). Valid viewer/node credentials receive 403 for these
administrator mutations; missing/invalid authentication is 401. Replayed request
IDs receive 409 request_replayed. Successful operations return 200 with data/meta;
malformed payload/filter fields are 400, missing episodes 404, stale revisions
409, and selected attention data exceeding bounds 503 attention_limit.

The acknowledgement body is:

```json
{"expected_revision":"revision-uuid","note":"Investigating this observation"}
```

It returns the updated episode and a new revision; the note is required (empty
is allowed), and unknown fields are rejected. This module document records the
inspected router contract; root owns integrated HTTP/browser validation and the
shared Server Spec. It does not claim a shared-document write.

A representative row (IDs abbreviated for readability):

```json
{
  "id": "episode-uuid", "source_key": "node_loss:node-uuid",
  "kind": "node_loss", "node_id": "node-uuid", "object_id": null,
  "status": "open", "severity": "critical", "observation_state": "ok",
  "reconciliation_state": "current",
  "summary": "Node observation lost; storage condition unknown",
  "evidence": {"last_seen_at":"2026-09-13T12:00:00.000Z","age_ms":95000,
    "goodbye":false,"revoked":false,"offline_after_seconds":90},
  "revision": "revision-uuid", "first_seen_at": "2026-09-13T12:01:35.000Z",
  "updated_at": "2026-09-13T12:01:35.000Z", "resolved_at": null,
  "acknowledgement": null,
  "notification": {"state":"accepted","attempts":1,"provider_status":"queued",
    "reason":null,"updated_at":"2026-09-13T12:01:40.000Z"}
}
```

The source observation state describes the evidence for the concern: an "ok"
node-loss observation does not mean storage is healthy. Notification details are
attached by list(); a direct acknowledgement result need not include that join.

Summary data:

```json
{
  "total": 3, "open": 2, "unacknowledged": 1,
  "notifications": {
    "enabled": false, "sender": "***0100", "recipient": "***0101",
    "revision": "settings-revision-uuid", "credential_state": "configured",
    "worker_seen_at": "2026-09-13T12:01:40.000Z", "admission_limited": false,
    "updated_at": "2026-09-13T12:00:00.000Z"
  },
  "delivery": {"suppressed":2,"accepted":1},
  "reconciliation": {"state":"current",
    "last_successful_at":"2026-09-13T12:01:40.000Z",
    "last_attempted_at":"2026-09-13T12:01:40.000Z",
    "error":null,"stale_after_seconds":15},
  "observed_at": "2026-09-13T12:01:40.000Z"
}
```

credential_state is not_loaded before the worker runs, configured after valid
credential syntax is loaded, or missing_or_invalid. Configured does not prove
Twilio accepted the credentials. Delivery counts cover retained outbox records,
not the number of physical handsets that received messages.

## Dashboard configuration

Private credentials are loaded once when the notification worker starts, from
the process environment with fallback to the current working directory's .env.
The dotenv iterator does not mutate the multithreaded process environment.
TWILIO_ACCOUNT_SID and TWILIO_SECRET use account SID/Auth Token authentication.
TWILIO_SID is also accepted as an AC account alias or an SK API key; an SK key
requires TWILIO_ACCOUNT_SID and uses TWILIO_SECRET as its API key secret. Credentials
never enter SQLite, browser responses, SMS bodies, or diagnostic logs.
TWILIO_FROM may initialize an untouched disabled sender setting. Restart the
worker to reload credentials. No supplied secret values were inspected for this
implementation.

Sender, recipient, enabled state, revision, timestamp and hashed actor are stored
in the private application SQLite database. Viewer responses expose only the
last four phone digits. Both phones must be E.164 (+ followed by 2–15 digits,
nonzero first digit). The service does not verify ownership or SMS reachability.
The sender must be usable by the Twilio account; trial/account restrictions are
reported through provider failure state.

Example administrator configure data (use an actual authorized destination only
when enabling live notifications):

```json
{
  "expected_revision": "initial", "enabled": true,
  "sender": "+15555550100", "recipient": "+15555550101"
}
```

The response is the masked settings object shown above. Omit sender/recipient to
preserve their current values; an empty string clears a phone only when disabled.
Never send the displayed mask back as a replacement phone. Changing settings
suppresses queued records addressed under the old revision. It cannot cancel an
in-flight send. Enabling affects future new/severity-transition concerns; it does
not replay concerns first recorded while disabled. Missing credentials leave
eligible alerts queued and visibly undelivered until their 24-hour expiry.

## Outbox and Twilio protocol

The worker uses the official HTTPS Messages REST resource and HTTP Basic auth.
It posts URL-encoded From, To and Body to
https://api.twilio.com/2010-04-01/Accounts/{AccountSid}/Messages.json and polls
https://api.twilio.com/2010-04-01/Accounts/{AccountSid}/Messages/{MessageSid}.json.
SMS text contains only an attention ID and instruction to inspect the dashboard;
raw telemetry, phone configuration, and secrets are not included.

A full active outbox retains one durable intent per episode instead of dropping
the transition. The episode's notification_intent is null or an object with
state, transition_revision, settings_revision, created_at, updated_at, and reason.
Dates are RFC3339. While waiting, state is pending_admission and reason is
outbox_full; list notification also projects this state with zero attempts.
Summary delivery.pending_admission counts these intents; admission_limited stays
true while any are waiting. Oldest pending intents receive freed slots before
new reconciliation transitions; admission is idempotent. The original transition
time remains the queue age. ACK, fresh recovery, superseding revisions and settings
changes suppress pending intent. After 24 hours it becomes failed/admission_expired.
Suppression/expiry remains visible on the episode until another notification
transition replaces it. Disabled transitions cannot become a pending backlog.
The extra durable storage is bounded by the 10,000 episode limit.

State transitions:

- queued: eligible and waiting for configuration/rate limit/backoff.
- sending: durably claimed before HTTP; at most one new send per 30 seconds,
  enforced by a separate persisted last_send_claim_at timestamp.
- accepted: valid Message SID and provider queued/accepted/sending/sent response;
  it does not prove handset delivery.
- delivered: explicit provider delivered response for the expected SID.
- failed: explicit terminal rejection/failure, four exhausted send attempts, or
  24-hour queued expiry. No automatic terminal-failure replay.
- uncertain: ambiguous send transport/response, unknown provider status, crash
  during sending, or no final delivery outcome within 24 hours. Never silently
  retries an uncertain send.
- suppressed: disabled, acknowledged, resolved, or obsolete settings revision.

Only explicit HTTP 429 send rejection retries, with exponential 60/120/240-second
backoff and four total attempts. HTTP 5xx, connection failures and malformed or
truncated success responses are uncertain because server-side acceptance is
ambiguous. Redirects are disabled. Connect timeout is five seconds; total request
timeout is ten seconds; responses are capped at 16 KiB. Error strings and bodies
from the provider are not logged or persisted.

Accepted records are polled no more often than once a minute per record. A polling
429/transport/rejection does not discard its SID or queue a second send; it stays
accepted with a reason until the deadline. The worker handles one request per
five-second cycle. A due queued send gets priority once the independent 30-second
send interval permits it; other cycles poll delivery status. Poll timestamps cannot
postpone eligible sends. A large accepted backlog can delay individual delivery
checks.
No webhook or public callback endpoint is required. Startup converts every
in-flight sending row to uncertain. Each subsequent serial worker step also converts a previous orphan claim to uncertain/outcome_not_recorded, including when a successful HTTP response could not be saved. It never retries that ambiguous send. HTTP runs after committing the claim and
releasing the SQLite transaction and writer lock.

Official references: [Twilio Messages resource](https://www.twilio.com/docs/messaging/api/message-resource),
[HTTP authentication](https://www.twilio.com/docs/usage/requests-to-twilio), and
[rate-limit retry semantics](https://www.twilio.com/docs/api/errors/20429).

## Verification boundary

Module tests exercise durable recurrence/acknowledgement, stale and disappearing
observations, fresh diagnostic recovery, node-loss deduplication, frozen paginator
handoff, settings validation and masking, accepted versus delivered, retry bounds,
ambiguous/truncated responses, writer-lock release during HTTP, polling rate-limit
SID preservation, redirect/response bounds, credential aliases, crash recovery,
reconciliation success/failure/stale states, full active outbox admission/retry/cancellation, accepted-response database failure, clock rollback, and read snapshot isolation across a concurrent oversized evidence update. Twilio HTTP tests use loopback
fixtures with invented credentials and destinations. No real SMS was sent and no
live Twilio account authentication or handset delivery was verified. Shared Google
Doc, complete HTTP authorization, and browser interaction validation belong to
the root integration task.

```sh
CARGO_INCREMENTAL=0 cargo test -p orchard-server --no-default-features --test attention_state --test notification_settings
CARGO_INCREMENTAL=0 cargo test -p orchard-server --no-default-features --lib notifications::tests
```
