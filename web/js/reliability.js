import { timeLabel } from './model.js';

const record = value => value && typeof value === 'object' && !Array.isArray(value);
const finite = value => value == null || typeof value === 'boolean' || (typeof value === 'string' && !value.trim())
  ? null : Number.isFinite(Number(value)) ? Number(value) : null;
const words = value => typeof value === 'string' && value.length ? value.replaceAll('_', ' ') : 'unknown';
const count = value => Number.isInteger(value) && value >= 0 ? value : null;
const observationStates = new Set(['current', 'stale', 'unavailable', 'unknown']);
const signalLabels = Object.freeze({
  'smart.overall': 'SMART overall health',
  'nvme.critical_warning': 'NVMe critical warning',
  'ata.pending_sectors': 'Pending sectors',
  'ata.uncorrectable_sectors': 'Offline uncorrectable sectors',
  'nvme.endurance_used': 'Endurance used',
  'nvme.spare_below_threshold': 'Available spare threshold',
  'iokit.read_errors': 'Read errors',
  'iokit.write_errors': 'Write errors',
  'iokit.read_retries': 'Read retries',
  'iokit.write_retries': 'Write retries',
  'nvme.media_errors': 'NVMe media errors',
  'ata.reallocated_increase': 'Reallocated sectors increase',
  'nvme.spare_decline': 'Available spare decline',
  'iokit.read_service_time': 'Read service time',
  'iokit.write_service_time': 'Write service time'
});

function ruleLabel(ruleId) {
  if (signalLabels[ruleId]) return signalLabels[ruleId];
  if (typeof ruleId !== 'string' || !ruleId.length) return 'Reliability signal';
  const acronyms = { iokit: 'IOKit', nvme: 'NVMe', ata: 'ATA', smart: 'SMART' };
  return ruleId.split(/[._-]+/).filter(Boolean).map((token, index) =>
    acronyms[token.toLowerCase()] || (index === 0 ? token[0].toUpperCase() + token.slice(1) : token)
  ).join(' ');
}

export function durationLabel(seconds) {
  const value = finite(seconds);
  if (value == null || value < 0) return 'age unknown';
  if (value < 60) return `${Math.round(value)} seconds`;
  if (value < 3600) return `${Math.round(value / 60)} minutes`;
  return `${Math.round(value / 3600)} hours`;
}

/** Age a frozen observation with browser elapsed time. The returned object is a
 * display copy: original states, exact evidence and source dates are untouched.
 */
export function ageObservation(value, { snapshotAgeMs = 0, current = true } = {}) {
  const raw = record(value) ? value : {};
  const snapshotState = observationStates.has(raw.state) ? raw.state : 'unknown';
  const sourceAge = finite(raw.age_seconds);
  const elapsed = Math.max(0, finite(snapshotAgeMs) ?? 0) / 1000;
  const ageSeconds = sourceAge == null ? null : sourceAge + elapsed;
  const staleAfterSeconds = finite(raw.stale_after_seconds);
  const expired = snapshotState === 'current' && staleAfterSeconds != null
    && ageSeconds != null && ageSeconds > staleAfterSeconds;
  const retained = snapshotState === 'current' && current !== true;
  const state = expired || retained ? 'stale' : snapshotState;
  const reason = retained ? 'snapshot_not_current' : expired ? 'age_exceeded' : raw.reason ?? null;
  const stateLabel = state === 'current' ? 'Current'
    : state === 'stale' ? 'Stale'
      : state === 'unavailable' ? 'Unavailable' : 'Unknown';
  const reasonText = reason ? ` · ${words(reason)}` : '';
  const ageText = ageSeconds == null ? 'age unknown'
    : `${durationLabel(ageSeconds)} old${staleAfterSeconds == null ? '' : `; stale after ${durationLabel(staleAfterSeconds)}`}`;
  return {
    ...raw, snapshotState, state, reason, ageSeconds, staleAfterSeconds,
    label: `${stateLabel}${snapshotState === 'current' && state === 'stale' ? ' (was current at snapshot)' : ''}${reasonText} · ${ageText}`,
    observedLabel: timeLabel(raw.observed_at), receivedLabel: timeLabel(raw.received_at)
  };
}

