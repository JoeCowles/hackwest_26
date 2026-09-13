# Cider integration review — 2026-09-12

This review covers the collector, HTTP server, persistence, API projections,
embedded console, desktop startup/lifetime, smoke tooling, and the separate
NFS helper. It preserves the earlier uncommitted metrics corrections. Nothing
was committed, pushed, deployed, or packaged as part of this review.

## Architecture and boundary contracts

`ciderd` owns native macOS acquisition. Isolated workers publish immutable
collections into the collector state; the sender delivers schema-2 heartbeats
every five seconds. Slow APFS, filesystem, SMART, and NFS acquisition remains
independent of that cadence. Exact integers, acquisition clocks, epochs,
resource identities, and explicit availability survive transport.

The Axum server owns enrollment, authentication, transactional SQLite storage,
retention, schema-2 compatibility, and authenticated reads. The native window
and headless executable share the same runtime. The server imports the pure
collector wire model and validator, not native collectors. The read layer
projects legacy and native observations into one dashboard contract while
keeping local/shared capacity and source/device coverage distinct.

The browser uses a read-only viewer credential held in memory and same-origin
requests. Its charts show browser-session observations of sampled driver I/O.
They do not measure application throughput or disk capability. Missing data is
unknown; node availability is not a storage-health assessment.

`simple-nfs-server` is a separate administrative CLI, outside the monitoring
workspace. It configures the OS NFS service and client mounts; the central
server does not start it or collect telemetry through it.

## Confirmed defects and corrections

| Area | Defect | Corrected behavior |
| --- | --- | --- |
| Native projection | Host OS and inventory update date were lost. | Native agent metadata and the receipt time of changed inventory reach node reads; routine heartbeats do not restamp inventory. |
| Protocol ownership | A v1 mutation could overwrite an established schema-2 node. | Legacy inventory, telemetry, heartbeat, and goodbye mutations return 409 once the node has accepted schema 2. |
| Failed collection | Repeated failed acquisitions could obscure the last usable reading or make it appear current. | Preserve exact values and original provenance, expose the latest failed attempt, and mark retained readings stale until a fresh usable acquisition. |
| Offline state | Slow native capacity could remain current after its owner went offline. | Object/filesystem readings become stale; node summaries retain stale values; cluster current totals exclude unavailable nodes. |
| Capacity | Native disk images and pseudo mounts inflated local totals; duplicate observers could be combined incoherently. | Require established physical backing for native local capacity, exclude ineligible filesystems, and choose one coherent observer per shared identity. Earlier corrections retained and rechecked. |
| Metric names | Six native catalog names did not match read aliases. | Correct APFS purgeable, NVMe warning/endurance, and NFS operations/timeouts/retries mappings. Earlier corrections retained and rechecked. |
| Network classification | SMB/CIFS/WebDAV mounts appeared local. | Classify supported network filesystem types consistently in rows and filters. |
| Console startup | Public CDN availability was required for app code and fonts. | Embed pinned Preact/htm, Three.js, and licensed Barlow fonts; use a same-origin content policy. |
| Visible features | Alerting navigation advertised an unimplemented feature; Access/filesystem labels overstated functionality. | Remove unsupported Alerting navigation and label the implemented security-event and filesystem-observation views accurately. |
| Inventory | Empty data could look perpetually loading; host identity was opaque; integer gauges lost units. | Distinguish loading, empty, and errors; expose reported names/properties and host metadata; preserve exact counters and non-byte integer gauges with units. Byte gauges use scaled units and an exact-value title. |
| Refresh and pagination | Optional inventory/table reads could stall core polling; large traversals and delayed responses could misrepresent freshness. | Poll core summaries independently, bound table requests to explicit frozen 100-row pages, preserve pages through retries/expiry, reject obsolete responses, and check both elapsed and wall time before accepting data as fresh. |
| Filesystem observations | A missing total-capacity value could incorrectly label usable fields as unavailable. | Display used/free/available values with their own states and acquisition dates. |
| Charts and interaction | Small rates rounded to zero, isolated samples disappeared, and an orbit drag could navigate. | Adaptive SI units, visible isolated points, explicit gaps/axes/coverage, and distinct pointer selection/orbit behavior. |
| Smoke test | Testing always used the separately packaged app, which could be stale. | Test the workspace binary by default; `--packaged` explicitly selects the bundle. Verify all eight read routes using the viewer credential. |
| NFS helper | Existing mounts could be mistaken for the requested export; config and managed-file errors could produce incorrect changes. | Verify the configured source, validate/escape configuration, reject malformed managed blocks, preserve unrelated bytes, and restore prior exports on validation failure. |

