# Disconnection observations

Cider retains distinct evidence for physical inventory removal, USB connection
absence, NFS mount removal, and kernel-reported NFS response problems. These
observations do not establish why a device or mount disappeared, whether the
action was intentional, or whether a node was compromised.

## Physical drives and NFS mount inventory

An accepted explicit resource tombstone can open an attention episode only when
the resource was previously observed in the same node boot, agent generation,
and session. Its source timestamp must be between server time and 90 seconds
earlier. A missing upsert, old-session cleanup, delayed tombstone, or future
timestamp cannot open a new current disconnection concern.

- Physical devices retain `kind=drive_removal` and
  `source_key=drive_removal:<node UUID>:<object UUID>`.
- NFS/NFS4 mount resources use the existing `kind=filesystem` and
  `source_key=mount_removal:<node UUID>:<object UUID>`.
- Evidence includes `classification=physical_drive_removed` or
  `classification=nfs_mount_removed`, `cause=unknown`, original resource
  attributes, the tombstone, boot/session, and receipt timestamp.

The removal remains an observed historical event across later missing polls.
A fresh explicit upsert of the same resource identity resolves it, including
when an earlier stale upsert had already reintroduced that identity into the
graph. Reappearance establishes presence only. A different BSD or mount
identity does not silently resolve the old event.

## USB correlation and notification delivery

USB connection absence still requires two complete, successful observations
after two observed presences. Failure, partial enumeration, stale data, unknown
inventory, or a changed source context cannot establish absence. Two fresh
presences establish recovery.

A physical-removal episode can correlate with USB absence only when retained
inventory establishes exactly one current-context `attached_to` relationship
from the physical disk to one armed USB driver watch. Missing, ambiguous, or
old-context relationships keep the notifications independent.

Each new two-presence confirmation creates a persisted `connection_epoch`.
Correlation requires the same watch and epoch, so an old unresolved physical
event cannot suppress a later USB disappearance after confirmed recovery.
Physical evidence carries `usb_watch_id`; USB evidence carries `watch_id` and
its hashed `connection_context`. Both carry the epoch.

Both supporting attention episodes remain visible, while the secondary
episode's evidence links `notification_correlation.episode_id` to the first.
If the first episode resolves with a queued or pending notification, a still
open, unacknowledged, current correlated episode retains delivery under the
current enabled notification settings. Its evidence records
`notification_transfer_from`. Accepted or uncertain sends are not repeated by
this transfer. For USB evidence, `notification_transfer_pending` persists the
source episode, connection epoch, settings revision, and creation time until a
fresh still-open observation can redeem it, including across server restart.
Unknown observations defer delivery; recovery, acknowledgement, changed settings,
or more than 24 hours prevent redemption. Receipt replays and repeated
observations remain idempotent.

## NFS accessibility

Native mount diagnostics are the owner of NFS accessibility attention:

- A current `dead=true` or `not_responding=true` flag warns immediately.
- A current oldest outstanding request age of at least 30 seconds warns with
  reason `nfs_requests_stalled`. This is a request-age threshold, not a failure
  prediction or average latency.
- Recovery requires current clear flags and current request-age evidence below
  30 seconds. Missing, failed, stale, or offline-owner observations keep an open
  concern unresolved and expose uncertainty.

The existing diagnostics accessibility object adds
`stalled_request_threshold_seconds`, `cause=unknown`, and
`evidence.oldest_request_age_seconds`. An NFS unmount is separate evidence from
an unresponsive mounted share; neither identifies the network failure's cause.

## API and verification

Existing authenticated routes remain unchanged: `GET /api/v1/attention`,
`GET /api/v1/attention/summary`, `GET /api/v1/diagnostics`, and existing node,
filesystem, and disk views. There are no new credentials, request parameters,
top-level response fields, or status codes. The additions above are episode
evidence and existing diagnostics fields. The shared Server Spec requires its
own update; this local note does not replace it.

Synthetic Rust coverage exercises authenticated ingestion, duplicate receipts,
session and timestamp boundaries, stale then fresh recovery, NFS response
failures, USB/physical arrival order, ambiguous mapping, notification transfer,
and a later USB disappearance while an earlier physical event remains open.
No live SMS send or network/device disruption is required by these tests.
