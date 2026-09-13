import { ApiClient } from './api.js';
import { rateValueLabel, timeLabel } from './model.js';
import { CursorPages } from './session.js';

export const SECURITY_COLLECTIONS = Object.freeze({
  findings: Object.freeze({ path: '/api/v1/findings', params: Object.freeze({ status: 'all' }) }),
  sources: Object.freeze({ path: '/api/v1/detectors/storage-activity/sources', params: Object.freeze({}) }),
  ruleFindings: Object.freeze({ path: '/api/v1/security/rule-findings', params: Object.freeze({ status: 'all' }) }),
  ruleSources: Object.freeze({ path: '/api/v1/security/rule-sources', params: Object.freeze({}) }),
  events: Object.freeze({ path: '/api/v1/events', params: Object.freeze({ category: 'security' }) })
});

const record = value => value && typeof value === 'object' && !Array.isArray(value);
const text = value => typeof value === 'string' && value.length ? value : null;
const finite = value => value == null || typeof value === 'boolean' || (typeof value === 'string' && !value.trim())
  ? null : Number.isFinite(Number(value)) ? Number(value) : null;
const words = value => text(value)?.replaceAll('_', ' ') || 'unknown';

export function durationLabel(seconds) {
  const amount = finite(seconds);
  if (amount == null || amount < 0) return 'Unknown duration';
  const total = Math.round(amount), parts = [];
  if (total >= 3600) parts.push(`${Math.floor(total / 3600)}h`);
  if (total >= 60) parts.push(`${Math.floor(total % 3600 / 60)}m`);
  parts.push(`${total % 60}s`);
  return parts.join(' ');
}

const rate = value => rateValueLabel(finite(value));
const reasonLabel = value => words(value);

function validSource(value) {
  return record(value) && ['source_id', 'node_id', 'object_id', 'resource_id', 'direction', 'metric', 'rule_id', 'policy_version', 'baseline_revision'].every(key => text(value[key]))
    && ['read', 'write'].includes(value.direction) && typeof value.active === 'boolean'
    && text(value.support_state) && record(value.baseline) && record(value.episode) && record(value.observation);
}

export function sourceVM(value, context = {}) {
  if (!validSource(value)) return { valid: false, reason: 'Source row does not match the detector contract.' };
  const baseline = value.baseline, observation = value.observation;
  const baselineState = ['learning', 'ready'].includes(baseline.state) ? baseline.state : 'unknown';
  const covered = finite(baseline.covered_seconds), required = finite(baseline.required_seconds);
  const intervals = finite(baseline.interval_count), requiredIntervals = finite(baseline.required_intervals);
  const readinessLabel = baselineState === 'learning'
    ? `Learning · ${covered ?? 'unknown'} / ${required ?? 'unknown'} seconds covered · ${intervals ?? 'unknown'} / ${requiredIntervals ?? 'unknown'} intervals`
    : baselineState === 'ready'
      ? `Ready · reference ${rate(baseline.reference_rate_bytes_per_second)} · ${durationLabel(covered)} across ${intervals ?? 'unknown'} intervals`
      : 'Baseline state unknown';
  const thresholdLabel = baselineState === 'ready'
    ? `Opens > ${rate(baseline.high_threshold_bytes_per_second)} after sustained elevation · recovers ≤ ${rate(baseline.recovery_threshold_bytes_per_second)}`
    : 'Thresholds are unavailable until learning is ready.';
  const snapshotObservationState = ['current', 'stale', 'unavailable'].includes(observation.state) ? observation.state : 'unknown';
  const startedAt = finite(context.startedAt), startedWallAt = finite(context.startedWallAt);
  const now = finite(context.now), wallNow = finite(context.wallNow);
  const elapsed = [startedAt != null && now != null ? now - startedAt : null,
    startedWallAt != null && wallNow != null ? wallNow - startedWallAt : null]
    .filter(value => value != null);
  const elapsedSeconds = elapsed.length ? Math.max(0, ...elapsed) / 1000 : null;
  const snapshotAgeSeconds = finite(observation.age_seconds), staleAfterSeconds = finite(observation.stale_after_seconds);
  const localAgeSeconds = snapshotAgeSeconds != null && elapsedSeconds != null ? snapshotAgeSeconds + elapsedSeconds : snapshotAgeSeconds;
  const agedLocally = snapshotObservationState === 'current' && localAgeSeconds != null && staleAfterSeconds != null && localAgeSeconds > staleAfterSeconds;
  const observationState = agedLocally ? 'stale' : snapshotObservationState;
  const observed = timeLabel(observation.observed_at), observedRate = rate(observation.rate_bytes_per_second);
  const ageDetail = localAgeSeconds == null ? 'local age unknown'
    : `${durationLabel(localAgeSeconds)} old${staleAfterSeconds == null ? '' : `; stale after ${durationLabel(staleAfterSeconds)}`}`;
  const observationLabel = agedLocally
    ? `Stale (aged locally) · was current at snapshot · last rate ${observedRate} · observed ${observed} · ${ageDetail}`
    : snapshotObservationState === 'current'
      ? `Current at snapshot · ${observedRate} · observed ${observed} · ${ageDetail}`
      : snapshotObservationState === 'stale'
        ? `Stale at snapshot · last rate ${observedRate} · observed ${observed} · ${reasonLabel(observation.reason)}`
        : `Unavailable at snapshot · ${reasonLabel(observation.reason)} · last observation ${observed}`;
  const supportState = value.support_state;
  const supportLabel = supportState === 'supported' ? 'Supported'
    : `${reasonLabel(supportState)} · ${reasonLabel(value.support_reason || value.reason)}`;
  return {
    valid: true, raw: value,
    sourceId: value.source_id, nodeId: value.node_id, objectId: value.object_id,
    resourceId: value.resource_id, direction: value.direction, metric: value.metric,
    ruleId: value.rule_id, policyVersion: value.policy_version,
    baselineRevision: value.baseline_revision, active: value.active,
    supportState, supportLabel, baselineState, readinessLabel, thresholdLabel,
    readinessAsOfLabel: `Readiness and coverage evaluated ${timeLabel(baseline.as_of)}`,
    episodeState: text(value.episode.state) || 'unknown',
    episodeFindingId: text(value.episode.finding_id),
    episodeLabel: `${words(value.episode.state)} · ${durationLabel(value.episode.elevated_seconds)} elevated · ${durationLabel(value.episode.recovery_seconds)} recovery`,
    snapshotObservationState, observationState, observationLabel, observedLabel: observed,
    localAgeSeconds,
    observedRateLabel: observedRate
  };
}

