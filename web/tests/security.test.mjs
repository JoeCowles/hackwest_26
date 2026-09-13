import test from 'node:test';
import assert from 'node:assert/strict';
import {
  SecurityPages,
  SECURITY_COLLECTIONS,
  findingVM,
  securityPageVM,
  sourceVM
} from '../js/security.js';
import { SecurityView } from '../js/security-views.js';
import { parseRoute } from '../js/session.js';

const source = (extra = {}) => ({
  source_id: '11111111-1111-4111-8111-111111111111',
  node_id: '22222222-2222-4222-8222-222222222222',
  object_id: '33333333-3333-4333-8333-333333333333',
  resource_id: '44444444-4444-4444-8444-444444444444',
  direction: 'read',
  metric: 'storage.device.read_bytes_total',
  rule_id: 'storage.activity.high_rate.v1',
  policy_version: '1',
  baseline_revision: '1',
  active: true,
  support_state: 'supported',
  baseline: {
    state: 'learning', covered_seconds: 300, interval_count: 30,
    required_seconds: 600, required_intervals: 60,
    reference_rate_bytes_per_second: null,
    high_threshold_bytes_per_second: null,
    recovery_threshold_bytes_per_second: null,
    as_of: '2026-09-13T12:02:02Z'
  },
  episode: { state: 'quiet', finding_id: null, elevated_seconds: 0, recovery_seconds: 0 },
  observation: {
    state: 'unavailable', reason: 'no_observations', observed_at: null,
    received_at: null, age_seconds: null, stale_after_seconds: 15,
    rate_bytes_per_second: null
  },
  ...extra
});

const interval = (extra = {}) => ({
  collection_id: '55555555-5555-4555-8555-555555555555',
  previous_collection_id: '66666666-6666-4666-8666-666666666666',
  counter_start: '900719925474099312345',
  counter_end: '900719925514099312345',
  monotonic_start_ns: '100000000000',
  monotonic_end_ns: '105000000000',
  interval_seconds: 5,
  rate_bytes_per_second: 8000000,
  observed_at: '2026-09-13T12:00:05Z',
  received_at: '2026-09-13T12:00:06Z',
  continuity: {
    boot_id: 'boot-1', agent_generation: '7', agent_session_id: 'session-1',
    clock_id: 'clock-1', counter_epoch: 'epoch-1', source_version: '1',
    adapter_version: '1', policy_version: '1'
  },
  ...extra
});

const finding = (extra = {}) => ({
  finding_id: '77777777-7777-4777-8777-777777777777',
  source_id: source().source_id,
  node_id: source().node_id,
  object_id: source().object_id,
  resource_id: source().resource_id,
  direction: 'read',
  rule_id: 'storage.activity.high_rate.v1',
  status: 'open', severity: 'warning',
  summary: 'Sustained read activity exceeded this driver source baseline.',
  first_seen_at: '2026-09-13T12:00:05Z',
  last_seen_at: '2026-09-13T12:02:00Z',
  opened_at: '2026-09-13T12:02:00Z',
  updated_at: '2026-09-13T12:02:01Z',
  ended_at: null, reason: null, baseline_revision: '1',
  policy: { opening_seconds: 120, recovery_seconds: 60 },
  baseline: {
    reference_rate_bytes_per_second: 2000000,
    high_threshold_bytes_per_second: 6000000,
    recovery_threshold_bytes_per_second: 4000000,
    covered_seconds: 600, interval_count: 120
  },
  evidence: {
    first: interval(), latest: interval(), peak: interval({ rate_bytes_per_second: 12000000 }),
    elevated_seconds: 120, recovery_seconds: 0
  },
  ...extra
});

const page = (data, extraMeta = {}) => ({
  data,
  meta: { api_version: '1', server_time: '2026-09-13T12:02:02Z', next_cursor: null, ...extraMeta }
});

