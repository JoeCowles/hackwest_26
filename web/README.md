# Orchard live console

The Rust server embeds and serves this frontend at its own root URL, normally
`http://127.0.0.1:8787/`. Rebuild and restart the server after changing web assets.
Running a separate static development server is not the supported live setup.

1. Start the server with `bash scripts/cargo-local.sh run -p orchard-server`.
2. In the native application, choose **Open web console** and **Copy viewer credential**.
3. Paste the viewer credential into the console's connection form.

For headless operation, use `-- --headless`. The current viewer credential is in
`viewer-token` in the server data directory, normally
`~/Library/Application Support/Orchard Server`. It has owner-only permissions,
expires eight hours after server startup, and rotates on restart. The browser
keeps it in memory only; disconnect or reload clears it. Do not use admin or node
credentials in the browser. Remote bindings require TLS as before.

The overview, filesystem, throughput, and host-detail views use authenticated
read routes documented in Server Spec section 16. Polling runs every five
seconds while visible and every thirty seconds while hidden. Pagination is
followed, reads time out, and errors back off with Retry-After support. Failed
refreshes retain the last snapshot for context but hide current numeric values.

The rack displays up to 24 real enrolled nodes; the host table includes all nodes.
Placement is illustrative. Unknown hardware uses a generic desktop model.
Charts contain actual polls observed in this browser session, not historical
backfill. Source timestamps and missing/stale observation states are retained.
There is no fallback to fixture hosts or random metric jitter.

Security displays reported collector events only. Alert evaluation, SMS, access
management, historical metric queries, SSE, and Prometheus remain unavailable.
The UI intentionally does not simulate these features. APFS/NFS rows are not
additive; aggregate capacity comes from the server's deduplicated summaries.

Existing pinned Preact/htm, Three.js, and Google Fonts dependencies still load
from their public CDNs. Three.js is optional; host tables work without it.

Validation status for this wiring change: not built, tested, or visually verified.
