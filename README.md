# Cider storage monitoring

Rust macOS collector, Rust native central server, and an authenticated live web
console for storage telemetry.

Cider helps an administrator review storage concerns and their evidence. The
console includes live throughput and capacity, drive reliability observations,
activity deviations, an attention queue, configurable Twilio SMS, stored metric
history, capacity exhaustion scenarios, NFS user quotas, and diagnostic readiness.
USB connection watches cover confirmed disappearance and a negotiated link below
the same enclosure's previously confirmed speed.
Missing evidence stays unknown. Cider observes and notifies; the administrator
decides how to respond.

Enrolled collectors reuse their saved credentials after restarts. The console
remembers its read-only viewer token, and server restarts retain that token.
Discovery and visible refreshes use three seconds; heartbeats remain five.
See [session and refresh details](docs/session-and-disconnection-refinements.md)
and [disconnect evidence and notification behavior](docs/disconnection-alerts.md).

- `server/`: `cider-server`, a native macOS control application with SQLite,
  TLS, enrollment, ingestion, and read APIs. It does not run node collectors.
- `crates/ciderd/`: the macOS storage collector and its schema-2 wire contract,
  collectors, isolated workers, fixtures, daemon configuration, and launchd example.
- `web/`: the Cider Cluster Console, using live authenticated server reads.
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
Sections 20–21 document storage activity and reliability, section 22 operator
workflows, and section 23 USB connection watches. Section 23 was saved through
native Google Docs and verified by export comparison.
The exact collector models, catalog, and JSON schemas live in
`crates/ciderd/contract/` and `crates/ciderd/src/model.rs`.

## Storage activity detection

The first rule uses existing native `iokit.block` driver byte counters. It learns
for at least 600 observed seconds and 60 intervals, then opens a finding after
120 sustained seconds above its versioned threshold. Missing observations remain
unavailable; they never resolve a finding. The viewer exposes findings, readiness,
coverage, and exact interval evidence. Administrator-only relearning is a guarded
API action. File-access detection and automated remediation remain outside
this implementation. Attention and optional Twilio delivery are described below.

```sh
bash scripts/cargo-local.sh test --workspace --locked
bash scripts/cargo-local.sh run -p cider-server --example detection_example --locked
bash scripts/cargo-local.sh build --workspace --locked
node scripts/smoke-detection.mjs
```

The example uses synthetic time and counters. The smoke uses disposable verified
TLS and this Mac's real collector to check admission, learning, stale observations,
and embedded modules. Neither is a measurement of detection efficacy.

## Drive reliability

Passive SMART/NVMe/ATA and IOKit evidence now feeds persistent reliability findings,
workload-qualified service-time assessment, authenticated read APIs, and the disk
view. SMART remains opt-in; missing evidence and replacement dates remain unknown.
See [the reliability runbook and API contract](docs/drive-reliability.md). Shared
Server Spec section 21 is saved and export-verified through native Google Docs.

## USB connection showcases

The collector, durable watch policy, disk read projection and UI are implemented.
Source checks and authenticated synthetic transition tests pass. A local physical
unplug, 5 Gb/s-to-480 Mb/s downshift, and return-to-5 Gb/s rehearsal passed on one
enclosure. Real handset delivery remains unverified.

The collector publishes passive USB presence and negotiated-speed observations
every three seconds with the example configuration, delivered in five-second
heartbeats. Two complete fresh absences of an armed connection open an
Attention concern; two slower-link observations against a confirmed baseline
open a separate connection-speed concern. Both use the existing optional SMS
outbox. Unknown, partial and stale observations do not prove loss or recovery.

The new **Presence and USB connection** panel presents readiness, identity
scope and exact bitrates. Replug comparisons require a stable reported enclosure
identity and a prior faster baseline. These observations describe the transport
connection; media health and future failure remain unknown from this evidence.
See [the USB demo runbook](docs/usb-demo-alerts.md) for rehearsal and verification.

## Run the server

Install Rust and the Xcode command-line tools. The Cargo wrapper also supports
this checkout's ignored project-local Rust installation.

```sh
bash scripts/cargo-local.sh run --package cider-server
```

