# Disk and hardware-model integration checkpoint

**Browser gate complete, 2026-09-13:** The working in-app Browser surface completed
the real-Mac and synthetic scenario checks. After fixing overlapping rack labels,
all **81 web tests** and the default workspace build passed. The current real
test console is `http://127.0.0.1:54618`, through a loopback proxy with verified
upstream TLS. Shared Server Spec section 19 native save/export verification is
complete; no connector write or connector sign-off is claimed. The user confirmed
that a second physical Mac is unavailable. See the [continuation handoff](handoff-disk-model-integration.md)
for current runtime, evidence and the remaining hardware gate.

Implementation tasks 1–7 are complete and reviewed in the current checkout.
Verification on 2026-09-12 passed **154 Rust tests** (three subprocess-helper
entry points intentionally ignored) and **76 web tests**. Default and headless
checks/builds passed. These Rust/build-matrix results remain prior-run evidence;
no Rust behavior changed or full Rust suite was rerun in this continuation.
Acceptance on two physical Macs remains the only planned pending gate; connector
write/sign-off remains unclaimed separately.

The collector now supplies optional hardware identity and native driver/mount
correlation evidence. The receiver preserves accepted acquisition context; a
later heartbeat does not give retained evidence a new boot or acquisition date.
The server supplies one physical-disk projection, confirmed source rates,
normalized typed relationships, disk-filtered inventory and exact hardware
classification. Its catalog contains 47 Apple Silicon identifiers from Apple's
four supported product lists. The console adds canonical disk/pool/volume/mount
groups, disk deep links and browser-session histories, plus explicit hardware
meshes and rack pages of at most 24 hosts.

The dense-rack browser check exposed overlapping labels. Compact labels now have
bounded widths, measured rectangles suppress collisions, and a hovered host gets
priority with its full name wrapped within the viewport. This changes label
presentation only; all meshes, stable identities/order, page sizes, camera/orbit
and pointer behavior remain intact. The host table retains full accessible names.

Explicit TLS configuration and enrollment remain required. Heartbeats remain
five seconds. Missing or ambiguous observations remain unknown, shared APFS/NFS
capacity is not added twice, and source counters retain exact integer values.
The central server performs no native collection. One enrolled collector per
physical host remains the deployment assumption; duplicate enrollment on the
same Mac is not automatically merged.

The implemented read contract adds `GET /api/v1/nodes/{node_id}/disks` and the
inventory `disk_id` filter; `disk_id` and `kind` cannot be combined. Cursor pages
retain their original data, source states, snapshot time and structural metadata.
The browser ages current-rate eligibility separately. `io.source_object_ids`
is always an array, empty when no source is selected; driver identity and
continuity remain nullable. These contract clarifications are included in the
native saved/exported section 19; this local description is not its substitute.

## Verified evidence

This continuation reverified native startup (one MacBook, one disk, one resolved
driver, ten linked mounts, 41 graph objects), rebuilt embedded assets,
401/403/viewer authentication behavior, frozen inventory cursor metadata and
400 invalid filters through read-only HTTP checks. Root then authenticated in
the working in-app Browser and verified this actual MacBook (`Mac15,6`), its
overview/host views, disk0 deep link, live disk rates and explicit unknown fields.

The separate `.codex-staging/storage-fixture.mjs` serves the actual production
assets with clearly labeled synthetic data. Its HTTP/model/VNode checker passed
both author and independent root runs, including byte-for-byte delivery of 12
production assets. Subsequent Browser checks verified all four meshes; Tab,
Enter and Space operated 24/24/9 rack pages; opening actual host 57's mesh retained
the later page; host 58 joined through ordinary polling without reload. Host 49's
two disks linked to one canonical shared pool. Exact unsigned-128-bit counters,
bytes units, 0.2 B/s activity and unknown fields remained readable.

Disk-summary/inventory 503 and inventory 429 cases retained dated complete storage
context, hid current disk rates as Unknown and preserved independent core polling.
Retry/recovery produced isolated points and distinct SVG path segments across
gaps. Selected-disk removal showed explicit unavailable text; host removal fell
back to the Host detail host list; restoration preserved node/disk IDs. Actual
390 × 844 browser surface/DOM checks covered disk and rack views. Final desktop
label and full-hover checks passed at 1280 × 900. Both the actual and synthetic
sessions had empty browser warning/error logs.

