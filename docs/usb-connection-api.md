# 23. USB connection presence and negotiated-link concerns

Implemented on 2026-09-13. This section supplements the surrounding collector and
operator contracts; implementation verification is recorded below.

Orchard observes an identified USB storage connection disappearing or negotiating
below its previously confirmed speed. The collector supplies connection evidence;
the server maintains durable watches and uses the existing Attention/SMS pipeline.
These observations do not establish damaged media, predict failure, distinguish
intentional unplugging, or trigger remediation. Five-second heartbeats remain
unchanged. Sections 17, 19 and 22 retain the surrounding contracts.

## 23.1 Routes, authentication and response context

`POST /api/v2/ciderd/heartbeat` accepts the optional collection extension below
inside an otherwise valid schema-2 heartbeat. It requires an active node Bearer
credential bound to the payload node_id, application/json and the normal verified
HTTPS collector transport. Viewer/administrator credentials do not replace the
node credential. No new endpoint or request header is introduced. The enclosing
request retains schema_version, message_type, node_id, boot_id, agent_session_id,
agent_generation, sequence, created_at, clock_id, monotonic_ns, agent, inventory,
resources, relationships, collections, collector_states, events and tombstones,
with their original types and limits. Collection acquisition IDs, clocks, dates
and statuses govern the extension; inventory revision dates do not refresh it.

Success is the existing 200 schema-2 acknowledgement: schema_version="2.0",
agent_session_id, accepted_sequence (exact decimal string), inventory_revision
(nullable exact decimal string), request_inventory (boolean), and
server_received_at (RFC3339). It is committed with the receiver, watch state and
Attention/outbox changes. An identical session/sequence replay
returns its existing acknowledgement without advancing a watch. Reusing that
sequence with different content returns 409. Other statuses retain their existing
meanings: 401 invalid/missing/revoked node credential; 403 wrong node; 413 body
limit; 415 wrong content type; 422 invalid schema-2 payload; 429 admission pressure;
500 internal error. A malformed, unsupported, stale or partial optional USB
snapshot makes the watch unavailable while otherwise valid telemetry can still
be admitted. It never becomes a complete empty scan.

`GET /api/v1/nodes/{node_id}/disks` returns existing disk rows with nullable
device_watch. It accepts the existing current-generation and limit/cursor paging
parameters. `GET /api/v1/nodes` and `GET /api/v1/nodes/{node_id}` expose node watch
coverage. `GET /api/v1/attention` exposes connection episodes through the existing
kind=reliability filter. No USB-specific query parameter is added. Node/disk and
Attention reads require an unexpired viewer or administrator Bearer; active node
credentials cannot read the operator API.

Read responses retain `{data,meta}` with api_version, server_time, request_id,
snapshot_cursor and next_cursor. Exact bitrates are decimal strings, dates are
RFC3339, nullable evidence remains unknown, and responses use no-store. Lists use
the existing limit=1..500 (default 100), opaque cursor, 300-second frozen traversal,
120 reads/minute and burst 20. Refresh the initial route for new observations.
Existing read statuses apply: 200 even when evidence is unavailable; 400 invalid
or repeated query fields; 401 credential failure; 403 scope failure; 404 unknown
resource; 410 expired cursor; 429 rate limiting; 503 bounded-selection unavailable;
500 internal error. Existing error envelopes and Retry-After behavior are unchanged.

The existing `POST /api/v1/attention/{episode_id}/acknowledgement`,
`GET /api/v1/notifications/settings` and `PUT /api/v1/notifications/settings`
remain the operator controls. Section 22 specifies their full fields, administrator
mutation authentication, transport UUID/timestamp, expected revisions and statuses.
The USB feature adds no configuration or mutation route.

## 23.2 Collector extension

Only a host-scoped `iokit.block` collection publishes
`extensions.usb_device_snapshot`. Its fields are version (integer 1), complete
(boolean) and devices (array). At most 128 devices and 65,536 encoded bytes are
allowed. Objects reject unknown fields; identities and driver references must be
unique. A successful empty enumeration has complete=true and devices=[]. Old
collectors without the extension are unsupported. Failed/ambiguous/bounded scans
cannot prove absence, even if they retain some positive observations.

