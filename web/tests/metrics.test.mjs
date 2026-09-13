import test from 'node:test';
import assert from 'node:assert/strict';
import * as model from '../js/model.js';

const measurement = (value, unit = 'bytes/second', extra = {}) => ({
  state: 'ok', value, unit, kind: 'gauge', source: 'server:deduplicated_sum',
  scope: 'aggregate', observed_at: '2026-09-12T12:00:00Z', received_at: '2026-09-12T12:00:01Z',
  age_seconds: 1, boot_id: null, inventory_generation: null,
  coverage: { observed: 1, expected: 1 }, ...extra
});
const unavailable = () => measurement(null, 'bytes/second', { state: 'unknown' });
const capacity = () => ({
  used_bytes: measurement('500', 'bytes'), capacity_bytes: measurement('1000', 'bytes'),
  free_bytes: measurement('500', 'bytes'), available_bytes: measurement('500', 'bytes'),
  used_ratio: measurement(0.5, 'ratio')
});
const node = (extra = {}) => ({
  node_id: 'one', name: 'One', model: 'MacBook Pro', availability: 'online',
  inventory_generation: '1', last_seen_at: '2026-09-12T12:00:01Z',
  health: { overall: 'unknown', unknown_dimensions: ['smart', 'capacity'] },
  capacity: capacity(), read_bytes_per_second: measurement(1000),
  write_bytes_per_second: measurement(0), temperature_celsius: unavailable(), ...extra
});
const snapshot = (nodes = [node()]) => ({ nodes, cluster: {
  node_counts: { online: nodes.length, degraded: 0, offline: 0, unknown: 0 },
  observed_node_count: nodes.length, expected_node_count: nodes.length,
  capacity: { local: capacity(), shared: capacity(), excluded_node_ids: [] },
  throughput: { read_bytes_per_second: nodes.length ? measurement(nodes.length * 1000) : unavailable(), write_bytes_per_second: nodes.length ? measurement(0) : unavailable() }
} });

test('shared capacity lists every NFS mount without inventing an additive total', () => {
  const s = snapshot();
  s.cluster.capacity.shared.capacity_bytes = unavailable();
  s.cluster.capacity.shared_mounts = ['one', 'two'].map(id => ({
    object_id: id, node_id: 'one', source: `nas:/exports/${id}`,
    mount_point: `/Volumes/${id}`, capacity: capacity(), included_in_shared_total: false
  }));
  const vm = model.clusterVM(s);
  assert.equal(vm.sharedMounts.length, 2);
  assert.equal(vm.sharedMounts[1].source, 'nas:/exports/two');
  assert.equal(vm.sharedMounts[0].hostLabel, 'One');
  assert.notEqual(vm.sharedMounts[0].freeLabel, 'Unknown');
  assert.equal(vm.sharedTotal, 'Unknown');
  assert.equal(model.clusterVM(s, true).sharedMounts[0].freeLabel, 'Unknown');
  s.cluster.capacity.shared_mounts[1].capacity.free_bytes = unavailable();
  assert.equal(model.clusterVM(s).sharedMounts[1].freeLabel, 'Unknown');
});

test('positive sub-MB traffic stays distinguishable from an idle source', () => {
  for (const value of [0.000001, 1, 1000, 49999]) {
    const label = model.rateLabel(measurement(value));
    assert.doesNotMatch(label, /^0(?:[.,]0+)? /, `${value} B/s must not display as zero`);
  }
  assert.equal(model.rateLabel(measurement(1000000)), '1 MB/s');
  assert.equal(model.rateLabel(measurement(2000000000)), '2 GB/s');
  assert.equal(model.rateLabel(unavailable()), 'Unknown');
  assert.equal(model.rateLabel(measurement(-1)), 'Unknown');
});

test('raw integer counters retain exact values and their units', () => {
  for (const unit of ['operations', 'nanoseconds', 'bytes']) {
    assert.equal(model.metricLabel(measurement('9007199254740993', unit, { kind: 'counter' })), `9007199254740993 ${unit}`);
  }
});