function performanceEvidenceVM(evidence) {
  if (!record(evidence)) return {
    readinessState: 'unknown', readinessLabel: 'Baseline readiness unavailable',
    thresholdLabel: 'Service-time thresholds unavailable', workloadLabel: 'Workload bucket unavailable',
    episodeLabel: 'Episode duration unavailable'
  };
  const covered = finite(evidence.baseline_covered_seconds), required = finite(evidence.required_baseline_seconds);
  const intervals = finite(evidence.baseline_interval_count), requiredIntervals = finite(evidence.required_baseline_intervals);
  const reference = finite(evidence.reference_ns_per_operation);
  const high = finite(evidence.high_threshold_ns_per_operation), recovery = finite(evidence.recovery_threshold_ns_per_operation);
  const bucket = record(evidence.workload_bucket) ? evidence.workload_bucket : {};
  const transferBand = finite(bucket.transfer_size_band), operationBand = finite(bucket.operation_rate_power_of_two_band);
  const ready = reference != null && covered != null && required != null && covered >= required
    && intervals != null && requiredIntervals != null && intervals >= requiredIntervals;
  return {
    readinessState: ready ? 'ready' : 'learning',
    readinessLabel: `${ready ? 'Ready' : 'Learning'} · ${covered ?? 'unknown'} / ${required ?? 'unknown'} seconds · ${intervals ?? 'unknown'} / ${requiredIntervals ?? 'unknown'} intervals${reference == null ? '' : ` · reference ${reference.toLocaleString()} ns/op`}`,
    thresholdLabel: high == null || recovery == null ? 'Service-time thresholds unavailable'
      : `Opens above ${high.toLocaleString()} ns/op · recovers at or below ${recovery.toLocaleString()} ns/op`,
    workloadLabel: transferBand == null || operationBand == null ? 'Workload bucket unavailable'
      : `Transfer size band ${transferBand} · operation rate power-of-two band ${operationBand}`,
    episodeLabel: `${finite(evidence.elevated_seconds) ?? 'unknown'} seconds elevated · ${finite(evidence.recovery_seconds) ?? 'unknown'} seconds recovery`
  };
}

function signalVM(value, context) {
  const raw = record(value) ? value : {};
  const observation = ageObservation(raw.observation, context);
  const performance = raw.dimension === 'performance' ? performanceEvidenceVM(raw.evidence) : {};
  const ruleId = raw.rule_id || null;
  return {
    raw, ruleId: ruleId || 'Rule not reported', title: raw.reason ? words(raw.reason) : ruleLabel(ruleId),
    dimension: raw.dimension || 'unknown',
    state: raw.state || 'unknown', severity: raw.severity || 'unknown',
    reason: raw.reason || null, reasonLabel: words(raw.reason), findingId: raw.finding_id || null,
    observation, observationState: observation.state, evidence: record(raw.evidence) ? raw.evidence : null,
    ...performance
  };
}

function sourceVM(value, context) {
  const raw = record(value) ? value : {};
  const observation = ageObservation(raw.observation, context);
  const sourceId = raw.source ?? raw.source_id ?? null;
  const nodeId = raw.node ?? raw.node_id ?? null;
  const objectId = raw.object ?? raw.object_id ?? null;
  return {
    raw, sourceId, nodeId, objectId,
    resourceId: raw.resource ?? raw.resource_id ?? null,
    collector: raw.collector || 'Collector not reported', scope: raw.scope || 'Scope not reported',
    confidence: raw.device_identity_confidence ?? raw.identity_confidence ?? raw.confidence ?? raw.attribution_confidence ?? null,
    observation, observationState: observation.state,
    signals: (Array.isArray(raw.signals) ? raw.signals : []).map(signal => signalVM(signal, context)),
    policy: record(raw.policy) ? raw.policy : null
  };
}

function findingVM(value, context, sources) {
  const raw = record(value) ? value : {};
  const sourceId = raw.source_id ?? raw.source ?? null;
  const source = sources.find(candidate => candidate.sourceId === sourceId);
  const signal = source?.signals.find(candidate => candidate.findingId === raw.finding_id);
  const findingObservation = record(raw.observation) ? ageObservation(raw.observation, context) : null;
  const supported = source ? source.observationState === 'current' && (!signal || signal.observationState === 'current')
    : findingObservation ? findingObservation.state === 'current' : false;
  return {
    raw, findingId: raw.finding_id || null, sourceId,
    nodeId: raw.node_id ?? raw.node ?? source?.nodeId ?? null,
    objectId: raw.object_id ?? raw.object ?? source?.objectId ?? null,
    resourceId: raw.resource_id ?? raw.resource ?? source?.resourceId ?? null,
    ruleId: raw.rule_id || 'Rule not reported', dimension: raw.dimension || 'unknown',
    classification: raw.classification || 'unknown', severity: raw.severity || 'unknown',
    status: raw.status || 'unknown', summary: raw.summary || raw.reason || 'Finding summary not reported.',
    policyVersion: raw.policy_version || 'Not reported',
    scope: raw.scope || source?.scope || 'Not reported',
    confidence: raw.device_identity_confidence ?? raw.identity_confidence ?? source?.confidence ?? null,
    current: raw.current === true && context.current === true && supported,
    firstSeenLabel: timeLabel(raw.first_seen_at), lastSeenLabel: timeLabel(raw.last_seen_at),
    openedLabel: timeLabel(raw.opened_at), updatedLabel: timeLabel(raw.updated_at),
    endedLabel: timeLabel(raw.ended_at), reasonLabel: raw.reason ? words(raw.reason) : 'Not ended',
    evidence: record(raw.evidence) ? raw.evidence : null,
    observation: findingObservation
  };
}

