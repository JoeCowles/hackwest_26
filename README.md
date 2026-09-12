# Orchard Server

Rust macOS application and durable storage-telemetry ingestion service.

The shared [Server Spec](https://docs.google.com/document/d/1JnsZHlYPXQ1IRsMSEHeboICzqqviKJylI7gRsuUK13E/edit?tab=t.bclfm0yxwd6r)
is the API source of truth. Section 14 contains the **planned** web and monitoring
routes; section 15 describes the core ingestion implementation. The existing
`web/` console is a separate fixture-driven frontend and is not connected yet.

## Run on macOS

```sh
bash scripts/cargo-local.sh run --package orchard-server
```

The native window displays the API address and storage counters, creates
single-use enrollment tokens, and lets the local administrator copy credentials.
Closing the window stops the server. To run without the window:

```sh
bash scripts/cargo-local.sh run --package orchard-server -- --headless
```

By default the server listens at `http://127.0.0.1:8787`. Data and the owner-only
admin credential are in `~/Library/Application Support/Orchard Server/`.
The credential is never printed by the server. Preserve this directory across
upgrades; it contains node identity, deduplication state, and the SQLite database.
Only one process can open the same data directory.

For LAN access, use a certificate trusted by your client machines:

```sh
bash scripts/cargo-local.sh run --package orchard-server -- \
  --bind 0.0.0.0:8787 --tls-cert /path/to/cert.pem --tls-key /path/to/key.pem
```

The server rejects non-loopback plaintext binding. Configuration can also use
`ORCHARD_BIND`, `ORCHARD_DATA_DIR`, `ORCHARD_TLS_CERT`, and `ORCHARD_TLS_KEY`.
There is no certificate generation, trust-store modification, automatic LAN
discovery, login item, or background launch daemon in this version.

## Build the macOS app

```sh
bash scripts/package-macos.sh
open 'dist/Orchard Server.app'
```

The script creates an app for the build machine's architecture and ad-hoc signs
it for local use. Apple Developer ID signing/notarization for distribution is
not included. Use `bash scripts/package-macos.sh debug` for a quicker local build.

## Implemented core routes

| Method | Route | Credential |
| --- | --- | --- |
| POST | `/api/v1/enrollment-tokens` | Administrator |
| POST | `/api/v1/nodes/enroll` | One-time token in body |
| PUT | `/api/v1/nodes/{node_id}/inventory` | Matching node |
| POST | `/api/v1/nodes/{node_id}/telemetry` | Matching node |
| POST | `/api/v1/nodes/{node_id}/heartbeat` | Matching node |
| POST | `/api/v1/nodes/{node_id}/goodbye` | Matching node |
| DELETE | `/api/v1/nodes/{node_id}/credential` | Administrator |

Every mutation requires `X-Request-ID` (fresh UUID) and `X-Request-Timestamp`
(RFC3339 within 300 seconds). Protected routes also require
`Authorization: Bearer <credential>`. Send a heartbeat every **5 seconds**.
See section 15 of the Google Doc for complete payloads and retry semantics.

All writes are transactional, with SHA-256 credential hashes, single-use
enrollment tokens, and persisted replay IDs. An accepted telemetry batch is
durable before HTTP 202 is returned. Duplicate `(node_id, boot_id, sequence)`
batches do not insert samples or events twice. Reusing that identity with a
different payload returns 409. The JSON body limit is 1 MiB.

Objects have stable server IDs based on node identity and collector-local IDs.
Inventory snapshots preserve history. Counter rates retain boot/generation and
observation-state boundaries; raw counters remain exact. A worker writes
5-minute/hourly rollups and expires old raw data, receipts, and events.

## Synthetic demo

Start the server, then run:

```sh
node scripts/demo-node.mjs
```

This creates one synthetic node and sends twelve batches at 5-second intervals.
It does not collect data from this Mac. Override `ORCHARD_URL`,
`ORCHARD_ADMIN_TOKEN_FILE`, or `ORCHARD_DEMO_BATCHES` if needed. The demo sends
goodbye on completion. It never prints credentials.

## Development

```sh
bash scripts/cargo-local.sh test --workspace
bash scripts/cargo-local.sh check --workspace
```

The project-local Rust toolchain under `.codex-staging/` is a convenience for
this checkout and is ignored by version control. Other developers can use their
normal Rust installation. Xcode command-line tools are needed on macOS.

Deferred: section 14 read/monitoring routes, public OpenAPI discovery, frontend
integration, alert evaluation/delivery, and collector implementations. The
native window reads local server operational state; it is not the web dashboard.

## Live frontend integration update

This section supersedes the earlier statement that all web read endpoints are
planned. The server now includes eight read routes and embeds the web console
at `/`. See `web/README.md` for the viewer-credential connection flow and Server
Spec section 16 for the exact supported API subset. Rebuild/restart to include
these source changes; the previously packaged application is not updated by
editing source files. This change has not been built, tested, or visually checked.

Implemented read routes: `/api/v1/cluster`, `/api/v1/nodes`,
`/api/v1/nodes/{node_id}`, `/api/v1/nodes/{node_id}/inventory`,
`/api/v1/objects/{object_id}`, `/api/v1/filesystems`, `/api/v1/events`, and
`/api/v1/capabilities`. The viewer credential is read-only and expires eight
hours after startup. The native app can copy it and open the console.

Historical inventory/metric queries, alert management, SSE, OpenAPI discovery,
Prometheus, and health/readiness monitoring endpoints remain planned.