test('availability does not upgrade unknown storage health to healthy', () => {
  assert.equal(model.nodeVM(node()).state, 'online');
  assert.equal(model.nodeVM(node({ health: { overall: 'critical', unknown_dimensions: [] } })).state, 'online');
  assert.equal(model.nodeVM(node(), true).state, 'unknown');
});

test('partial rate sums disclose incomplete source coverage', () => {
  const data = snapshot();
  data.cluster.throughput.read_bytes_per_second = measurement(1000000, 'bytes/second', { coverage: { observed: 1, expected: 4 } });
  const vm = model.clusterVM(data);
  assert.match(vm.readCoverage, /partial/i);
  assert.match(vm.readCoverage, /1\s*\/\s*4/);
  assert.equal(vm.totalRead, '1 MB/s');
});

test('capacity with unequal coverage is not presented as a complete used-to-total fraction', () => {
  const data = node();
  data.capacity.used_bytes.coverage = { observed: 1, expected: 2 };
  data.capacity.capacity_bytes.coverage = { observed: 2, expected: 2 };
  data.capacity.used_ratio = unavailable();
  const vm = model.nodeVM(data);
  assert.match(vm.capacityLabel, /partial/i);
  assert.match(vm.capacityCoverage, /1\s*\/\s*2/);
  assert.match(vm.capacityCoverage, /2\s*\/\s*2/);
  assert.equal(vm.usedPct, null);
});

test('chart scale supplies a physical rate and arrival-time bounds', () => {
  assert.equal(typeof model.chartScale, 'function');
  const scale = model.chartScale([{ at: 1000, read: 1000000, write: 0 }, { at: 6000, read: 2000000, write: 0 }]);
  assert.equal(scale.max, 2000000);
  assert.equal(scale.maxLabel, '2 MB/s');
  assert.equal(scale.start, 1000);
  assert.equal(scale.end, 6000);
});

test('failure inserts a gap into both aggregate and every retained host history', () => {
  assert.equal(typeof model.appendPollHistory, 'function');
  const before = model.appendPollHistory({ history: [], nodeHistory: {} }, snapshot(), 1000);
  const failed = model.appendPollHistory(before, null, 6000);
  const after = model.appendPollHistory(failed, snapshot(), 11000);
  for (const history of [after.history, after.nodeHistory.one]) {
    assert.deepEqual(history.map(p => p.at), [1000, 6000, 11000]);
    assert.equal(history[1].read, null);
    assert.doesNotMatch(model.chartPath(history, 'read'), /L/);
  }
});

test('poll history retains source dates without inventing a unique aggregate sample identity', () => {
  assert.equal(typeof model.appendPollHistory, 'function');
  const first = model.appendPollHistory({ history: [], nodeHistory: {} }, snapshot(), 1000);
  const next = model.appendPollHistory(first, snapshot(), 6000);
  assert.equal(next.history.length, 2);
  assert.equal(next.history[1].at, 6000);
  assert.equal(next.history[1].readObservedAt, '2026-09-12T12:00:00Z');
  assert.equal(next.nodeHistory.one[1].readObservedAt, '2026-09-12T12:00:00Z');
});

test('missing hosts and changed inventory generations break host paths', () => {
  assert.equal(typeof model.appendPollHistory, 'function');
  const first = model.appendPollHistory({ history: [], nodeHistory: {} }, snapshot(), 1000);
  const gone = model.appendPollHistory(first, snapshot([]), 6000);
  assert.equal(gone.nodeHistory.one.at(-1).read, null);
  const reset = model.appendPollHistory(first, snapshot([node({ inventory_generation: '2' })]), 6000);
  assert.doesNotMatch(model.chartPath(reset.nodeHistory.one, 'read'), /L/);
  assert.doesNotMatch(model.chartPath(reset.history, 'read'), /L/);
});

test('stale or reset-derived rates split each affected direction without fabricating zero', () => {
  assert.equal(typeof model.appendPollHistory, 'function');
  const first = model.appendPollHistory({ history: [], nodeHistory: {} }, snapshot(), 1000);
  const stale = snapshot([node({ read_bytes_per_second: measurement(1000, 'bytes/second', { state: 'stale' }) })]);
  stale.cluster.throughput.read_bytes_per_second = unavailable();
  const next = model.appendPollHistory(first, stale, 6000);
  assert.equal(next.nodeHistory.one[1].read, null);
  assert.equal(next.nodeHistory.one[1].write, 0);
  assert.equal(next.history[1].read, null);
});