## Verification record

The final workspace suite passed **114 tests**, with zero failures and three
subprocess helpers intentionally ignored as standalone tests (their parent
tests exercise them). This includes 31 server tests: 17 unit tests and 14
collector compatibility tests. All **47 web tests** and **22 separate NFS
helper tests** passed. Workspace check, the default workspace build, the
headless server build, NFS helper check, six application JavaScript syntax
checks, and whitespace validation passed.

```sh
cargo test --workspace --locked --no-fail-fast
cargo check --workspace --locked
cargo build --package cider-server --no-default-features --locked
cargo build --workspace --locked
node --test web/tests/*.test.mjs
cargo test --manifest-path simple-nfs-server/Cargo.toml --locked
cargo check --manifest-path simple-nfs-server/Cargo.toml --locked
node scripts/smoke-ciderd.mjs
git diff --check
```

The final smoke used the rebuilt workspace binaries and passed verified TLS,
CLI enrollment, private credential/state checks, two distinct five-second
heartbeats, all eight authenticated viewer read routes, and embedded console
serving. It stopped its children and removed disposable credentials/state.
No new `dd` workload was run; the earlier bounded metrics evidence is not a
disk capability benchmark.

Browser QA exercised the real server and collector through the verified TLS
proxy described below. Checks covered rejected authentication, connect and
disconnect, overview/host/filesystem/throughput/security views, back/forward
navigation, rack orbit versus selection, host metadata and descriptive device
labels, exact counters, field-specific filesystem states, and rate chart
axes/units/points. A 390-by-844 viewport confirmed wrapped navigation, readable
controls and horizontally scrollable tables. Synthetic observations in the
disposable server additionally exercised sub-byte rates, missing contributors,
offline transitions, and more than 100 filesystem/event rows. Previous/Next
preserved a frozen snapshot, Refresh advanced it, and core summaries continued
refreshing while paging. The final freshness and favicon changes passed the
47-test run and rebuilt-binary smoke after the interactive browser run.

Independent reviewers cross-checked the server, collector, frontend, and NFS
changes. Logs remain under ignored `.codex-staging/`; all owned diagnostic
processes were stopped. Raw telemetry and credentials are not part of this
report or the shared specification.

## Limits and release boundary

Real NFS mounting/export changes were deliberately not exercised on this host;
the separate helper has pure regression tests and compile checks. Broader NFS
hardware/protocol coverage, Intel macOS, installed launchd behavior, and signed
distribution remain separate validation work. The computer-control provider
could not attach to the unbundled native executable for a native-window visual
sign-off. Browser checks use the built server and actual collector through a
temporary loopback proxy that verifies its upstream TLS certificate.

Alert evaluation/delivery, stored historical reads, SSE, Prometheus, OpenAPI,
and independently managed viewer credentials remain unimplemented. They are
not advertised as working console actions. IOKit rates still include the
reported driver set, potentially including virtual/disk-image traffic.

The shared Server Spec is the API source of truth. Native Google Docs edits
were saved to Drive and its current Server Spec tab was exported and compared
with the pre-review export. The audited final export is
`~/Downloads/TCL challenge 2026 (3).txt`; its new section is unique and all
three illustrative JSON fragments parse. Sections 16–18 record corrected behavior, route
contracts, examples, verification and limits. No Google Docs connector was
available and no connector write occurred, so connector-based documentation
sign-off remains unavailable. This local report is not a replacement for the
shared contract.
