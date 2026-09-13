# Cider sessions, refresh, and disconnect observations

Implementation record for the 2026-09-13 refinement. Work started after rebasing
the existing USB-alert commit onto main at `925e178`; the rebased commit is
`4bb51db`. The refinements themselves remain local and uncommitted.

The shared Server Spec is authoritative. Its native Chrome session was stopped
during this work, and no Google Docs connector was available. The following
contract notes are ready to transfer but are **not a shared-spec update**.

## Restart and rename behavior

`ciderd enroll-server` remains a one-time enrollment. Subsequent starts use
`ciderd run --config /absolute/path/ciderd.toml` with the same saved node identity,
credential, state directory, endpoint, and trust configuration. Node credentials
and revocation remain in the server database; collector session and generation
boundaries remain intact. A restart does not consume another enrollment token
or create another node. Rejected credentials do not trigger automatic enrollment.

The server loads the owner-only `viewer-token` file instead of replacing it on
startup. Existing generated viewer credentials remain valid. Viewer credentials
have no scheduled expiry; to rotate one, stop the server, remove its viewer-token
file, and restart so it creates a fresh token. Existing browsers then receive
401 until connected with the replacement. A corrupt or unsafe file prevents
startup instead of silently changing the credential. Tokens are never logged.

The browser stores only a successfully authenticated viewer credential under
`cider.viewer-token` in that server origin's localStorage. Reload restores it;
temporary server outages retain it and retry. Disconnect or HTTP 401/403 clears
it. Administrator and node credentials are never stored. If browser storage is
blocked or full, the console explains that it cannot remember the token.

New installations use `Cider Server`, `cider.sqlite3`, and the `cider-server`
binary. Existing `Orchard Server` directories and `orchard.sqlite3` databases are
reused without moving/copying them. Ambiguous simultaneous old/new paths fail
explicitly. Command-line options take precedence over `CIDER_*` variables, which
take precedence over legacy `ORCHARD_*` variables. The separate NFS helper emits
Cider managed-export markers but recognizes its existing legacy markers; mixed
or duplicate blocks are rejected. No system export or mount is changed by tests.

## Refresh timing

The example collector configuration schedules physical inventory, mount
inventory, IOKit I/O/USB presence, and NFS mount status every three seconds, with
no jitter. Existing deployments must update these fields in their saved config:

```toml
[collection]
io_seconds = 3
mount_inventory_seconds = 3
nfs_status_seconds = 3
inventory_reconcile_seconds = 3
```

This fragment supplements the complete example configuration; it is not a
standalone configuration. Heartbeats remain five seconds. Discovery completion,
worker contention, heartbeat delivery, and multiple-confirmation alert rules
can make end-to-end detection take longer than three seconds.

Visible core, disk, attention, and first-page filesystem refreshes use three
seconds. Background polling remains thirty seconds. Paging freezes a filesystem
snapshot until Refresh snapshot; it never mixes cursor pages across refreshes.
Backoff and Retry-After continue to apply.

## API notes for Server Spec sections 16–17 and 22–23

`GET /api/v1/capabilities` accepts `Authorization: Bearer <viewer-token>` or the
administrator credential, with no query or request body. Node credentials are
forbidden. The existing response envelope contains `data` and `meta`;
`meta` contains `api_version`, `server_time`, `request_id`, `snapshot_cursor`,
and nullable `next_cursor`. The following data fields change or clarify:

| Field | Type | Value and meaning |
| --- | --- | --- |
| `viewer_credential_expires_at` | string or null | `null`: no scheduled expiry; explicit replacement still invalidates the old credential |
| `viewer_scope` | string | `telemetry:read` |
| `browser_transport` | string | `same_origin` |
| `poll_interval_seconds` | integer | `3` |
| `hidden_poll_interval_seconds` | integer | `30` |
| `heartbeat_interval_seconds` | integer | `5` |

Other capability fields retain their existing contract. Status codes remain
200 on success, 400 for unsupported/duplicate query parameters, 401 for absent
or rejected credentials, 403 for a node credential, 429 for exhausted read
budget, and 503 for unavailable server state. JSON examples below are excerpts
from `data`, not complete response objects:

