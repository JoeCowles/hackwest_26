# Stored metric history and capacity scenarios

The history reader consumes the server's existing SQLite raw observations and
five-minute/hourly rollups. It adds no collection or backfill. Empty series mean
no stored observations in the selection, not zero activity. The shared Server
Spec remains the API source of truth; this file records implementation assumptions.

## History contract

`GET /api/v1/objects/{object_id}/history` uses the existing authenticated read
API envelope and viewer/admin authorization. The module returns `(data, meta)`;
the router adds the normal response envelope, read budget and authentication.
`metric`, `from` and `to` are required. Dates must be RFC3339. `resolution` is
`raw` (default), `5m` or `1h`. Unknown parameters are rejected. A metric name
contains 1–128 ASCII letters, digits, underscores or periods. Both time endpoints
are inclusive; `from < to <= server now`. Selections must be within the last
24 hours / 30 days / 365 days, respectively. For rollups, these bounds select
bucket **starts**; a selected bucket can extend beyond `to`. Open buckets are
not materialized by maintenance.

Every selected point counts against a combined 2,000-point budget across all
series. A 2,001-row sentinel detects overflow and rejects the request with HTTP
400, code `history_too_large`, rather than truncating it. Narrow the selection or
choose a coarser resolution. Invalid parameters return HTTP 400
`invalid_payload`; unknown objects return HTTP 404 `not_found`. An existing object
with no matching metric observations returns an empty series list. The forecast
endpoint accepts no query parameters. Authentication, authorization and read
budget error behavior is supplied by the shared router.

Each series has its own identity: object, metric, kind, unit, boot, inventory
generation, source, scope and complete labels. Native history labels include
clock and counter-epoch identifiers. No series is merged across any of these
boundaries. Raw byte values and counter integers are returned as exact decimal
strings; stored states, source dates, receipt dates, rate derivation states and
optional derived rates remain explicit. Historical `ok` means usable **at that
observation**, not fresh now. Rollup numeric values are approximate floating
point min/max/mean; counter rollups summarize derived rates per second, while
gauge rollups summarize values. They cannot recover exact original integers.

Rollup `coverage.observed` is the stored valid numeric sample count and
`coverage.expected` is the stored total sample count. This is not an estimate of
expected collector cadence. State counts and unclassified state count remain
visible. A nonempty bucket with incomplete usable observations is `partial`;
zero usable observations are `unavailable`. No missing buckets are synthesized.
Points expose elapsed time from the previous point in that series. `gap_before`
flags absent rollup buckets or a raw gap greater than one hour. Raw collector
cadence is not persisted in the sample table, so the raw flag is a conservative
bound, not proof of uninterrupted telemetry. Renderers should also split on
unusable states, derivation gaps and series boundaries, and expose actual dates.

Existing maintenance retains raw observations for 24 hours rounded back to the
start of the oldest hour (up to one extra hour), five-minute rollups for 30 days,
and hourly rollups for 365 days. The public reader intentionally selects only
the nominal retention window. Maintenance controls when rollups appear and when
expired data is removed; the reader does not generate rollups on demand.

Example `data` (illustrative, not host telemetry):

```json
{
  "object_id": "pool-1",
  "metric": "used_bytes",
  "resolution": "raw",
  "from": "2026-09-13T10:00:00.000Z",
  "to": "2026-09-13T12:00:00.000Z",
  "series": [{
    "identity": {
      "object_id": "pool-1", "boot_id": "boot-1",
      "inventory_generation": "1", "metric": "used_bytes",
      "kind": "gauge", "unit": "bytes", "source": "ciderd:diskutil",
      "scope": "container", "labels": {"ciderd_clock_id": "clock-1"}
    },
    "points": [{
      "observed_at": "2026-09-13T12:00:00.000Z",
      "received_at": "2026-09-13T12:00:01.000Z",
      "value": "9007199254740993", "state": "ok",
      "derived_rate_per_second": null, "derivation_state": "unavailable",
      "gap_before": false, "elapsed_since_previous_seconds": null
    }]
  }]
}
```

## Capacity forecast contract and limitations