test('a removed host does not keep breaking every subsequent cluster poll', () => {
  const other = node({ node_id: 'two' });
  const first = model.appendPollHistory({ history: [], nodeHistory: {} }, snapshot([node(), other]), 1000);
  const removed = model.appendPollHistory(first, snapshot([other]), 6000);
  const later = model.appendPollHistory(removed, snapshot([other]), 11000);
  assert.match(model.chartPath(later.history, 'read'), /L/);
});

test('cluster coverage distinguishes contributing hosts from partial device coverage', () => {
  const data = snapshot([node({
    read_bytes_per_second: measurement(1000, 'bytes/second', { coverage: { observed: 1, expected: 4 } })
  })]);
  const vm = model.clusterVM(data);
  assert.match(vm.readCoverage, /1\s*\/\s*1 hosts/i);
  assert.match(vm.readCoverage, /partial.*1\s*\/\s*4 devices/i);
  assert.match(vm.writeCoverage, /1\s*\/\s*1 devices/i);
  assert.doesNotMatch(vm.writeCoverage, /partial/i);
});

test('missing device coverage is reported instead of inferred from a host count', () => {
  const data = snapshot([node({ read_bytes_per_second: measurement(1000, 'bytes/second', { coverage: undefined }) })]);
  const vm = model.clusterVM(data);
  assert.match(vm.readCoverage, /1\s*\/\s*1 hosts/i);
  assert.match(vm.readCoverage, /device coverage.*not reported/i);
  assert.doesNotMatch(vm.readCoverage, /1\s*\/\s*1 devices/i);
});

test('a newly contributing host breaks the cluster path once', () => {
  const first = model.appendPollHistory({ history: [], nodeHistory: {} }, snapshot(), 1000);
  const joined = snapshot([node(), node({ node_id: 'two' })]);
  const next = model.appendPollHistory(first, joined, 6000);
  for (const key of ['read', 'write']) assert.doesNotMatch(model.chartPath(next.history, key), /L/);
  const later = model.appendPollHistory(next, joined, 11000);
  assert.match(model.chartPath(later.history, 'read'), /L/);
});

test('loss of one host read source breaks only the affected cluster direction', () => {
  const first = model.appendPollHistory({ history: [], nodeHistory: {} }, snapshot([node(), node({ node_id: 'two' })]), 1000);
  const data = snapshot([node(), node({ node_id: 'two', read_bytes_per_second: unavailable() })]);
  data.cluster.throughput.read_bytes_per_second = measurement(1000, 'bytes/second', { coverage: { observed: 1, expected: 2 } });
  const next = model.appendPollHistory(first, data, 6000);
  assert.doesNotMatch(model.chartPath(next.history, 'read'), /L/);
  assert.match(model.chartPath(next.history, 'write'), /L/);
  const recovered = model.appendPollHistory(next, snapshot([node(), node({ node_id: 'two' })]), 11000);
  assert.doesNotMatch(model.chartPath(recovered.history, 'read'), /L/);
});

test('device coverage changes break only the affected host and cluster direction', () => {
  const firstData = snapshot([node({ read_bytes_per_second: measurement(2000, 'bytes/second', { coverage: { observed: 2, expected: 2 } }) })]);
  const first = model.appendPollHistory({ history: [], nodeHistory: {} }, firstData, 1000);
  const partialData = snapshot([node({ read_bytes_per_second: measurement(1000, 'bytes/second', { coverage: { observed: 1, expected: 2 } }) })]);
  const next = model.appendPollHistory(first, partialData, 6000);
  for (const history of [next.history, next.nodeHistory.one]) {
    assert.doesNotMatch(model.chartPath(history, 'read'), /L/);
    assert.match(model.chartPath(history, 'write'), /L/);
  }
  const later = model.appendPollHistory(next, partialData, 11000);
  assert.match(model.chartPath(later.nodeHistory.one, 'read'), /L/);
});