```json
{"viewer_credential_expires_at":null,"viewer_scope":"telemetry:read","browser_transport":"same_origin","poll_interval_seconds":3,"hidden_poll_interval_seconds":30,"heartbeat_interval_seconds":5}
```

`POST /api/v1/nodes/enroll` and `POST /api/v2/ciderd/heartbeat` keep their routes,
authentication, request/response fields, status codes, idempotence, and five-second
heartbeat contract. Persistence changes neither credential scope nor revocation.

Disconnect refinements use existing `GET /api/v1/attention`,
`GET /api/v1/attention/summary`, `GET /api/v1/diagnostics`, and
`GET /api/v1/nodes/{node_id}/disks`. Existing viewer/admin authentication,
pagination, filters, status codes, and error envelopes still apply. No new
remediation or notification-send endpoint is introduced. The companion
disconnect review documents the added evidence fields and lifecycle behavior.

## Verification

Reproducible commands for the final implementation:

```sh
bash scripts/cargo-local.sh test --workspace --locked --no-fail-fast
bash scripts/cargo-local.sh check --workspace --locked
bash scripts/cargo-local.sh build --workspace --locked
node --test web/tests/*.test.mjs
node scripts/smoke-restarts.mjs
bash scripts/cargo-local.sh test --manifest-path simple-nfs-server/Cargo.toml --locked
git diff --check
```

The restart smoke uses disposable loopback verified TLS, one initial enrollment,
real local ciderd observations, and separate server-only, collector-only, and
combined restarts. It removes its temporary credentials and state. It does not
unplug drives, disrupt NFS, or send SMS. Browser visual QA and real Twilio handset
delivery are not established by these source/API tests.

## Automatic refresh cursor release and read limits

Paginated reads now accept the optional `X-Cider-Release-Cursor` request header
on a new first-page request. The filesystem follower sends its previous
`meta.next_cursor` when automatically replacing its live first page. The header
does not create a route or change the response envelope, collection fields,
authentication, or filters. Manual refresh and frozen page navigation omit it.

For example, the next automatic refresh after a paginated filesystem response
uses the same origin and bearer credential:

```http
GET /api/v1/filesystems?limit=100 HTTP/1.1
Authorization: Bearer <viewer-token>
X-Cider-Release-Cursor: <previous-meta.next_cursor>
```

The server authenticates the viewer or administrator and applies its read budget
before checking the header. The header must appear once and must not accompany
a `cursor` query parameter. A cached cursor can be released only when its owner,
route, parsed query parameters, expiry, and page boundary match the new request.
Query parameter order is immaterial; parameter values, filters, and the presence
and value of `limit` must match exactly. A well-formed cursor whose snapshot has
expired or is no longer cached is a no-op, allowing refresh to recover after
server restart or a failed prior request. A malformed, duplicate, foreign, or
mismatched release cursor returns 400 without releasing the cached snapshot.

Release happens before the replacement snapshot is built or allocated. If that
refresh fails, the browser keeps its prior rows as dated local reference; the
released server cursor is no longer usable. Requesting a released cursor through
normal pagination returns 410. A successful replacement returns the existing
200 collection envelope with its new nullable `meta.next_cursor`; ordinary
401/403 authentication errors, 429 budget/cache limits, and 503 unavailable-state
errors continue to apply. Other cached manual or frozen traversals are retained.

Read authentication now allows 240 requests per minute per credential, with a
burst of 40 and continuous replenishment at four requests per second. This
supports two visible console tabs polling core summaries, attention, and one
filesystem or disk view every three seconds when each regularly polled collection
fits one page. Larger traversals and additional tabs still honor 429 and backoff.
The existing 64 concurrent-request
permits and pagination bounds of 128 snapshots, 32 MiB, and five-minute expiry
remain unchanged. `GET /api/v1/capabilities` exposes the actual values:

```json
{"limits":{"default_page_size":100,"maximum_page_size":500,"cursor_ttl_seconds":300,"read_requests_per_minute":240,"read_burst":40}}
```

Targeted regression coverage exercises repeated automatic replacement while a
manual snapshot stays available, rejection without deletion for foreign or
mismatched cursors, expired/missing-cursor recovery, and advertised/enforced read
budgets. These are implementation notes awaiting transfer to the shared Server
Spec; they do not establish a successful shared-document write.