Each device contains identity (canonical non-nil opaque UUID), identity_basis
(reported_usb_serial or boot_registry), identity_scope (usb_enclosure or
driver_incarnation respectively), driver_resource_id (current graph reference),
bsd_name (nullable proven whole-media diskN locator), negotiated_bps (nullable
canonical positive u64 decimal string), speed_state (available, unknown or
unsupported), and reason (nullable 1–128 ASCII letter/digit/underscore/period/hyphen
explanation code). Driver references are 1–512 ASCII graphic bytes; whole-media
locators are disk plus 1–60 digits. Available speed
requires a bitrate; unavailable/unsupported speed requires null. A driver reference
must identify the current node's IOBlockStorageDriver controller with driver scope.

Example extension inside a successful host collection (synthetic identifiers):

```json
{"usb_device_snapshot":{"version":1,"complete":true,"devices":[{"identity":"11111111-1111-4111-8111-111111111111","identity_basis":"reported_usb_serial","identity_scope":"usb_enclosure","driver_resource_id":"22222222-2222-4222-8222-222222222222","bsd_name":"disk7","negotiated_bps":"5000000000","speed_state":"available","reason":null}]}}
```

Native acquisition walks unique IOService parents to the nearest IOUSBHostDevice,
bounded to 32 levels and 4,096 ancestor visits per acquisition. Ambiguous, cyclic,
invalid or changed iterators yield incomplete coverage. Direct whole-media evidence
remains distinct from the USB ancestor. USB lookup failure preserves usable
independent driver counters. The native version-2 envelope distinguishes a complete
empty scan from a legacy unsupported array.

USBSpeed uses Apple's IOUSBHost connection-speed enum: 1=12 Mb/s, 2=1.5 Mb/s,
3=480 Mb/s, 4=5 Gb/s, 5=10 Gb/s and 6=20 Gb/s. Zero, missing and malformed codes
are unknown; other codes are unsupported. bcdUSB and configured/forced link values
are not substituted for observed negotiated speed. Bitrates are transport evidence,
separate from sampled read/write bytes per second.

Reported USB serial plus vendor/product IDs derive a node-scoped opaque enclosure
identity using the existing namespaced UUID mechanism. Raw serials stay inside the
private native worker/parser boundary and are absent from the published snapshot,
resources, metrics and diagnostic text. Reported identity does not verify installed
media or uniquely authenticate hardware. Missing/invalid identifying properties
use a boot/registry identity; duplicates or ambiguous multiple drivers make the
snapshot partial. Temporary identity changes on a still-present driver are unknown
continuity, not evidence that its previously identified connection disappeared.

## 23.3 Durable policy and evidence

Only distinct successful complete acquisitions in the current heartbeat clock,
boot, generation and session advance the state machine. Source age must be at most
15 seconds, and acquisitions must be at least one source second apart. Repeated
collection IDs/monotonic acquisitions do not advance streaks or refresh dates.
Gaps exceeding 15 seconds, stale/partial/failed evidence and context changes break
pending streaks. A new collector session must establish presence before a new loss
can be declared. Missing evidence never resolves an existing concern.

Two positive observations arm presence. Two subsequent consecutive complete fresh
absences open one warning. Two positive observations of the same identity resolve
it. Weak boot/registry identity supports presence loss only for that incarnation;
it cannot claim recovery through a different registry identity. A deliberate and
an accidental unplug produce the same observed absence.

For reported enclosure identity, two equal speed observations confirm a baseline.
The highest confirmed speed is retained across registry changes and server restart;
one high outlier cannot raise it. Two observations below the confirmed baseline
open a link warning; two at or above it recover. A connection first observed at
480 Mb/s establishes a 480 Mb/s baseline and does not warn solely for being slow.
Missing speed or an absent connection leaves an existing link episode unresolved,
with current assessment unknown. Weak identity may expose a reported bitrate but
cannot establish a baseline comparison across reconnects.

Both rules produce kind=reliability Attention episodes with stable source keys
device_presence:<watch_id> and usb_link:<watch_id>. Evidence carries policy_version,
rule_id/classification, opaque identity/basis/scope, driver/object attribution,
nullable BSD locator, presence/arming/streak counts, acquisition ID/date, required
observation count/spacing, baseline/current link evidence and reason. Each link
sample retains exact negotiated_bps, collection_id, observed_at, driver_resource_id
and object_id. Source observation metadata ages independently of Attention's
evaluation timestamp. These are connection concerns, separate from statistical
service-time and SMART findings at `/api/v1/reliability/findings`.