function renderedText(node) {
  const output = [];
  const visit = value => {
    if (Array.isArray(value)) return value.forEach(visit);
    if (value == null || value === false || value === true) return;
    if (typeof value === 'string' || typeof value === 'number') { output.push(String(value)); return; }
    if (typeof value !== 'object') return;
    if (typeof value.type === 'function') return visit(value.type(value.props || {}));
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

test('detector readiness keeps learning, ready, stale and unavailable states explicit', () => {
  const learning = sourceVM(source());
  assert.equal(learning.baselineState, 'learning');
  assert.match(learning.readinessLabel, /300.*600.*30.*60/);
  assert.equal(learning.observationState, 'unavailable');
  assert.match(learning.observationLabel, /no observations/i);

  const ready = sourceVM(source({
    baseline: {
      state: 'ready', covered_seconds: 900, interval_count: 180,
      required_seconds: 600, required_intervals: 60,
      reference_rate_bytes_per_second: 2000000,
      high_threshold_bytes_per_second: 6000000,
      recovery_threshold_bytes_per_second: 4000000
    },
    observation: {
      state: 'stale', reason: 'age_exceeded', observed_at: '2026-09-13T11:59:00Z',
      received_at: '2026-09-13T11:59:01Z', age_seconds: 62,
      stale_after_seconds: 15, rate_bytes_per_second: 2500000
    }
  }));
  assert.equal(ready.baselineState, 'ready');
  assert.match(ready.readinessLabel, /reference 2 MB\/s/i);
  assert.match(ready.thresholdLabel, /> 6 MB\/s.*≤ 4 MB\/s/i);
  assert.equal(ready.observationState, 'stale');
  assert.match(ready.observationLabel, /stale/i);
});

test('capacity-limited and inactive sources remain visible with their uncertainty', () => {
  const vm = sourceVM(source({
    active: false,
    support_state: 'capacity_limited',
    support_reason: 'active_source_limit_reached',
    observation: { ...source().observation, state: 'unavailable', reason: 'source_removed' }
  }));
  assert.equal(vm.active, false);
  assert.equal(vm.supportState, 'capacity_limited');
  assert.match(vm.supportLabel, /capacity limited.*active source limit reached/i);
  assert.match(vm.observationLabel, /source removed/i);
});

test('active admission is labeled supported rather than online when its owner is offline', () => {
  const offlineOwner = source({
    observation: {
      ...source().observation,
      state: 'stale', reason: 'owner_offline', observed_at: '2026-09-13T12:00:00Z',
      received_at: '2026-09-13T12:00:01Z', age_seconds: 122,
      rate_bytes_per_second: 2000000
    }
  });
  const tree = SecurityView({
    security: {
      sources: { rows: [offlineOwner], meta: page([], { policy: {}, coverage: {} }).meta, startedAt: 1000, startedWallAt: 100000 },
      findings: { rows: [], busy: false }, events: { rows: [], busy: false }
    },
    nodes: [{ id: offlineOwner.node_id, name: 'Offline owner' }],
    now: 1000, wallNow: 100000
  });
  const text = renderedText(tree);
  const tags = elements(tree, 'span').filter(node => String(node.props.class || '').includes('tag')).map(renderedText);
  assert.ok(tags.includes('supported'));
  assert.ok(!tags.includes('online'));
  assert.match(text, /Supported\s*·\s*Admitted/);
  assert.match(text, /owner offline/i);
  assert.doesNotMatch(text, /\bonline\b/i);
});

test('an active capacity-limited source is visible without claiming admission', () => {
  const limited = source({
    active: true,
    support_state: 'capacity_limited',
    support_reason: 'active_source_limit',
    observation: { ...source().observation, reason: 'capacity_limited' }
  });
  const tree = SecurityView({
    security: {
      sources: { rows: [limited], meta: page([], { policy: {}, coverage: {} }).meta },
      findings: { rows: [] }, events: { rows: [] }
    },
    nodes: [{ id: limited.node_id, name: 'Capacity-limited owner' }]
  });
  const supportCell = renderedText(elements(tree, 'td')[2]);
  assert.match(supportCell, /capacity limited.*active source limit/i);
  assert.match(supportCell, /Not admitted/i);
  assert.doesNotMatch(supportCell, /·\s*Admitted\b/i);
});

test('a frozen current source ages locally without mutating its snapshot evidence', () => {
  const current = source({
    observation: {
      state: 'current', reason: null, observed_at: '2026-09-13T12:02:00Z',
      received_at: '2026-09-13T12:02:01Z', age_seconds: 2,
      stale_after_seconds: 15, rate_bytes_per_second: 2000000
    }
  });
  const snapshot = { rows: [current], meta: page([]).meta, startedAt: 1000, startedWallAt: 100000 };
  const fresh = securityPageVM('sources', snapshot, 1000, 100000).rows[0];
  assert.equal(fresh.observationState, 'current');
  assert.match(fresh.observationLabel, /current.*at snapshot/i);
  const aged = securityPageVM('sources', snapshot, 15001, 114001).rows[0];
  assert.equal(aged.observationState, 'stale');
  assert.match(aged.observationLabel, /aged locally/i);
  assert.equal(current.observation.state, 'current');
  assert.equal(current.observation.age_seconds, 2);
});

test('a frozen current source ages across sleep when the monotonic clock pauses', () => {
  const current = source({
    observation: {
      state: 'current', reason: null, observed_at: '2026-09-13T12:02:00Z',
      received_at: '2026-09-13T12:02:01Z', age_seconds: 1,
      stale_after_seconds: 15, rate_bytes_per_second: 2000000
    }
  });
  const snapshot = {
    rows: [current], meta: page([]).meta,
    startedAt: 1000, startedWallAt: 100000
  };
  const aged = securityPageVM('sources', snapshot, 1000, 116000).rows[0];
  assert.equal(aged.observationState, 'stale');
  assert.match(aged.observationLabel, /aged locally/i);
  assert.equal(current.observation.state, 'current');
});

test('a slow first response consumes source freshness from request start', async () => {
  let now = 1000, wallNow = 100000;
  const current = source({
    observation: {
      state: 'current', reason: null, observed_at: '2026-09-13T12:02:00Z',
      received_at: '2026-09-13T12:02:01Z', age_seconds: 2,
      stale_after_seconds: 15, rate_bytes_per_second: 2000000
    }
  });
  const pages = new SecurityPages('viewer_test', () => {}, () => {}, () => ({
    close() {},
    async get(path) {
      if (path === '/api/v1/detectors/storage-activity/sources') {
        now = 15000; wallNow = 114000;
        return page([current], { snapshot_cursor: 'slow-source-snapshot' });
      }
      return page([], { snapshot_cursor: `${path}-snapshot` });
    }
  }), () => now, () => wallNow);
  await pages.snapshot().sources.refresh();
  const snapshot = pages.snapshot().sources;
  assert.equal(snapshot.startedAt, 1000);
  assert.equal(snapshot.startedWallAt, 100000);
  assert.equal(securityPageVM('sources', snapshot, now, wallNow).rows[0].observationState, 'stale');
  pages.close();
});

test('a failed source refresh retains dated rows but cannot retain current labels indefinitely', async () => {
  let now = 1000, wallNow = 100000, sourceReads = 0;
  const current = source({
    observation: {
      state: 'current', reason: null, observed_at: '2026-09-13T12:02:00Z',
      received_at: '2026-09-13T12:02:01Z', age_seconds: 1,
      stale_after_seconds: 15, rate_bytes_per_second: 2000000
    }
  });
  const pages = new SecurityPages('viewer_test', () => {}, () => {}, () => ({
    close() {},
    async get(path) {
      if (path === '/api/v1/detectors/storage-activity/sources') {
        if (sourceReads++) throw new Error('Detector refresh failed');
        return page([current], { snapshot_cursor: 'source-snapshot-1' });
      }
      return page([], { snapshot_cursor: `${path}-snapshot-1` });
    }
  }), () => now, () => wallNow);
  await pages.refreshAll();
  assert.equal(securityPageVM('sources', pages.snapshot().sources, now, wallNow).rows[0].observationState, 'current');
  now = 17000; wallNow = 116000;
  await pages.snapshot().sources.refresh();
  const retained = pages.snapshot().sources;
  assert.match(retained.error, /Detector refresh failed/);
  assert.equal(retained.rows.length, 1);
  assert.equal(securityPageVM('sources', retained, now, wallNow).rows[0].observationState, 'stale');
  pages.close();
});

test('baseline readiness is dated independently from observation freshness', () => {
  const vm = sourceVM(source(), { startedAt: 1000, startedWallAt: 100000, now: 1000, wallNow: 100000 });
  assert.match(vm.readinessAsOfLabel, /9\/13\/2026/);
  assert.equal(vm.observationState, 'unavailable');
});

test('missing numeric detector fields stay unknown instead of becoming zero activity', () => {
  const vm = sourceVM(source({
    baseline: {
      state: 'ready', covered_seconds: 600, interval_count: 120,
      required_seconds: 600, required_intervals: 60,
      reference_rate_bytes_per_second: null,
      high_threshold_bytes_per_second: null,
      recovery_threshold_bytes_per_second: null
    },
    observation: { ...source().observation, state: 'stale', rate_bytes_per_second: null }
  }));
  assert.match(vm.readinessLabel, /reference Unknown/);
  assert.match(vm.thresholdLabel, /> Unknown.*≤ Unknown/);
  assert.match(vm.observationLabel, /last rate Unknown/);
});

test('finding evidence uses the frozen baseline and preserves exact counter digits', () => {
  const vm = findingVM(finding());
  assert.equal(vm.valid, true);
  assert.equal(vm.status, 'open');
  assert.equal(vm.baselineLabel, 'Reference 2 MB/s · opens > 6 MB/s · recovers ≤ 4 MB/s');
  assert.equal(vm.evidence.first.counterStart, '900719925474099312345');
  assert.equal(vm.evidence.first.counterEnd, '900719925514099312345');
  assert.equal(vm.evidence.peak.rateLabel, '12 MB/s');
  assert.equal(vm.elevatedLabel, '2m 0s elevated');
});

test('malformed rows are disclosed without hiding valid dated rows', () => {
  const snapshot = { rows: [finding(), { finding_id: 'missing-contract-fields' }], meta: page([]).meta, error: '' };
  const vm = securityPageVM('findings', snapshot);
  assert.equal(vm.rows.length, 1);
  assert.equal(vm.invalidCount, 1);
  assert.match(vm.contractError, /1.*invalid/i);
  assert.equal(vm.rows[0].findingId, finding().finding_id);
});

test('findings, detector sources and collector events use independent frozen reads', async () => {
  const pending = new Map(), requested = [];
  const clientFactory = () => ({
    close() {},
    get(path, params) {
      requested.push([path, params]);
      return new Promise((resolve, reject) => pending.set(path, { resolve, reject }));
    }
  });
  const updates = [];
  const pages = new SecurityPages('viewer_test', state => updates.push(state), () => {}, clientFactory);
  const loading = pages.refreshAll();
  assert.deepEqual(requested.map(([path]) => path).sort(), [
    '/api/v1/detectors/storage-activity/sources', '/api/v1/events', '/api/v1/findings',
    '/api/v1/security/rule-findings', '/api/v1/security/rule-sources'
  ]);
  pending.get('/api/v1/findings').resolve(page([finding()]));
  await Promise.resolve(); await Promise.resolve();
  assert.equal(pages.snapshot().findings.rows.length, 1);
  assert.equal(pages.snapshot().sources.busy, true);
  assert.equal(pages.snapshot().events.busy, true);
  pending.get('/api/v1/security/rule-findings').resolve(page([]));
  pending.get('/api/v1/security/rule-sources').resolve(page([]));
  pending.get('/api/v1/detectors/storage-activity/sources').reject(new Error('Detector status unavailable'));
  pending.get('/api/v1/events').resolve(page([{ event_id: 'event-1', category: 'security' }]));
  await loading;
  assert.equal(pages.snapshot().findings.error, '');
  assert.match(pages.snapshot().sources.error, /Detector status unavailable/);
  assert.deepEqual(pages.snapshot().events.rows.map(row => row.event_id), ['event-1']);
  assert.ok(updates.length >= 3);
});

test('closing Security pages ignores every late response from the obsolete viewer session', async () => {
  const completions = [];
  const pages = new SecurityPages('viewer_old', () => {}, () => {}, () => ({
    close() {},
    get() { return new Promise(resolve => completions.push(resolve)); }
  }));
  const loading = pages.refreshAll();
  pages.close();
  completions.forEach(resolve => resolve(page([finding()])));
  await loading;
  assert.deepEqual(pages.snapshot().findings.rows, []);
  assert.deepEqual(pages.snapshot().sources.rows, []);
  assert.deepEqual(pages.snapshot().events.rows, []);
  assert.deepEqual(pages.snapshot().ruleFindings.rows, []);
  assert.deepEqual(pages.snapshot().ruleSources.rows, []);
});

test('the Security view distinguishes generated findings from collector events and stays read-only', () => {
  const security = {
    findings: { rows: [finding()], meta: page([]).meta, pageNumber: 1, rangeStart: 1, rangeEnd: 1 },
    sources: { rows: [source()], meta: page([], {
      policy: { version: '1', baseline_horizon_seconds: 1800, opening_seconds: 120 },
      coverage: { known_sources: 12, active_sources: 10, supported_sources: 9, capacity_limited_sources: 1, learning_sources: 4, ready_sources: 5, current_sources: 7, security_assessment: 'unknown', inventory_completeness: 'unknown' }
    }).meta, pageNumber: 1, rangeStart: 1, rangeEnd: 1 },
    events: { rows: [{ event_id: 'event-1', category: 'security', occurred_at: '2026-09-13T12:00:00Z', severity: 'warning', source: 'collector', node_id: source().node_id, summary: 'Collector reported an event.' }], meta: page([]).meta, pageNumber: 1, rangeStart: 1, rangeEnd: 1 }
  };
  const tree = SecurityView({ security, nodes: [{ id: source().node_id, name: 'Inference Mac 01' }] });
  const text = renderedText(tree);
  assert.match(text, /Generated storage-activity findings/);
  assert.match(text, /Detector source coverage/);
  assert.match(text, /Collector-reported security events/);
  assert.match(text, /baseline horizon seconds.*1,800/i);
  assert.match(text, /known sources.*12/i);
  assert.match(text, /security assessment.*unknown/i);
  assert.match(text, /does not establish.*safe|does not establish.*healthy/i);
  assert.doesNotMatch(text, /rebaseline|administrator credential/i);
  const buttons = elements(tree, 'button').map(node => renderedText(node));
  assert.ok(buttons.every(label => ['Previous', 'Next', 'Refresh snapshot'].includes(label)));
  const links = elements(tree, 'a').map(node => node.props.href);
  assert.ok(links.some(href => href === `#node/${encodeURIComponent(source().node_id)}`));
  const objectHref = `#node/${encodeURIComponent(source().node_id)}/object/${encodeURIComponent(source().object_id)}`;
  assert.ok(links.some(href => href === objectHref));
  assert.deepEqual(parseRoute(objectHref), { view: 'node', nodeId: source().node_id, objectId: source().object_id });
});

test('security collection contracts do not repurpose collector events as findings', () => {
  assert.deepEqual(SECURITY_COLLECTIONS, {
    findings: { path: '/api/v1/findings', params: { status: 'all' } },
    sources: { path: '/api/v1/detectors/storage-activity/sources', params: {} },
    ruleFindings: { path: '/api/v1/security/rule-findings', params: { status: 'all' } },
    ruleSources: { path: '/api/v1/security/rule-sources', params: {} },
    events: { path: '/api/v1/events', params: { category: 'security' } }
  });
});
