# Cider live console

The Rust server embeds and serves this frontend at its own root URL, normally
`http://127.0.0.1:8787/`. Rebuild and restart the server after changing web assets.
Running a separate static development server is not the supported live setup.

1. Start the server with `bash scripts/cargo-local.sh run -p cider-server`.
2. In the native application, choose **Open web console** and **Copy viewer credential**.
3. Paste the viewer credential into the console's connection form.

For headless operation, use `-- --headless`. The current viewer credential is in
`viewer-token` in the server data directory, normally
`~/Library/Application Support/Cider Server`. It has owner-only permissions.
The server reuses this credential across restarts, without a fixed expiration.
To replace it, stop the server and remove or replace its `viewer-token` file
before starting again; browsers holding the old token must reconnect with the
current credential.
After a successful authenticated core read, the browser saves only the read-only
viewer credential in this server origin's local storage (`cider.viewer-token`).
Reloading or reopening the browser restores it and reconnects automatically;
temporary network/server failures retain it while retrying. **Disconnect** removes
the saved token and clears this page's observations. Authentication rejection
also removes it and returns to the connection form. A browser that blocks storage
can still connect for the current page session and shows why persistence failed;
if removal is blocked, the page disconnects and explains how to clear site data.
Administrator and node credentials are never accepted by this connection form or
saved by it. Remote bindings require TLS as before.

The console uses authenticated read routes documented in Server Spec section 16.
Cluster, node, selected-host disk, and attention summaries poll every three
seconds while visible and every thirty seconds while hidden, independently of
optional tables and inventory. Collector heartbeats remain five seconds; a
three-second browser refresh does not promise end-to-end detection within three
seconds.
Reads time out and core failures back off with Retry-After support. Core values
older than 15 seconds are hidden, including immediately after sleep/visibility
changes. A core refresh exceeding 15 seconds cannot receive a new success time;
its pending requests settle before another core poll starts.

Filesystem and event tables fetch only the active view's first 100-row page.
The first filesystem page follows current rows every three seconds while visible
(thirty while hidden), subject to read completion and Retry-After. **Next** pauses
following: Previous/Next then navigate one frozen server snapshot, with accepted
pages cached in memory. **Refresh snapshot** starts a new traversal and resumes
following the first filesystem page. Event evidence remains frozen until manually
refreshed. The table shows its row range, whether more rows exist, following/paused
status, and its snapshot time; no rows are silently truncated.
Automatic filesystem replacement sends `X-Cider-Release-Cursor` with the prior
first page's cursor, releasing only that matching snapshot before allocating its
replacement. Manual refresh and frozen page navigation retain their snapshots.
If replacement fails after release, the dated local rows remain visible and
Next is disabled until an automatic retry or **Refresh snapshot** succeeds.
Periodic complete node and disk traversals also retain their initial snapshot
handle for release on their next first-page read, including after reaching the
terminal page. These handles stay with their API client, route, and query, and
are cleared on disconnect. Release retries tolerate expired or absent snapshots;
Retry-After continues to govern retries. The server allows 240 reads per minute
per credential with a burst of 40; larger traversals and additional tabs can
still encounter the read budget.
Server cursors expire five minutes after snapshot creation; an expired cursor
retains already displayed pages and shows a refresh action. Table errors and slow
page loads do not block core summaries. Security and host event views send their
category/node filters to the server. Event windows cover the hour ending at the
table snapshot, and frozen filesystem fields retain individual states/source dates.

Host storage polls `GET /api/v1/nodes/{node_id}/disks` independently, only for
the selected host. Disk summaries preserve frozen metadata and must arrive within
15 seconds to supply current rates. That budget starts before the first page;
pagination and network delay consume it. Each direction also ages the server
reported source age under its reported stale threshold. Final arrival time is
retained separately for chart positions. The complete host inventory uses 500-row
frozen pages with one traversal at a time, a shared eight-page-per-30-second
budget, core-read priority and Retry-After backoff. A traversal may continue for
up to the server's 300-second cursor lifetime. Partial loading shows its received
object count; grouped storage appears atomically only when the complete inventory
matches the disk summaries' node, boot and topology revision. Structure age
starts with the traversal request, including time spent waiting for further
pages; completion time is shown separately. Both ages use elapsed browser
monotonic/wall clocks and do not compare server dates to the browser clock. A mismatched or
failed replacement retains dated context and hides current disk rates. Unchanged
topology reuses the structure; source observations refresh after 30 seconds or
with **Refresh source observations**. Route changes discard obsolete responses.

Each physical disk has a stable `#node/{node_id}/disk/{object_id}` deep link.
Typed relationships nest containers, volumes and mounts. Pools backed by multiple
disks and their descendants appear once, with references from participating disks.
Physical hardware size, pool capacity, volume usage/quota and mount observations
have separate labels. Virtual, network and unresolved records remain inspectable,
including exact raw metric counters and full reported source objects. Their
observation states are explicitly labeled as frozen snapshot context, with
original source dates. No browser
sum constructs host or cluster capacity. Removed disk selections remain explicit.