Existing acknowledgement, recurrence and notification policy applies. Repeated
observations retain the same episode and do not enqueue duplicate sends. Fresh
recovery resolves it; later recurrence can open another episode. Acknowledgement
suppresses queued notification while leaving the condition open. Disabled SMS is
recorded as suppressed and enablement does not replay disabled history. Provider
acceptance is not handset delivery. The existing one-send-per-30-second worker
limit and queue/provider delays apply in addition to the two-sample detection time.

## 23.4 Disk projection and node coverage

The disk device_watch is present only for one uniquely attributable current driver
association. It is null for unsupported/unattributable/ambiguous watches. Object
existence or node availability alone is not proof of current USB presence. A disk
can have current USB evidence even when throughput is unavailable.

Projection fields are watch_id, identity_basis, identity_scope, presence
(present/absent/unknown), armed (boolean), link_state
(warming_up/no_current_warning/warning/unknown/unsupported), negotiated_bps,
baseline_bps, reason, observation and evidence. Observation contains state,
observed_at, received_at, age_seconds and stale_after_seconds=15. Read-time aging,
owner loss and incompatible acquisition context make current presence/link status
unknown. The browser adds elapsed snapshot time and refresh-failure state; cached
or sleeping tabs cannot keep evidence current indefinitely. Dated baseline/evidence
can remain visible after current status becomes unknown.

Example disk projection (synthetic):

```json
{"watch_id":"33333333-3333-4333-8333-333333333333","identity_basis":"reported_usb_serial","identity_scope":"usb_enclosure","presence":"present","armed":true,"link_state":"warning","negotiated_bps":"480000000","baseline_bps":"5000000000","reason":"below_previously_confirmed_link_speed","observation":{"state":"current","observed_at":"2026-09-13T12:00:00Z","received_at":"2026-09-13T12:00:01Z","age_seconds":1,"stale_after_seconds":15},"evidence":{"policy_version":"1"}}
```

Example request: `GET /api/v1/attention?kind=reliability&status=open&node_id=00000000-0000-4000-8000-000000000001`.
The operator can read a removed connection's durable Attention evidence after its
disk row leaves active inventory. The disk fallback links to Attention.

NodeSummary.device_watch exposes state, reason, observation, admission_limited,
watched_devices, maximum_devices_per_node=128 and maximum_devices_global=8192.
State is current, partial, stale, unavailable, unsupported or unknown. Partial
means that a current acquisition could not admit all new watches within the
retention bounds. Observation uses the same state/date/age fields above; it is
unknown before source evidence exists. A current complete empty scan can report
zero watched devices. Retained watch count does not mean every device is currently
present. Node coverage makes admission limits visible even when no new disk can
receive a watch.

## 23.5 Persistence and verification

SQLite migration 006 raises the schema version to 6 and persists node acquisitions
and watched devices. An older server refuses this newer database. Watch state is
bounded to 128 devices per node and 8,192 globally, with bounded row JSON and explicit
admission limitations. Unresolved state is not silently evicted to admit a new
device. This is monitoring state, not a new secret store.

Verification on 2026-09-13: 345 Rust workspace tests passed, with three existing
subprocess helpers and the explicit native USB probe excluded from the default
suite. The native USB probe was run separately and passed. Workspace check,
headless workspace check, default workspace build, 128 web tests, JavaScript syntax
and diff checks passed. The static contract validator passed 85 metric definitions
and 21 invalid-input cases. Nineteen authenticated connection-watch tests cover
transitions, persistence, rollback, replay, source age/clock changes, identity
ambiguity and admission bounds. Independent review found no remaining blocker
in its scoped review.

The final built server passed the verified-TLS smoke with a real passive collector
and synthetic USB loss/replug, slower-link and recovery observations. It checked
receipt idempotence, viewer disk projection, driver replacement mapping, unknown
values, removed-disk Attention retention and disabled notification suppression.
Rendered Browser checks covered seven production-panel scenarios, snapshot aging
and a 390-pixel mobile layout; console warnings/errors and horizontal overflow
were absent in those checks. The browser observations were synthetic. No real SMS
was sent. Physical unplug, slower-link and recovery checks also passed on the local
demonstration enclosure. Intel/second-Mac behavior, real Twilio authentication/handset
delivery, packaging and deployment remain separate acceptance gates. Native Google
Docs publication and export comparison are tracked
separately from these software checks; no connector write is claimed.