function assessmentLabel(assessment, hasCurrentConcern, current) {
  if (assessment === 'critical') return hasCurrentConcern && current ? 'Critical reliability evidence' : 'Dated critical evidence';
  if (assessment === 'warning') return hasCurrentConcern && current ? 'Reliability warning' : 'Dated warning evidence';
  if (assessment === 'no_current_warning') return current
    ? 'No current warning in observed dimensions' : 'Dated snapshot reported no warning in observed dimensions';
  return 'Reliability assessment unknown';
}

function forecastLabel(value) {
  if (!record(value) || typeof value.state !== 'string') return 'Replacement forecast unavailable';
  if (value.state === 'insufficient_data') return 'Replacement timing cannot be estimated from the available evidence.';
  if (value.estimated_failure_at) return `${words(value.state)} · estimated ${timeLabel(value.estimated_failure_at)}`;
  return `${words(value.state)} · no failure date reported`;
}

/** Normalize the fixed DiskSummary reliability shape for rendering. Missing
 * fields remain unavailable, and all freshness derivation is local-only.
 */
export function reliabilityVM(value, { snapshotAgeMs = 0, current = true } = {}) {
  if (!record(value)) return {
    available: false, assessment: 'unknown', assessmentLabel: 'Reliability assessment unavailable',
    observationState: 'unavailable', sourceCount: null, currentSourceCount: null,
    coverageLabel: 'Source coverage unavailable', sources: [], findings: [], signals: [],
    unknownDimensions: ['media_health', 'performance'],
    forecastLabel: 'Replacement forecast unavailable', forecast: null
  };
  const context = { snapshotAgeMs, current };
  const sources = (Array.isArray(value.sources) ? value.sources : []).map(source => sourceVM(source, context));
  const sourceCount = count(value.source_count), reportedCurrentCount = count(value.current_source_count);
  const locallyCurrentSources = sources.filter(source => source.observationState === 'current').length;
  const currentSourceCount = reportedCurrentCount == null ? null
    : sources.length ? Math.min(reportedCurrentCount, locallyCurrentSources) : reportedCurrentCount;
  const findings = (Array.isArray(value.findings) ? value.findings : []).map(row => findingVM(row, context, sources));
  const signals = sources.flatMap(source => source.signals.map(signal => ({ ...signal, source })));
  const hasCurrentConcern = findings.some(row => row.current && row.status === 'open')
    || signals.some(signal => signal.observationState === 'current' && ['warning', 'critical', 'open'].includes(signal.state));
  const reportedState = observationStates.has(value.observation_state) ? value.observation_state : 'unknown';
  const observationState = reportedState === 'current' && (current !== true || (sources.length && currentSourceCount === 0))
    ? 'stale' : reportedState;
  const assessment = ['unknown', 'no_current_warning', 'warning', 'critical'].includes(value.assessment)
    ? value.assessment : 'unknown';
  const unknownDimensions = Array.isArray(value.unknown_dimensions)
    ? value.unknown_dimensions.filter(dimension => typeof dimension === 'string') : [];
  return {
    available: true, raw: value, assessment,
    assessmentLabel: assessmentLabel(assessment, hasCurrentConcern, observationState === 'current'),
    observationState, sourceCount, currentSourceCount,
    coverageLabel: sourceCount == null || currentSourceCount == null ? 'Source coverage unavailable'
      : `${currentSourceCount} of ${sourceCount} sources current${currentSourceCount < sourceCount ? ' · partial current coverage' : ''}`,
    sources, findings, signals, unknownDimensions,
    forecast: record(value.replacement_forecast) ? value.replacement_forecast : null,
    forecastLabel: forecastLabel(value.replacement_forecast)
  };
}
