import test from 'node:test';
import assert from 'node:assert/strict';

const reliability = await import('../js/reliability.js').catch(() => ({
  ageObservation: () => undefined,
  reliabilityVM: () => undefined
}));
const views = await import('../js/reliability-views.js').catch(() => ({
  ReliabilityPanel: () => null
}));

const ids = {
  node: '22222222-2222-4222-8222-222222222222',
  disk: '33333333-3333-4333-8333-333333333333',
  smart: '44444444-4444-4444-8444-444444444444',
  iokit: '55555555-5555-4555-8555-555555555555',
  finding: '66666666-6666-4666-8666-666666666666'
};

const observation = (extra = {}) => ({
  state: 'current', reason: null, age_seconds: 2, stale_after_seconds: 15,
  observed_at: '2026-09-13T12:00:00Z', received_at: '2026-09-13T12:00:01Z',
  ...extra
});

const source = (extra = {}) => ({
  source_id: ids.iokit, node_id: ids.node, object_id: ids.disk,
  resource_id: 'disk0', collector: 'iokit.block_storage', scope: 'physical_device',
  device_identity_confidence: 'source_reported',
  observation: observation({ stale_after_seconds: 300 }),
  signals: [{
    rule_id: 'iokit.read_errors', dimension: 'media_health',
    state: 'warning', severity: 'critical', reason: 'new_read_errors',
    finding_id: ids.finding, observation: observation(),
    evidence: { counter_start: '184467440737095516160', counter_end: '184467440737095516161', delta: '1' }
  }, {
    rule_id: 'iokit.read_service_time', dimension: 'performance',
    state: 'warning', severity: 'warning', reason: null, finding_id: null,
    observation: observation({ stale_after_seconds: 15 }),
    evidence: {
      bytes_delta: '409600', operations_delta: '100',
      accounted_time_delta_ns: '400000000', interval_seconds: 10,
      counter_epoch: 'driver-instance-1', monotonic_start_ns: '100000000000',
      monotonic_end_ns: '110000000000', ns_per_operation: 4000000,
      reference_ns_per_operation: 1000000,
      baseline_covered_seconds: 600, baseline_interval_count: 60,
      required_baseline_seconds: 600, required_baseline_intervals: 60,
      opening_seconds_required: 120, recovery_seconds_required: 60,
      high_threshold_ns_per_operation: 3000000,
      recovery_threshold_ns_per_operation: 2000000,
      workload_bucket: { transfer_size_band: 0, operation_rate_power_of_two_band: 3 },
      elevated_seconds: 120, recovery_seconds: 0,
      label: 'driver-accounted service-time degradation',
      possible_causes: ['workload', 'caching', 'controller', 'media']
    }
  }],
  ...extra
});

const finding = (extra = {}) => ({
  finding_id: ids.finding, source_id: ids.iokit, node_id: ids.node,
  object_id: ids.disk, resource_id: 'disk0',
  rule_id: 'storage.iokit.read_errors.v1', dimension: 'media_health',
  classification: 'observed_error', severity: 'critical', status: 'open', current: true,
  policy_version: '1',
  summary: 'A new driver read error was observed.',
  first_seen_at: '2026-09-13T12:00:00Z', last_seen_at: '2026-09-13T12:00:00Z',
  opened_at: '2026-09-13T12:00:00Z', updated_at: '2026-09-13T12:00:01Z',
  ended_at: null, reason: null,
  evidence: { counter_start: '184467440737095516160', counter_end: '184467440737095516161', delta: '1' },
  ...extra
});

const fixture = (extra = {}) => ({
  assessment: 'critical', observation_state: 'current',
  source_count: 2, current_source_count: 2,
  sources: [
    source(),
    source({
      source_id: ids.smart, collector: 'smartctl',
      observation: observation({ stale_after_seconds: 15 }),
      signals: [{
        rule_id: 'smart.overall', dimension: 'media_health',
        state: 'clear', severity: 'info', reason: 'reported_passed', finding_id: null,
        observation: observation({ stale_after_seconds: 300 }), evidence: { passed: true }
      }],
      policy: { policy_version: '1', opening_seconds: 120 }
    })
  ],
  findings: [finding()],
  unknown_dimensions: ['performance'],
  replacement_forecast: { state: 'insufficient_data', estimated_failure_at: null },
  ...extra
});

function renderedText(node, { includePre = false } = {}) {
  const output = [];
  const visit = value => {
    if (Array.isArray(value)) return value.forEach(visit);
    if (value == null || typeof value === 'boolean') return;
    if (typeof value === 'string' || typeof value === 'number') { output.push(String(value)); return; }
    if (typeof value !== 'object') return;
    if (typeof value.type === 'function') return visit(value.type(value.props || {}));
    if (!includePre && value.type === 'pre') return;
    visit(value.props?.children);
  };
  visit(node);
  return output.join(' ');
}

function elements(node, type) {
  const result = [];
  const visit = value => {
    if (Array.isArray(value)) return value.forEach(visit);
    if (!value || typeof value !== 'object') return;
    if (typeof value.type === 'function') return visit(value.type(value.props || {}));
    if (value.type === type) result.push(value);
    visit(value.props?.children);
  };
  visit(node);
  return result;
}

