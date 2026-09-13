# USB disappearance and connection-speed demo

This change adds a connection watch to Cider's existing Attention and SMS
pipeline. It covers an observed USB storage connection disappearing and the same
identified connection negotiating below a previously confirmed speed. These are
connection observations; they do not establish damaged media or predict failure.

Implementation and software integration checks pass on `codex/usb-demo-alerts`.
The collector, durable server policy, Attention/outbox and live disk projection
are connected. The local physical rehearsal also passed; the steps below make it
repeatable on the demonstration hardware. Real SMS delivery remains unverified.
Shared Server Spec section 23 was saved through native Browser/Google Docs and
export-verified: the preceding 2,592 paragraphs were preserved and all 34 appended
paragraphs matched, with native Heading 1/2 styles. No connector write was used.
See [the API reference](usb-connection-api.md).

## Preparing the hardware rehearsal

Run the current server and one current ciderd per Mac, using their normal verified
HTTPS enrollment. Attach both USB drives and allow physical inventory to discover
them. Physical-disk discovery still follows its independent 300-second default;
the connection watch uses five-second IOKit observations and does not wait for
another physical-disk scan to detect an already observed connection's loss.

Open a disk's **Presence and USB connection** section. Confirm current observations
and an armed presence watch. Two distinct successful observations arm the watch.
For the slower-link scenario, first use the faster connection until two matching
negotiated-speed observations confirm the baseline. Keep the collector running
through unplug/replug; a new collector session must establish presence again.

The slower-link comparison requires `reported_usb_serial` identity with
`usb_enclosure` scope. A boot/registry identity can detect disappearance in the
same collector session, but cannot reliably match a reconnection or carry its
speed baseline to a new registry entry. Some enclosures omit serials, change
identity across connection modes, or present ambiguous multiple drives. The UI
must expose those limitations rather than claim a comparison.

An enclosure identity follows the enclosure, not verified installed media.
Changing the contained drive can preserve that identity. Current link rate is a
negotiated transport bitrate, separate from sampled read/write bytes per second.
A device starting on a slow link without a prior faster observation establishes
that initial baseline and does not warn.

Configure Twilio credentials and sender/recipient through the existing documented
workflow, then enable notifications before creating the concern. Enablement does
not replay disabled history. The SMS contains an Attention ID and directs the
operator to the dashboard; provider acceptance and handset delivery are distinct.
See [SMS configuration and delivery](operator-api.md#223-sms-configuration-and-delivery).

## Expected transitions

| Action | Expected connection-watch behavior |
| --- | --- |
| Previously observed device absent in one complete fresh scan | Pending; no new loss message |
| Absent in two consecutive complete fresh scans | One warning/Attention episode and normal SMS admission |
| Same identified device observed twice again | Loss concern resolves |
| Same reliably identified enclosure negotiates below its confirmed baseline twice | One connection-speed warning |
| Negotiated speed returns to baseline or above twice | Connection-speed concern resolves |
| Scan fails, is partial, stale, replayed, or clock/session changes | Unknown; no fabricated disappearance or recovery |
| Slower connection subsequently disappears | Existing link concern remains unresolved; disappearance is assessed separately |

At the five-second sampling cadence, two successful observations usually provide
the transition evidence about 5–10 seconds after the physical change, plus
collection, heartbeat and delivery scheduling. This is not a delivery guarantee.
The SMS worker limits new send claims to one every 30 seconds; queued concerns
and provider latency can extend the delay. Deliberate and accidental unplugging
are not distinguished by this evidence.

Use expendable demonstration storage for a physical unplug rehearsal; the
monitoring code itself performs no write workload, eject, disconnect, isolation
or remediation. Hardware actions remain with the operator.

## Local verification

Final software checks on 2026-09-13: 345 Rust tests passed; four were excluded
from the default run (three existing helpers and the native USB probe, which
passed separately). Workspace/default and headless checks, rebuilt binaries,
128 web tests, syntax/diff checks and the static contract validator passed.
Nineteen server tests cover the watch transitions and reviewed edge cases.
The final verified-TLS smoke passed with a real passive collector plus synthetic
transitions, including authenticated disk projection and suppressed notifications.
Seven production-panel Browser scenarios and 390-pixel layout checks passed.

The local physical rehearsal passed on one reported enclosure identity:

- Two observations armed the watch and confirmed 5 Gb/s.
- Physical disconnection opened a current loss concern.
- Reconnection at 480 Mb/s resolved loss and opened a separate link warning,
  retaining the same watch and 5 Gb/s baseline.
- Returning to 5 Gb/s resolved the link warning and the loss recurrence caused by
  the second cable change. No concerns remained open.

This was real passive native collection through verified TLS and the production
disk/Attention APIs. Notifications stayed disabled; no real SMS was sent. No
workload, write benchmark, forced eject or automated hardware action was performed.

```sh
bash scripts/cargo-local.sh test --workspace --locked --no-fail-fast
bash scripts/cargo-local.sh check --workspace --locked
bash scripts/cargo-local.sh check --workspace --locked --no-default-features
bash scripts/cargo-local.sh build --workspace --locked
node --test web/tests/*.test.mjs
node scripts/smoke-usb-watch.mjs
```

The USB smoke uses a disposable server, verified loopback TLS and real passive
ciderd collection. It then sends explicitly synthetic USB observations through
the authenticated heartbeat route to exercise disappearance, replug at a slower
link, recovery and receipt replay. It removes Twilio process variables, runs in
private temporary state, keeps notifications disabled, and cleans up only its
own processes and state. It sends no real SMS and does not manipulate hardware.

Two simultaneous drives, another enclosure, Intel/second-Mac deployment, and real
provider authentication/handset delivery remain separate acceptance gates.
