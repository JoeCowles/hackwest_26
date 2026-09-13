# Orchard storage monitoring

Rust macOS collector, Rust native central server, and an authenticated live web
console for storage telemetry.

- `server/`: `orchard-server`, a native macOS control application with SQLite,
  TLS, enrollment, ingestion, and read APIs. It does not run node collectors.
- `crates/ciderd/`: the macOS storage collector and its schema-2 wire contract,
  collectors, isolated workers, fixtures, daemon configuration, and launchd example.
- `web/`: the Orchard Cluster Console, using live authenticated server reads.
- `scripts/`: project-local Cargo wrapper, macOS packaging, and synthetic demo client.
- `flake.nix` and `flake.lock`: the collector branch's optional Nix development environment.

## Source of truth

The [Server Spec](https://docs.google.com/document/d/1JnsZHlYPXQ1IRsMSEHeboICzqqviKJylI7gRsuUK13E/edit?tab=t.bclfm0yxwd6r)
defines the receiver API. Section 15 documents v1 ingestion, section 16 the live
read API, and section 17 the ciderd compatibility endpoint. Planned monitoring
features are not implied to be implemented by their appearance in section 14.
The exact collector models, catalog, and JSON schemas live in
`crates/ciderd/contract/` and `crates/ciderd/src/model.rs`.

## Run the server

Install Rust and the Xcode command-line tools. The Cargo wrapper also supports
this checkout's ignored project-local Rust installation.

```sh
bash scripts/cargo-local.sh run --package orchard-server
```

The native window owns the server lifetime, creates one-use enrollment tokens,
and opens the live web console. Default binding is `http://127.0.0.1:8787`.
Default macOS state is `~/Library/Application Support/Orchard Server/`; do not
commit its credentials or database. `--headless` runs without the native window.

For real collectors, enable TLS even on loopback:

```sh
bash scripts/cargo-local.sh run --package orchard-server -- \
  --bind 127.0.0.1:8787 \
  --tls-cert /absolute/path/server-cert.pem \
  --tls-key /absolute/path/server-key.pem
```

Clients must trust the certificate and its hostname/IP. Non-loopback binding
requires TLS. No trust root or launch daemon is installed automatically.
Use the server's console link rather than opening `web/index.html` as a file;
the live API is same-origin and uses a separate read-only viewer credential.

## Connect ciderd

Build/install the collector separately, following `crates/ciderd/README.md`.
Adapt `crates/ciderd/examples/ciderd.toml` with absolute local paths, the server's
HTTPS address, and an optional private-CA certificate file. Its endpoint is
`/api/v2/ciderd/heartbeat`, with a five-second heartbeat interval. Slower and
potentially blocking collectors remain independent of that cadence.

Use a new one-use token from the native server window, stored in a private file:

```sh
ciderd enroll-server --config /etc/ciderd.toml --name studio-mac \
  --enrollment-token-file /secure/enrollment-token
ciderd run --config /etc/ciderd.toml
```

Enrollment stores a mode-0600 node credential and a matching persistent identity,
creates a private collector state directory before exchanging the token,
refuses to overwrite either identity/credential file, and does not print the credential. Do not run the
local-only `enroll` command first. Partial enrollment or a lost network response
requires explicit recovery; the single-use exchange is not retried. Production
collector installation uses the root-owned paths described in its README.

The compatibility endpoint preserves immutable collection IDs, exact decimal
integers, counter epochs, monotonic acquisition times, explicit availability,
and tombstone-based inventory removal. It projects supported storage metrics
into the existing dashboard API; additional catalog metrics retain their
schema-2 names and provenance. Cached MNT_NOWAIT observations do not substitute
for fresh filesystem-capacity measurements. Physical devices and IOKit driver
observations are not summed twice. Shared NFS capacity remains unknown unless
an authoritative shared filesystem identity is supplied.

Do not send legacy v1 inventory/telemetry and schema-2 heartbeats using the same
node identity. Existing v1 clients keep their original endpoints and contract.
The SQLite migration adds schema-2 receiver state and raises the schema version
to 2; an older server refuses this newer database. Back up state before running
a newly built server against an existing installation.

## Build a macOS server bundle

```sh
bash scripts/package-macos.sh debug
```

The script produces `dist/Orchard Server.app` with an ad-hoc signature. Developer
ID signing and notarization remain separate release work.

## Integration status

The collector branch is integrated alongside the server and web console.
Workspace builds/checks and all 100 tests passed; three ignored subprocess
helpers are exercised by their parent tests. Five server compatibility tests
cover replay/deduplication, exact wide counter rates and epochs, inventory
upserts/tombstones, persisted receipts, and credential/privacy rejection.
The headless server built, and the debug macOS application bundle was rebuilt
and passed strict code-signature verification.

The real collector and packaged server also passed an isolated local TLS smoke
test: verified certificates, CLI enrollment, private credentials/state directory,
two accepted five-second heartbeats, real measurements through authenticated
read APIs, and the embedded console HTML. This is not a desktop/mobile visual
review or exhaustive hardware/NFS validation. Alert evaluation/delivery,
historical read routes, Prometheus/SSE, and other planned APIs remain separate work.

To repeat the validation on macOS:

```sh
bash scripts/cargo-local.sh build --workspace --locked
bash scripts/cargo-local.sh test --workspace --locked --no-fail-fast
bash scripts/cargo-local.sh check --workspace --locked
bash scripts/cargo-local.sh build --package orchard-server --no-default-features --locked
bash scripts/package-macos.sh debug
codesign --verify --deep --strict 'dist/Orchard Server.app'
node scripts/smoke-ciderd.mjs
```

The smoke test requires Node.js and macOS OpenSSL. It uses a temporary CA,
ephemeral local port, and separate state; it does not modify a running
installation. It briefly gathers this Mac's real storage telemetry and stops
its processes afterward. Successful runs remove temporary state; failures keep
private diagnostic artifacts under the ignored `.codex-staging/` directory.
