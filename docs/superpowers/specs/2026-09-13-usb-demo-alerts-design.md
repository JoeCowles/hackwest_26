# USB disappearance and connection-speed concerns

The user authorized work toward demos 3 and 4 after the readiness assessment:
notify the operator when a USB drive disappears and when its connection is
degraded by a slower USB link. This is local implementation and verification;
it does not authorize real SMS, deployment, commits, or publishing raw telemetry.

## Approach

Extend the existing five-second IOKit worker with bounded USB ancestor evidence.
Use a separate durable server device-watch policy, independent of statistical
service-time and SMART/media-health rules. Feed its conditions to the existing
Attention and Twilio outbox. Show connection readiness in the existing disk view.

Simple low-throughput thresholds cannot establish a USB downshift. Reusing the
service-time baseline would lose comparison continuity on reconnect. Instead,
compare a reported negotiated speed with the same identified USB connection's
highest previously confirmed observed speed. Cold-start slow connections establish
a baseline and do not warn. This detects transport degradation, not media failure.

## Collector contract

Add public pure types in `ciderd::device_snapshot` with serde strict fields and
explicit `validate()`:

```rust
pub struct DeviceSnapshot {
    pub version: u32,              // exactly 1
    pub complete: bool,            // complete USB ancestry enumeration
    pub devices: Vec<UsbDevice>,   // at most 128, encoded snapshot <= 65536 bytes
}
pub struct UsbDevice {
    pub identity: String,          // opaque UUID, node-scoped
    pub identity_basis: String,    // reported_usb_serial | boot_registry
    pub identity_scope: String,    // usb_enclosure | driver_incarnation
    pub driver_resource_id: String,
    pub bsd_name: Option<String>,  // one proven whole-media child only
    pub negotiated_bps: Option<String>, // canonical positive decimal
    pub speed_state: String,      // available | unknown | unsupported
    pub reason: Option<String>,   // bounded explanation code
}
```

The host-scoped `iokit.block` collection has
`extensions.usb_device_snapshot = DeviceSnapshot`. Collection identity, source
monotonic clock, acquisition date and status govern this snapshot; unchanged
resource attributes cannot restamp it. An empty successful enumeration must
publish a complete empty snapshot. Old collectors without this extension are
unsupported. A partial or failed lookup must not certify absence.

Walk unique IOService parents, at most 32 levels per driver and 4096 ancestor
visits per acquisition, stopping at the nearest IOUSBHostDevice. Check iterator
validity and reject ambiguous/cyclic/racing ancestry. Read USBSpeed using Apple's
IOUSBHost connection-speed enum: 1=12 Mb/s, 2=1.5 Mb/s, 3=480 Mb/s, 4=5 Gb/s,
5=10 Gb/s, 6=20 Gb/s. Zero/missing/unknown codes are unavailable or unsupported,
never zero activity. Do not use bcdUSB or forced UsbLinkSpeed.

Reported USB serial plus VID/PID can identify an enclosure. Derive a node-scoped
opaque UUID using the existing namespaced identity mechanism; never serialize
raw serials into heartbeat resources, metrics, errors or public artifacts.
Raw properties may exist only in the private worker-to-parser acquisition pipe,
as with existing smartctl acquisition. This is a reported enclosure identity,
not verified media identity. If unavailable, use boot/registry identity for
same-session presence only, with link comparison disabled. Duplicate stable
identities (including multi-LUN ambiguity) must not transfer baselines/recovery;
mark the snapshot partial rather than infer missing identities.

Keep existing driver IDs, counter epochs, catalog and acquisition behavior.
USB-lookup failure preserves usable I/O counters. USB devices with unknown speed
can still establish presence when ancestry/identity enumeration is complete.

## Server policy