The rack pages all enrolled nodes in groups of at most 24, ordered by enrollment
time and node UUID. **Previous hosts** and **Next hosts** are native keyboard
buttons outside the canvas overlay; the host table also includes every node.
Placement is illustrative. The server's explicit `hardware.machine_family`
selects distinct MacBook, Mac mini, iMac or neutral unknown meshes. The browser
does not infer hardware families from model strings or identifiers.

Charts contain observations received in this browser session, without historical
backfill. Selected-disk charts keep at most 180 points, use server-derived rates
and skip retained disk samples. Missing/stale values, failures, long pauses and
identity/boot/inventory/source changes split paths. Source timestamps remain
separate from the monotonic browser arrival axis. There is no fixture fallback
or random metric jitter.

Security displays generated storage-activity findings, source readiness/coverage,
and collector-reported security events in three independent frozen reads. Exact
counter evidence and baseline policy remain fixed while reviewing a page;
observation freshness ages locally across request delays and laptop sleep. Source
admission is separate from availability: only active, supported sources show
“Admitted”; capacity-limited sources show “Not admitted.” The viewer has no
administrator relearn action. See [security implementation and evidence](../docs/storage-activity-detection.md).
General alert delivery, SMS, access management, historical metric queries, SSE, and Prometheus remain outside
the implemented console; no navigation entry advertises those features. APFS/NFS rows are not
additive; aggregate capacity comes from the server's deduplicated summaries.

Pinned Preact/htm, Three.js, and Barlow fonts are vendored and embedded by the
server. The console loads without a public CDN connection. Third-party sources,
checksums, and license notices are in `vendor-licenses/`. Three.js is optional;
host tables work without WebGL. Rack drags orbit without selecting a host, and
a click or touch tap selects the host under that pointer.

Run the browser-independent model tests from the repository root:

```sh
node --test web/tests/*.test.mjs
```

Viewer-session and refresh verification on 2026-09-13: all 153 web tests pass.
Twenty session/refresh regressions cover same-origin saved viewer restoration,
temporary-outage retry, successful-authentication-only persistence, explicit
disconnect and authentication rejection, unavailable browser storage, obsolete
requests and child callbacks, page disposal, visible/hidden polling cadence,
filesystem following versus frozen paging, and Retry-After. These tests run
production components and API/session code with controlled browser storage,
network responses, and timers; they do not claim rendered browser QA.
Five additional HTTP-client regressions cover automatic-only cursor release,
manual traversal retention, failed replacement and disabled stale paging,
first-page-only release headers, and release handles across complete node/disk
traversals and selected-host changes.

Validation on 2026-09-12: all 76 web tests pass, covering rate, counter, and integer gauge
formatting, partial host/device coverage, availability, chart scaling and gaps,
same-origin client behavior against a local HTTP server, pagination retries,
empty/error inventory handling, route decoding, snapshot freshness/sleep expiry before and after arrival, retry timing, frozen cursor paging, route isolation,
independent core/table loading, rack pointer gestures, field-specific filesystem states,
descriptive inventory labels/properties, canonical storage graph/shared-pool uniqueness,
cycles and unassigned sources, frozen metadata consistency, generation/boot/route
races, shared topology budgets and TTL/429 handling, disk history continuity,
render-time joins and removal states, traversal-start freshness/source aging,
clock-skew-safe structure age after budget delays, and 57-host rack paging. JavaScript syntax
checks pass. These automated checks
do not substitute for rendered views and authenticated browser interaction QA.
The workspace review records current Rust and browser verification separately.

Storage-activity verification on 2026-09-13: all 97 web tests passed after the
admission-label fix, and a fresh workspace build embedded the final assets.
Rendered desktop and 390×844 mobile QA passed with clearly synthetic one-page
cases served using production assets: exact/expanded evidence, source links,
independent source-read errors with retained evidence/recovery, local aging,
disabled pagination controls and contained table overflow. Multi-page pagination
and authorization are covered by automated tests, not a browser multi-page run.
Server Spec section 20 was published through native Google Docs UI and verified
by Saved to Drive plus DOCX comparison; no connector write/sign-off is claimed.


For the bounded native storage smoke, build the workspace binaries first:

```sh
cargo build --workspace --locked
node scripts/smoke-storage.mjs
```

This Apple Silicon macOS check uses OpenSSL, verified private TLS, explicit
collector enrollment, disposable loopback state and the real local collector.
It cleans up only its own processes/state and waits through degraded/offline
thresholds. The verified run observed one physical disk, one resolved driver,
ten linked local mounts, coherent disk/inventory topology and all embedded
storage modules. Empty-cluster and stopped-collector aggregates remained Unknown.
This command does not perform browser interaction QA, disk capability benchmarking
or acceptance on two physical Macs. See the [integration checkpoint](../docs/disk-model-integration.md)
for the 154-test Rust result, completed native/browser checks and remaining
browser, hardware and shared-specification gates. A separately requested browser
test session may intentionally remain running; it is not owned by this smoke.