The native window owns the server lifetime, creates one-use enrollment tokens,
and opens the live web console. Default binding is `http://127.0.0.1:8787`.
Default macOS state is `~/Library/Application Support/Cider Server/`; do not
commit its credentials or database. `--headless` runs without the native window.

For real collectors, enable TLS even on loopback:

```sh
bash scripts/cargo-local.sh run --package cider-server -- \
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
to 6, including detection, reliability, attention, notification and USB watch state; an older
server refuses this newer database. Back up state before running
a newly built server against an existing installation.

## Operator workflows

Open **Operations** in the dashboard:

- **Attention** combines capacity pressure, node observation loss, activity and
  reliability findings, and passive filesystem concerns. Acknowledgement records
  review separately from source recovery. A live indicator exposes evaluation
  freshness while evidence pages remain frozen until refreshed.
- **Notifications** configures the sender, recipient and enablement with a
  temporary administrator credential. Phone numbers are masked on reads.
- **History** reads raw or rolled-up observations for one object and metric.
  Capacity runway requires sufficient compatible, fresh growth evidence; the
  forecast includes assumptions and a scenario range. It does not predict drive
  failure or guarantee three days of warning.
- **Quotas** reports configured NFS server user quotas through read-only rquota.
  Quota usage and limits are separate from APFS volume quotas and filesystem
  capacity. Missing, denied and stale records never imply an unlimited quota.
- **Diagnostics** distinguishes disabled, unsupported, missing, failed and stale
  collectors and passive NFS accessibility concerns. Filesystem integrity remains
  unknown without supporting evidence.

Create a private `.env` in the server's working directory, or set the process
variables. `.env` and `.env.*` are ignored by Git:

```dotenv
TWILIO_ACCOUNT_SID=AC_your_account_sid
TWILIO_SECRET=your_account_auth_token
```

An API key secret additionally needs `TWILIO_SID=SK_your_api_key_sid`; the account
SID alone pairs with an account Auth Token. Restart the server after changing
credentials. In **Operations → Notifications**, set E.164 sender and recipient
numbers, choose whether to enable SMS, and save with the administrator credential.
Sending is disabled initially. Enablement applies to future new or escalating
concerns; suppressed history is not replayed. Provider acceptance and handset
delivery have separate states. Ambiguous sends are marked uncertain.

See the [HTTP contract](docs/operator-api.md), [attention/SMS runbook](docs/operator-attention.md),
[history and forecast contract](docs/metric-history.md), and [quota setup and live
verification](docs/nfs-user-quotas.md). Original project code is [MIT licensed](LICENSE);
existing third-party code, fonts and assets retain their own notices.

## Integration status

The current build passes 345 Rust tests, 128 web tests, workspace/default and
headless checks, and verified-TLS integration. USB physical rehearsal and software
evidence are recorded in the [USB runbook](docs/usb-demo-alerts.md). Real SMS
delivery has not been exercised. No packaged/signed application or production
installation is implied by these checks.

To repeat source validation on macOS:

```sh
bash scripts/cargo-local.sh test --workspace --locked --no-fail-fast
bash scripts/cargo-local.sh check --workspace --locked
bash scripts/cargo-local.sh build --package cider-server --no-default-features --locked
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
node scripts/smoke-operators.mjs
```

The ciderd smoke test requires Node.js and macOS OpenSSL. It uses a temporary CA,
ephemeral local port, and separate state; it does not modify a running
installation. It briefly gathers this Mac's real storage telemetry and stops
its processes afterward. Successful runs remove temporary state; failures keep
private diagnostic artifacts under the ignored `.codex-staging/` directory.
The operator smoke uses a disposable loopback server with synthetic observations
and a captured real rquota response replay. SMS remains disabled and it does not
load the working-directory `.env`.

## Optional macOS packaging

Packaging and bundle verification are separate from the source checks above:

```sh
bash scripts/package-macos.sh debug
codesign --verify --deep --strict 'dist/Cider Server.app'
node scripts/smoke-ciderd.mjs --packaged
```

The packaging script produces `dist/Cider Server.app` with an ad-hoc signature.
The packaged smoke also requires the workspace ciderd binary. Developer ID
signing and notarization remain separate release work. These commands are
instructions for a new packaging run, not evidence that this review rebuilt or
validated a distributable bundle.