Persist a bounded node acquisition record and up to 128 watched USB devices per
node / 8192 globally in schema migration 006. Newer-schema protection advances
to 6. Device state keys combine node and opaque identity. Preserve confirmed
baselines and unresolved episodes through server restart. Bound per-row JSON and
expose admission-limited/unknown status; never evict unresolved concerns silently.

Process the latest distinct host IOKit attempt inside the accepted heartbeat
transaction. Require graph-known references, matching clock/session, current
monotonic acquisition age <=15 seconds, status ok and a complete valid extension.
Reject mismatched resource references; unsupported/malformed optional evidence
does not invalidate otherwise valid telemetry but makes the watch unavailable.
Repeated collection IDs or monotonic acquisitions never advance streaks or age.
Source gaps over 15 seconds, failed/partial/stale acquisition and context changes
break pending streaks. A new collector session must establish presence before
it can declare new loss; retained episodes do not become recovered by omission.

Presence arms after two fresh distinct observations at least one source second
apart. Two consecutive complete fresh absences open a warning. Two positive
observations of the same identity resolve it. Repeated absence creates no duplicate
episode/send. Weak boot-registry identities cannot recover through a new registry
identity; explain that limitation. The statement is "Previously observed USB
storage connection is absent", without claiming failure or unauthorized removal.

Two matching fresh speed observations confirm a baseline. Retain the highest
confirmed speed and evidence. Two observations below it open a link warning;
two at or above it recover. One high outlier cannot raise the baseline. Only
reported_usb_serial / usb_enclosure identities compare across reconnects. Missing
speed, absence, source failure or node loss breaks streaks and leaves existing
link episodes unresolved with unknown/stale evidence.

Use stable Attention source keys `device_presence:<watch_id>` and
`usb_link:<watch_id>`, kind `reliability`, and explicit rule/classification fields.
These are connection-watch concerns, separate from `/reliability/findings`.
Use existing acknowledgement, suppression, recurrence and SMS worker behavior.
Preserve node/object attribution and dated previous/current link evidence.

## Read projection and UI

Existing disk summaries add nullable `device_watch` only for a unique current
driver association. Shape:

```json
{
  "watch_id":"opaque", "identity_basis":"reported_usb_serial",
  "identity_scope":"usb_enclosure", "presence":"present",
  "armed":true, "link_state":"no_current_warning",
  "negotiated_bps":"5000000000", "baseline_bps":"5000000000",
  "reason":null,
  "observation":{"state":"current","observed_at":"RFC3339",
    "received_at":"RFC3339","age_seconds":0,"stale_after_seconds":15},
  "evidence":{}
}
```

Presence is present/absent/unknown. Link state is warming_up,
no_current_warning/warning/unknown/unsupported. Read projections independently
age source evidence and owner availability; stale data cannot claim current
presence or a current link. No attributable watch is null and rendered as
unavailable, not healthy. The UI retains exact bitrates and provides human units,
identity scope, source time and baseline readiness. Browser refresh failure and
sleep age the projection independently of disk I/O linkage readiness. Absent
devices remain represented in Attention even after active disk rows disappear.

No new routes or mutation controls are required. Update the shared Server Spec
with full extension/projection/lifecycle contracts, auth/status context, examples
and actual verification status. Native Google Docs save/export is distinct from
a connector write; no connector capability was discovered.

## Acceptance

Tests first: native/parser speed and privacy cases; complete empty vs partial
enumeration; identity collision and weak identity; two-observation loss/recovery;
reconnect to slower speed; cold-start slow baseline; outlier; replay; stale/gap/
session change; node loss; persistence and bounded admission. Authenticated
synthetic heartbeat tests must cover atomic state/attention/outbox and deduplication.
Frontend tests cover unknown/failure/aging, exact rates, weak identity and no
dependence on I/O linkage. Run full Cargo tests/checks, web tests and rebuilt
embedded assets, then passive native TLS smoke and rendered browser checks.
No real SMS or physical unplug is performed without the corresponding hardware
and user action. Synthetic acceptance is explicitly labeled.