| Check | Result and scope |
| --- | --- |
| Rust workspace tests | Prior run: 154 passed, zero failed, three intentionally ignored helpers; [local log](../.codex-staging/disk-model-workspace-tests.log). |
| Web tests | Fresh final run: 81 passed, zero failed. Five added rack-label geometry tests cover collisions, anchor/identity preservation, hover priority, viewport bounds and invalid projections; [local log](../.codex-staging/browser-final-web-tests.log). Four behavior tests failed against the old placement before the fix. Prior 76-test log remains historical. |
| Builds | Fresh default workspace build passed with final embedded web assets; [local log](../.codex-staging/browser-final-build.log). Prior workspace/headless checks and builds passed in `.codex-staging/disk-model-{workspace,headless}-{check,build}.log`; those matrix checks were not rerun for this label-only change. |
| Native identity proof | This Mac's boot snapshot resolved through a direct IOKit APFS-volume parent; an ordinary local volume resolved directly. Both matched one graph volume; [redacted proof](../crates/ciderd/tests/fixtures/mount-identity-boot-snapshot.json). No BSD-suffix guessing was used. |
| Verified-TLS storage smoke | One real MacBook, one physical disk, one resolved driver, ten linked local mounts and 41 graph objects. Enrollment, two distinct five-second heartbeats, coherent topology, authenticated reads and embedded modules passed; [local log](../.codex-staging/disk-model-live-smoke.log). |
| Empty/stopped collector | The smoke verified unknown empty-cluster capacity/rates, then degraded-to-offline transition, retained node/disks and unknown current aggregates. |
| Fresh native/API checks | Startup verified one actual MacBook, one physical disk, one driver, ten linked mounts and 41 objects over verified TLS; [startup log](../.codex-staging/browser-final-native.log). Authenticated reads, source-matching embedded modules, frozen cursor metadata and status/filter checks passed; [HTTP log](../.codex-staging/browser-final-live-verify.log). |
| Authenticated real-Mac browser | Rebuilt overview, MacBook `Mac15,6` label/mesh, host/disk0 deep link, live disk rates, unknown fields and no warning/error logs; [overview capture](../.codex-staging/browser-real-overview.png). Earlier desktop/mobile real-Mac evidence remains in the handoff. |
| Synthetic browser interactions | Four meshes, keyboard paging, host 57 selection, host 58 join, shared pool once, exact counters, failure/retry/recovery and stable removal/restoration passed. Captures include [shared pool/raw values](../.codex-staging/browser-shared-raw.png), [summary failure](../.codex-staging/browser-storage-503.txt), [inventory failure](../.codex-staging/browser-inventory-503.txt), [429 retry](../.codex-staging/browser-inventory-429.txt) and [selected-disk removal](../.codex-staging/browser-disk-removed.txt). |
| Label and responsive layout | Visible label rectangles did not overlap, and the full hovered name was readable: [final desktop](../.codex-staging/browser-rack-labels-desktop.png), [hover](../.codex-staging/browser-rack-hover.png). Valid 390 × 844 captures: [disk](../.codex-staging/browser-disk-mobile.png), [rack](../.codex-staging/browser-rack-mobile.png). The removed-host capture is an earlier viewport-control attempt and is not mobile acceptance evidence. |
| Shared specification native save/export | “Saved to Drive” observed. Independent final DOCX export preserved the sections 1–18 prefix, exactly one section 19, one Heading 1/eight Heading 2 subheadings, ten native tables and the full 46,833 non-whitespace-character body. All ten decoded JSON examples matched; all 571 code/request paragraphs used Roboto Mono 9 pt and light shading. [Validation record](../.codex-staging/server-spec-section19-published-validation.json), [before text](../.codex-staging/browser-spec-before-docx.txt), [after text](../.codex-staging/browser-spec-after-docx.txt); prepared source is `server-spec-section19.{html,txt}` and `server-spec-section19-inline-fragment.html` under `.codex-staging/`. This is native UI evidence, not a connector write/sign-off. |

Private logs and browser-session state under `.codex-staging/` are ignored local
evidence. They are not required repository artifacts and must not be published
with credentials or raw host telemetry. Synthetic multi-disk/shared-pool and
57-node tests demonstrate software behavior; they are not two-Mac evidence.

## Reproducing the local checks

From the repository root:

```sh
cargo test --workspace --locked --no-fail-fast
cargo check --workspace --locked
cargo check --package cider-server --locked --no-default-features
cargo build --workspace --locked
cargo build --package cider-server --locked --no-default-features
node --test web/tests/*.test.mjs
git diff --check
node scripts/smoke-storage.mjs
```

The smoke requires Apple Silicon macOS, the built workspace binaries and
OpenSSL. It uses its own loopback ports, verified private TLS, explicit enrollment
and disposable state; it stops and cleans up only processes/state it owns.
It waits through the collector's normal degraded/offline thresholds. It does
not drive browser interactions, benchmark disk capability or enroll another
physical Mac. Default and headless builds share an executable path; rebuild the
desired feature set before starting it. Rebuild after any web-asset change.

## Remaining gates and running session

- Run acceptance with two physical Macs, one MacBook and one Mac mini,
  explicitly enrolled to the same head node over verified TLS. Those machines
  were unavailable for this run.

The native section 19 write/export task is complete. No successful Google Docs
connector write or connector sign-off is claimed.

The older native sessions and the synthetic fixture are confirmed stopped; the
fixture launcher exited successfully after its browser tabs were closed. The
current real isolated launcher remains running at port 54618;
`.codex-staging/storage-browser-state.json` contains its fresh metadata. Use the
native window's **Copy viewer credential**
button with the proxy URL above. Recheck metadata and process identities before
stopping or restarting; keep credentials private.
No commit, push, package or deployment was performed.

The [implementation plan](superpowers/plans/2026-09-12-disk-model-integration.md)
tracks the remaining hardware gate. The [web README](../web/README.md) provides the normal
application startup and viewer-credential workflow. This checkpoint records
local implementation and evidence; it does not claim complete hardware acceptance
or a successful connector write/sign-off.