function validInterval(value) {
  return record(value) && text(value.collection_id) && text(value.previous_collection_id)
    && text(value.counter_start) && text(value.counter_end)
    && text(value.monotonic_start_ns) && text(value.monotonic_end_ns)
    && finite(value.interval_seconds) != null && finite(value.rate_bytes_per_second) != null
    && text(value.observed_at) && text(value.received_at) && record(value.continuity);
}

export function intervalVM(value) {
  if (!validInterval(value)) return null;
  return {
    raw: value, collectionId: value.collection_id,
    previousCollectionId: value.previous_collection_id,
    counterStart: value.counter_start, counterEnd: value.counter_end,
    monotonicStart: value.monotonic_start_ns, monotonicEnd: value.monotonic_end_ns,
    intervalSeconds: Number(value.interval_seconds),
    intervalLabel: durationLabel(value.interval_seconds),
    rate: Number(value.rate_bytes_per_second), rateLabel: rate(Number(value.rate_bytes_per_second)),
    observedLabel: timeLabel(value.observed_at), receivedLabel: timeLabel(value.received_at),
    continuity: value.continuity
  };
}

function validFinding(value) {
  return record(value) && ['finding_id', 'source_id', 'node_id', 'object_id', 'resource_id', 'direction', 'rule_id', 'status', 'severity', 'summary', 'first_seen_at', 'last_seen_at', 'opened_at', 'updated_at', 'baseline_revision'].every(key => text(value[key]))
    && ['open', 'resolved', 'interrupted'].includes(value.status)
    && value.severity === 'warning' && record(value.policy) && record(value.baseline)
    && record(value.evidence) && validInterval(value.evidence.first)
    && validInterval(value.evidence.latest) && validInterval(value.evidence.peak);
}

export function findingVM(value) {
  if (!validFinding(value)) return { valid: false, reason: 'Finding row does not match the storage-activity contract.' };
  const baseline = value.baseline;
  return {
    valid: true, raw: value,
    findingId: value.finding_id, sourceId: value.source_id, nodeId: value.node_id,
    objectId: value.object_id, resourceId: value.resource_id,
    direction: value.direction, ruleId: value.rule_id, status: value.status,
    severity: value.severity, summary: value.summary,
    firstSeenLabel: timeLabel(value.first_seen_at), lastSeenLabel: timeLabel(value.last_seen_at),
    openedLabel: timeLabel(value.opened_at), updatedLabel: timeLabel(value.updated_at),
    endedLabel: timeLabel(value.ended_at), reasonLabel: value.reason ? reasonLabel(value.reason) : 'Not ended',
    baselineRevision: value.baseline_revision, policy: value.policy,
    baselineLabel: `Reference ${rate(baseline.reference_rate_bytes_per_second)} · opens > ${rate(baseline.high_threshold_bytes_per_second)} · recovers ≤ ${rate(baseline.recovery_threshold_bytes_per_second)}`,
    baselineCoverageLabel: `${durationLabel(baseline.covered_seconds)} across ${finite(baseline.interval_count) ?? 'unknown'} intervals`,
    evidence: {
      first: intervalVM(value.evidence.first), latest: intervalVM(value.evidence.latest), peak: intervalVM(value.evidence.peak)
    },
    elevatedLabel: `${durationLabel(value.evidence.elevated_seconds)} elevated`,
    recoveryLabel: `${durationLabel(value.evidence.recovery_seconds)} recovery`
  };
}