`GET /api/v1/objects/{object_id}/capacity-forecast` estimates **capacity
exhaustion**, never a drive failure date. It applies to one APFS container,
APFS volume, mount or NFS mount. It does not sum objects, APFS containers or
shared NFS observers. Use this object attribution when presenting a forecast.

The owner must have a heartbeat within 15 seconds, no goodbye/revocation, and
an active object with the owner's current inventory generation. The reader takes
a consistent database snapshot. All recent capacity observations must use one
current boot/generation and one source/scope/label/kind/unit identity. Any extra
observer, identity transition or unusable observation in the window makes the
result unknown. Used and total observations must have exactly matching dates,
identity, gauge kind and byte unit. Missing pair members are not interpolated.

The model uses at most 2,000 paired raw observations from the last 24 hours. It
requires at least eight distinct dates spanning at least one hour, no gap over
one hour, and positive unchanged capacity with used bytes no greater than total.
It rejects missing/ambiguous/stale observations and capacity changes. Legacy
latest capacity must be at most 90 seconds old. Native latest capacity must
match the history endpoint's actual acquisition, clock, boot and original
inventory generation; source age is monotonic age at receipt plus server elapsed
time, compared with that collector's `stale_after_seconds`. Retained readings
after a failure and absent freshness evidence are rejected.

The model subtracts exact unsigned integers **before** converting differences
to floating point. Ordinary least squares estimates positive used-byte growth.
Every observed interval must grow positively, maximum interval growth must be
at most three times minimum growth, and the linear fit must have R² ≥ 0.9.
These intentionally conservative guards reject stepwise cleanup and unstable
workloads. The model does not promise an estimate for every capacity object.

Remaining exact bytes divided by growth determines the date relative to the
latest observation. The scenario range uses minimum and maximum observed
interval growth, widened by a 25% margin. This is a heuristic scenario range,
**not a statistical confidence interval**. All dates must be future dates within
365 days of the last observation; otherwise the horizon is unreliable. Full
capacity produces `capacity_exhausted` with no predicted date. The model makes
no allowance for changed workloads, cleanup, snapshots or added capacity.

Unknown results have `state: "unknown"`, explicit `reasons`, null
`estimated_exhaustion_at` and null `scenario_range`. Available basis is retained.
Reasons include `ineligible_object`, `owner_unavailable`, `identity_changed`,
`history_too_large`, `unusable_observation`, `invalid_capacity`,
`ambiguous_series`, `insufficient_data`, `incompatible_observations`,
`insufficient_span`, `history_gap`, `capacity_changed`, `freshness_unavailable`,
`stale_capacity`, `capacity_exhausted`, `no_reliable_growth`, `unstable_growth`
and `unreliable_horizon`.

An estimated result has this shape (numbers and dates are illustrative):

```json
{
  "object_id": "pool-1", "state": "estimated", "reasons": [],
  "estimated_exhaustion_at": "2026-09-14T01:50:00.000Z",
  "scenario_range": {
    "earliest_at": "2026-09-13T23:04:00.000Z",
    "latest_at": "2026-09-14T06:26:40.000Z"
  },
  "basis": {
    "sample_count": 8,
    "observed_from": "2026-09-13T10:50:00.000Z",
    "observed_to": "2026-09-13T12:00:00.000Z",
    "observation_span_seconds": 4200,
    "capacity_bytes": "1000", "used_bytes": "170", "remaining_bytes": "830",
    "growth_bytes_per_second": 0.016666666666666666,
    "fit_r_squared": 1.0,
    "scenario_growth_bytes_per_second": {"low": 0.0125, "high": 0.020833333333333332},
    "series_identity": {
      "object_id": "pool-1", "boot_id": "boot-1", "inventory_generation": "1",
      "kind": "gauge", "unit": "bytes", "source": "example",
      "scope": "container", "labels": {}
    }
  },
  "assumptions": ["Capacity exhaustion only; no drive failure prediction",
    "Capacity and recent positive growth continue unchanged"]
}
```

## Validation scope

Focused SQLite integration tests exercise real module reads against temporary
stores: precision and series isolation, states and rollup coverage, time/point
bounds, linear and rejected forecasts, native freshness and exact arithmetic.
They do not constitute authenticated browser integration, a live production
forecast validation, or final shared Google Doc verification. Root integration
owns those checks and the shared API publication.