test('host availability changes break partial cluster rates', () => {
  const first = model.appendPollHistory({ history: [], nodeHistory: {} }, snapshot([node(), node({ node_id: 'two' })]), 1000);
  const data = snapshot([node(), node({ node_id: 'two', availability: 'degraded' })]);
  const next = model.appendPollHistory(first, data, 6000);
  for (const key of ['read', 'write']) {
    assert.equal(next.nodeHistory.two.at(-1)[key], null);
    assert.doesNotMatch(model.chartPath(next.history, key), /L/);
  }
});

test('single and isolated chart samples have visible points without inventing missing values', () => {
  assert.equal(typeof model.chartPoints, 'function');
  assert.deepEqual(model.chartPoints([{ at: 1000, read: 1000, write: null }], 'read'), [{ x: 0, y: 8 }]);
  assert.deepEqual(model.chartPoints([{ at: 1000, read: null, write: 0 }], 'write'), [{ x: 0, y: 160 }]);
  assert.deepEqual(model.chartPoints([{ at: 1000, read: null, write: null }], 'read'), []);
  const history = [
    { at: 1000, read: 1000, write: null },
    { at: 6000, read: null, write: null },
    { at: 11000, read: 1000, write: null, readBreakBefore: true }
  ];
  assert.deepEqual(model.chartPoints(history, 'read'), [{ x: 0, y: 8 }, { x: 600, y: 8 }]);
  assert.doesNotMatch(model.chartPath(history, 'read'), /L/);
});

test('inventory labels expose reported mount and device names while preserving stable identity fallback', () => {
  assert.equal(typeof model.inventoryLabel, 'function');
  assert.equal(model.inventoryLabel({ object_id: 'stable-id', local_id: 'ciderd:opaque', properties: { mount_point: '/Volumes/Research', bsd_name: 'disk4s1' } }), '/Volumes/Research');
  assert.equal(model.inventoryLabel({ object_id: 'stable-id', local_id: 'ciderd:opaque', properties: { bsd_name: 'disk0' } }), 'disk0');
  assert.equal(model.inventoryLabel({ object_id: 'stable-id', local_id: 'ciderd:opaque', properties: { volume_name: 'Data', bsd_name: 'disk3s1' } }), 'Data');
  assert.equal(model.inventoryLabel({ object_id: 'stable-id', local_id: 'ciderd:opaque', properties: { name: {} } }), 'ciderd:opaque');
});

test('inventory properties retain scalar observations including false and zero', () => {
  assert.equal(typeof model.inventoryProperties, 'function');
  assert.deepEqual(model.inventoryProperties({ properties: { bsd_name: 'disk0', mounted: false, revision: 0, missing: null, ciderd_relationships: [], nested: {} } }), [
    ['bsd_name', 'disk0'], ['mounted', 'false'], ['revision', '0']
  ]);
});

test('native integer gauges retain exact digits and units while enum labels stay natural', () => {
  assert.equal(model.metricLabel(measurement('17', 'percent')), '17 percent');
  assert.equal(model.metricLabel(measurement('9007199254740993', 'count')), '9007199254740993 count');
  assert.equal(model.metricLabel(measurement('4', 'bitfield')), '4 bitfield');
  assert.equal(model.metricLabel(measurement('mounted', 'enum')), 'mounted');
  assert.equal(model.metricLabel(measurement('1', 'enum')), '1');
});

test('filesystem fields expose their own observation states and source dates', () => {
  assert.equal(typeof model.filesystemMeasurement, 'function');
  const used = model.filesystemMeasurement(measurement('1000', 'bytes'));
  const free = model.filesystemMeasurement(measurement(null, 'bytes', { state: 'unknown', observed_at: null }));
  assert.equal(used.label, '1 KB');
  assert.equal(used.state, 'ok');
  assert.notEqual(used.observedLabel, 'Not reported');
  assert.equal(free.label, 'Unknown');
  assert.equal(free.state, 'unknown');
  assert.equal(free.observedLabel, 'Not reported');
});