export function ruleFindingVM(value) {
  const valid = record(value) && ['finding_id', 'source_id', 'node_id', 'object_id', 'resource_id', 'scope', 'rule_id', 'status', 'severity', 'summary', 'first_seen_at', 'last_seen_at', 'updated_at'].every(key => text(value[key]))
    && ['open', 'resolved', 'interrupted'].includes(value.status)
    && ['warning', 'critical'].includes(value.severity) && record(value.evidence) && record(value.evidence.features);
  if (!valid) return { valid: false, reason: 'Finding row does not match the fixed-rule contract.' };
  return {
    valid: true, raw: value, findingId: value.finding_id, nodeId: value.node_id,
    objectId: value.object_id, scope: words(value.scope), ruleId: value.rule_id,
    status: value.status, severity: value.severity, summary: value.summary,
    firstSeenLabel: timeLabel(value.first_seen_at), lastSeenLabel: timeLabel(value.last_seen_at),
    updatedLabel: timeLabel(value.updated_at), endedLabel: timeLabel(value.ended_at),
    features: value.evidence.features, requiredIntervals: finite(value.evidence.required_intervals),
    recoveryIntervals: finite(value.evidence.recovery_intervals)
  };
}

export function ruleSourceVM(value) {
  const valid = record(value) && ['source_id', 'node_id', 'object_id', 'resource_id', 'collector', 'scope', 'policy_version'].every(key => text(value[key]))
    && typeof value.active === 'boolean' && record(value.features) && Array.isArray(value.rules) && record(value.observation);
  if (!valid) return { valid: false, reason: 'Source row does not match the fixed-rule contract.' };
  return { valid: true, raw: value, sourceId: value.source_id, nodeId: value.node_id, objectId: value.object_id,
    collector: value.collector, scope: words(value.scope), active: value.active, features: value.features,
    rules: value.rules, observationState: text(value.observation.state) || 'unknown',
    observedLabel: timeLabel(value.observation.observed_at) };
}

function validEvent(value) {
  return record(value) && text(value.event_id) && text(value.occurred_at)
    && text(value.severity) && text(value.source) && text(value.summary);
}

export function securityPageVM(kind, snapshot = {}, now = performance.now(), wallNow = Date.now()) {
  const transform = kind === 'findings' ? findingVM : kind === 'sources'
    ? value => sourceVM(value, { startedAt: snapshot.startedAt, startedWallAt: snapshot.startedWallAt, now, wallNow })
    : kind === 'ruleFindings' ? ruleFindingVM : kind === 'ruleSources' ? ruleSourceVM
    : value => validEvent(value) ? { valid: true, raw: value } : { valid: false };
  const transformed = (Array.isArray(snapshot.rows) ? snapshot.rows : []).map(transform);
  const invalidCount = transformed.filter(row => !row.valid).length;
  return {
    ...snapshot,
    rows: transformed.filter(row => row.valid).map(row => kind === 'events' ? row.raw : row),
    invalidCount,
    contractError: invalidCount ? `${invalidCount} invalid ${kind.endsWith('Sources') || kind === 'sources' ? 'detector source' : 'finding'} row${invalidCount === 1 ? '' : 's'} omitted; the response did not match the documented contract.` : ''
  };
}

function controls(page, freshness) {
  return {
    ...page.snapshot(), ...freshness,
    previous: () => page.previous(), next: () => page.next(), refresh: () => page.refresh()
  };
}

/** Three independent cursor owners keep generated findings, detector coverage,
 * and collector-originated events separate. Closing the session aborts every
 * owner and prevents an obsolete viewer credential from publishing late rows.
 */
export class SecurityPages {
  constructor(token, onChange = () => {}, onAuth = () => {}, clientFactory = value => new ApiClient(value), now = () => performance.now(), wallNow = () => Date.now()) {
    this.onChange = onChange;
    this.now = now;
    this.wallNow = wallNow;
    this.closed = false;
    this.freshness = {};
    this.pendingStarts = {};
    this.pages = Object.fromEntries(Object.entries(SECURITY_COLLECTIONS).map(([name, config]) => {
      const rawClient = clientFactory(token);
      const client = {
        close: () => rawClient.close?.(),
        get: (path, params) => {
          if (!params?.cursor) this.pendingStarts[name] = { startedAt: this.now(), startedWallAt: this.wallNow() };
          return rawClient.get(path, params);
        }
      };
      const page = new CursorPages(client, config.path, config.params,
        () => {
          if (this.closed) return;
          const snapshot = page.snapshot(), pending = this.pendingStarts[name];
          if (!snapshot.busy && pending) {
            if (!snapshot.error) this.freshness[name] = pending;
            delete this.pendingStarts[name];
          }
          this.onChange(this.snapshot());
        }, onAuth);
      return [name, page];
    }));
  }
  snapshot() { return Object.fromEntries(Object.entries(this.pages).map(([name, page]) => [name, controls(page, this.freshness[name])])); }
  refreshAll() { return Promise.all(Object.values(this.pages).map(page => page.refresh())); }
  close() { this.closed = true; Object.values(this.pages).forEach(page => page.close()); }
}