test('source and signal currentness ages independently at their reported limits', () => {
  const value = fixture();
  const vm = reliability.reliabilityVM(value, { snapshotAgeMs: 14000, current: true });
  assert.equal(vm.sources[0].observationState, 'current');
  assert.equal(vm.sources[0].signals[0].observationState, 'stale');
  assert.equal(vm.sources[1].observationState, 'stale');
  assert.equal(vm.currentSourceCount, 1);
  assert.equal(vm.findings[0].current, false);
  assert.equal(value.sources[0].observation.state, 'current', 'aging must not alter retained evidence');
});

test('a retained or unmatched disk snapshot cannot keep current reliability claims', () => {
  const vm = reliability.reliabilityVM(fixture(), { snapshotAgeMs: 0, current: false });
  assert.equal(vm.observationState, 'stale');
  assert.ok(vm.sources.every(row => row.observationState === 'stale'));
  assert.ok(vm.findings.every(row => row.current === false));
  assert.equal(vm.assessmentLabel, 'Dated critical evidence');
});

test('partial coverage and no-current-warning wording do not claim blanket health', () => {
  const vm = reliability.reliabilityVM(fixture({
    assessment: 'no_current_warning', source_count: 3, current_source_count: 1,
    findings: [], unknown_dimensions: ['media_health', 'performance']
  }), { current: true });
  assert.match(vm.coverageLabel, /1 of 3 sources current/i);
  assert.equal(vm.assessmentLabel, 'No current warning in observed dimensions');
  assert.deepEqual(vm.unknownDimensions, ['media_health', 'performance']);
  assert.doesNotMatch(vm.assessmentLabel, /healthy/i);
});

test('missing reliability fields stay explicitly unavailable and never become zero', () => {
  const vm = reliability.reliabilityVM(undefined, { current: true });
  assert.equal(vm.available, false);
  assert.equal(vm.assessmentLabel, 'Reliability assessment unavailable');
  assert.equal(vm.coverageLabel, 'Source coverage unavailable');
  assert.equal(vm.forecastLabel, 'Replacement forecast unavailable');
  assert.equal(vm.sourceCount, null);
  assert.equal(vm.currentSourceCount, null);
  const partial = reliability.reliabilityVM({
    assessment: 'unknown', observation_state: 'unknown', source_count: 1,
    sources: [source()], findings: [], unknown_dimensions: [],
    replacement_forecast: { state: 'insufficient_data', estimated_failure_at: null }
  }, { current: true });
  assert.equal(partial.currentSourceCount, null);
  assert.equal(partial.coverageLabel, 'Source coverage unavailable');
});

test('panel renders reported conditions, readiness, source links and exact evidence', () => {
  const vm = reliability.reliabilityVM(fixture(), { snapshotAgeMs: 0, current: true });
  const performance = vm.signals.find(signal => signal.ruleId === 'iokit.read_service_time');
  assert.equal(performance.readinessState, 'ready');
  assert.match(performance.readinessLabel, /600.*600.*60.*60/);
  assert.match(performance.thresholdLabel, /3,000,000.*2,000,000.*ns\/op/i);
  assert.match(performance.workloadLabel, /transfer.*0.*operation rate.*3/i);
  const tree = views.ReliabilityPanel({ reliability: fixture(), snapshotAgeMs: 0, current: true });
  const text = renderedText(tree);
  assert.match(text, /Drive reliability evidence/);
  assert.match(text, /Reported conditions/);
  assert.match(text, /Deterioration and observed errors/);
  assert.match(text, /Service-time baseline readiness/);
  assert.match(text, /Ready.*600.*600.*60.*60/i);
  assert.match(text, /Identity confidence.*source reported/i);
  assert.match(text, /Replacement timing cannot be estimated from the available evidence/i);
  assert.match(text, /performance.*unknown/i);
  const links = elements(tree, 'a').map(element => element.props.href);
  assert.ok(links.includes(`#node/${ids.node}/object/${ids.disk}`));
  const findingCard = elements(tree, 'article').find(element => String(element.props.class).includes('reliability-finding'));
  assert.match(renderedText(findingCard), /Policy version.*1/i);
  assert.match(renderedText(findingCard), /Scope.*physical device/i);
  assert.match(renderedText(findingCard), /Identity confidence.*source reported/i);
  const evidence = renderedText(tree, { includePre: true });
  assert.match(evidence, /184467440737095516160/);
  assert.match(evidence, /184467440737095516161/);
});

test('panel preserves stale dated finding details without presenting them as current', () => {
  const tree = views.ReliabilityPanel({ reliability: fixture(), snapshotAgeMs: 16000, current: true });
  const text = renderedText(tree);
  assert.match(text, /Dated critical evidence/);
  assert.match(text, /A new driver read error was observed/);
  assert.match(text, /Dated evidence/);
  assert.doesNotMatch(text, /Current finding/);
});

test('signals with no reported reason use readable rule labels', () => {
  const signals = source().signals;
  const value = fixture({
    source_count: 1,
    current_source_count: 1,
    findings: [],
    sources: [source({ signals: [
      { ...signals[0], reason: null },
      { ...signals[1], reason: null },
      { ...signals[0], rule_id: 'smart.overall', reason: null }
    ] })]
  });
  const vm = reliability.reliabilityVM(value, { current: true });
  assert.deepEqual(vm.signals.map(signal => signal.title), [
    'Read errors', 'Read service time', 'SMART overall health'
  ]);
  const text = renderedText(views.ReliabilityPanel({ reliability: value, current: true }));
  assert.match(text, /Read errors/);
  assert.match(text, /Read service time/);
  assert.match(text, /SMART overall health/);
});
