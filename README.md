# Orchard storage monitoring

Rust macOS collector, Rust native central server, and an authenticated live web
console for storage telemetry.

- `server/`: `orchard-server`, a native macOS control application with SQLite,
  TLS, enrollment, ingestion, and read APIs. It does not run node collectors.
- `crates/ciderd/`: the macOS storage collector and its schema-2 wire contract,
  collectors, isolated workers, fixtures, daemon configuration, and launchd example.
- `web/`: the Orchard Cluster Console, using live authenticated server reads.
- `scripts/`: project-local Cargo wrapper, TLS integration smoke test, macOS
  packaging, and synthetic demo client.
- `simple-nfs-server/`: a separate administrative NFS helper, outside the monitoring
  workspace; its tests do not change system exports or mounts.
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

## Integration status

The integrated checkout passed 114 workspace tests, with three subprocess helpers
intentionally ignored as standalone tests and exercised through their parent
tests. The total includes 31 server tests: 17 unit tests and 14 compatibility
tests. All 47 web tests and the separate NFS helper's 22 tests passed. Workspace
check, the default workspace build, and the headless server build also passed.

The latest isolated TLS smoke passed against the workspace server binary and
actual ciderd process: certificate verification, CLI enrollment, private
credentials/state, two distinct five-second heartbeats, real measurements,
all eight read routes authenticated with a viewer credential, and embedded
console delivery. The smoke command uses the workspace binary by default;
`--packaged` explicitly selects the separately built application bundle.

Browser checks covered desktop and 390 × 844 layouts through a temporary loopback
proxy that verified the server's upstream TLS certificate. They used the real
collector plus synthetic records for pagination, tiny rates, partial coverage,
and offline states. Native-window visual QA, a freshly packaged/signed bundle,
and deployment were not performed during this review. Live NFS export/mount
operations and broader hardware/platform coverage remain unverified.

See the [integration review](docs/integration-review.md) for the corrections,
evidence, and remaining limits. Alert evaluation/delivery, stored historical
reads, Prometheus/SSE, and other planned APIs remain unimplemented; the console
exposes the implemented views and actions.

To repeat source validation on macOS:

```sh
bash scripts/cargo-local.sh test --workspace --locked --no-fail-fast
bash scripts/cargo-local.sh check --workspace --locked
bash scripts/cargo-local.sh build --package orchard-server --no-default-features --locked
bash scripts/cargo-local.sh build --workspace --locked
node --test web/tests/*.test.mjs
node --check web/js/api.js
node --check web/js/app.js
node --check web/js/model.js
node --check web/js/session.js
node --check web/js/stage.js
node --check web/js/views.js
bash scripts/cargo-local.sh test --manifest-path simple-nfs-server/Cargo.toml --locked
node scripts/smoke-ciderd.mjs
```

The smoke test requires Node.js and macOS OpenSSL. It uses a temporary CA,
ephemeral local port, and separate state; it does not modify a running
installation. It briefly gathers this Mac's real storage telemetry and stops
its processes afterward. Successful runs remove temporary state; failures keep
private diagnostic artifacts under the ignored `.codex-staging/` directory.

## Optional macOS packaging

Packaging and bundle verification are separate from the source checks above:

```sh
bash scripts/package-macos.sh debug
codesign --verify --deep --strict 'dist/Orchard Server.app'
node scripts/smoke-ciderd.mjs --packaged
```

The packaging script produces `dist/Orchard Server.app` with an ad-hoc signature.
The packaged smoke also requires the workspace ciderd binary. Developer ID
signing and notarization remain separate release work. These commands are
instructions for a new packaging run, not evidence that this review rebuilt or
validated a distributable bundle.
