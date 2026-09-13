import { OK, WARN, CRIT, statusColor } from './data.js';
import { rackFamily } from './rack.js';

export const measured = m => m?.state === 'ok' && m.value != null ? m.value : null;
export const numeric = m => { const value = measured(m); return value == null || !Number.isFinite(Number(value)) ? null : Number(value); };
export const display = (value, digits = 1) => value == null ? 'Unknown' : Number(value).toLocaleString(undefined, { maximumFractionDigits: digits });
export function bytes(value) {
  if (value == null) return 'Unknown';
  const amount = Number(value);
  if (!Number.isFinite(amount)) return 'Unknown';
  const units = ['B','KB','MB','GB','TB','PB'];
  const i = Math.max(0, Math.min(5, Math.floor(Math.log10(Math.max(1, amount)) / 3)));
  return `${display(amount / 1000 ** i, 2)} ${units[i]}`;
}
export function rateValueLabel(value) {
  if (!Number.isFinite(value) || value < 0) return 'Unknown';
  const units = ['B/s', 'KB/s', 'MB/s', 'GB/s', 'TB/s', 'PB/s'];
  const i = Math.max(0, Math.min(units.length - 1, Math.floor(Math.log10(Math.max(1, value)) / 3)));
  // Significant digits retain low nonzero activity, including fractional bytes/s.
  return `${(value / 1000 ** i).toLocaleString(undefined, { maximumSignificantDigits: 3 })} ${units[i]}`;
}
export const rateLabel = m => rateValueLabel(numeric(m));
export const timeLabel = text => text ? new Date(text).toLocaleString() : 'Not reported';
export const metricLabel = m => {
  const value = measured(m);
  if (value == null) return m?.state && m.state !== 'ok' ? m.state.replaceAll('_', ' ') : 'Unknown';
  if (m.kind === 'counter' && typeof value === 'string') return `${value} ${m.unit || ''}`.trim();
  if (m.kind === 'gauge' && typeof value === 'string' && /^-?\d+(?:\.\d+)?$/.test(value) && !['bytes', 'enum', 'string'].includes(m.unit)) return `${value} ${m.unit || ''}`.trim();
  return m.unit === 'bytes' ? bytes(value) : typeof value === 'number' ? `${display(value)} ${m.unit || ''}` : String(value);
};
export const filesystemMeasurement = measurement => ({
  label: bytes(measured(measurement)), state: measurement?.state || 'unknown',
  observedLabel: timeLabel(measurement?.observed_at)
});
export function inventoryLabel(object) {
  const properties = object?.properties || {};
  for (const key of ['mount_point', 'mount_path', 'volume_name', 'name', 'bsd_name', 'device_path', 'model', 'hostname']) {
    const value = properties[key];
    if (typeof value === 'string' && value.trim()) return value;
  }
  return object?.local_id || object?.object_id || 'Unnamed object';
}
export function inventoryProperties(object) {
  return Object.entries(object?.properties || {})
    .filter(([key, value]) => !key.startsWith('ciderd_') && ['string', 'number', 'boolean'].includes(typeof value))
    .map(([key, value]) => [key, String(value)]);
}
const partial = m => m?.coverage?.observed < m?.coverage?.expected;
export function coverageLabel(m, sources = 'sources') {
  const c = m?.coverage;
  if (!Number.isInteger(c?.observed) || !Number.isInteger(c?.expected)) return 'Coverage not reported';
  return `${partial(m) ? 'Partial: ' : ''}${c.observed}/${c.expected} ${sources}`;
}
function clusterRateCoverage(measurement, nodes, key) {
  // The cluster endpoint counts hosts; each node endpoint counts devices.
  // Requests are separate snapshots, so identify the provenance of both counts.
  const contributing = nodes.filter(node => node.availability === 'online' && numeric(node[key]) != null);
  let devices = 'Device coverage unavailable';
  if (contributing.length) {
    const coverages = contributing.map(node => node[key]?.coverage);
    if (coverages.every(c => Number.isInteger(c?.observed) && Number.isInteger(c?.expected))) {
      const coverage = coverages.reduce((total, c) => ({ observed: total.observed + c.observed, expected: total.expected + c.expected }), { observed: 0, expected: 0 });
      devices = coverageLabel({ coverage }, 'devices');
    } else devices = 'Device coverage not reported for all contributing hosts';
  }
  return `${coverageLabel(measurement, 'hosts')} contributing. Node poll: ${devices}.`;
}
const capacityValue = m => `${bytes(measured(m))}${partial(m) ? ' (partial)' : ''}`;
export function capacityCoverage(capacity) {
  return `Used: ${coverageLabel(capacity?.used_bytes)}; total: ${coverageLabel(capacity?.capacity_bytes)}. ${partial(capacity?.used_bytes) || partial(capacity?.capacity_bytes) ? 'Partial fields may cover different sources.' : ''}`.trim();
}
export function nodeVM(node, stale = false) {
  const read = stale ? null : numeric(node.read_bytes_per_second);
  const write = stale ? null : numeric(node.write_bytes_per_second);
  const usedRatio = stale ? null : numeric(node.capacity?.used_ratio);
  const model = node.hardware?.display_name || node.model || 'Model not reported';
  const state = stale ? 'unknown' : ['online', 'degraded', 'offline'].includes(node.availability) ? node.availability : 'unknown';
  return { ...node, id: node.node_id, state, col: statusColor(state), model,
    kind: rackFamily(node.hardware),
    read, write, usedPct: usedRatio == null ? null : usedRatio * 100,
    readLabel: rateValueLabel(read), writeLabel: rateValueLabel(write),
    readCoverage: stale ? 'No current coverage' : coverageLabel(node.read_bytes_per_second, 'devices'),
    writeCoverage: stale ? 'No current coverage' : coverageLabel(node.write_bytes_per_second, 'devices'),
    readObservedLabel: stale ? 'Not current' : timeLabel(node.read_bytes_per_second?.observed_at),
    writeObservedLabel: stale ? 'Not current' : timeLabel(node.write_bytes_per_second?.observed_at),
    capacityLabel: stale ? 'Unknown' : `${capacityValue(node.capacity?.used_bytes)} / ${capacityValue(node.capacity?.capacity_bytes)}`,
    capacityCoverage: stale ? 'No current coverage' : capacityCoverage(node.capacity),
    temperatureLabel: stale ? 'Unknown' : metricLabel(node.temperature_celsius),
    lastSeenLabel: timeLabel(node.last_seen_at),
    capBarStyle: { width: `${Math.min(100, Math.max(0, usedRatio == null ? 0 : usedRatio * 100))}%`, background: usedRatio >= .95 ? CRIT : usedRatio >= .9 ? WARN : OK }
  };
}
export function clusterVM(snapshot, stale = false) {
  const cluster = snapshot?.cluster;
  const nodes = (snapshot?.nodes || []).map(n => nodeVM(n, stale));
  const unavailable = !cluster || stale;
  const counts = cluster?.node_counts;
  return { nodes, cluster, stale: unavailable,
    healthyCount: unavailable ? '?' : counts.online,
    degradedCount: unavailable ? '?' : counts.degraded,
    offlineCount: unavailable ? '?' : counts.offline,
    unknownCount: unavailable ? '?' : counts.unknown,
    localFree: unavailable ? 'Unknown' : bytes(measured(cluster.capacity.local.free_bytes)),
    localTotal: unavailable ? 'Unknown' : bytes(measured(cluster.capacity.local.capacity_bytes)),
    sharedTotal: unavailable ? 'Unknown' : bytes(measured(cluster.capacity.shared.capacity_bytes)),
    totalRead: unavailable ? 'Unknown' : rateLabel(cluster.throughput.read_bytes_per_second),
    totalWrite: unavailable ? 'Unknown' : rateLabel(cluster.throughput.write_bytes_per_second),
    readCoverage: unavailable ? 'No current coverage' : clusterRateCoverage(cluster.throughput.read_bytes_per_second, snapshot.nodes || [], 'read_bytes_per_second'),
    writeCoverage: unavailable ? 'No current coverage' : clusterRateCoverage(cluster.throughput.write_bytes_per_second, snapshot.nodes || [], 'write_bytes_per_second'),
    localFreeCoverage: unavailable ? 'No current coverage' : coverageLabel(cluster.capacity.local.free_bytes),
    localTotalCoverage: unavailable ? 'No current coverage' : coverageLabel(cluster.capacity.local.capacity_bytes),
    sharedCoverage: unavailable ? 'No current coverage' : coverageLabel(cluster.capacity.shared.capacity_bytes),
    filesystems: snapshot?.filesystems || [], events: snapshot?.events || [], inventory: snapshot?.inventory || [],
    coverage: unavailable ? 'No current snapshot' : `${cluster.observed_node_count} / ${cluster.expected_node_count} hosts online; ${cluster.capacity.excluded_node_ids.length} excluded from local capacity`
  };
}

// Aggregate observed_at is the oldest contributing source date, not a sample ID.
// Retain polls honestly; timestamp-only deduplication can erase another source's update.
export function appendPollHistory(state, snapshot, at) {
  const point = (read, write) => ({
    at, read: numeric(read), write: numeric(write),
    readObservedAt: read?.observed_at || null, writeObservedAt: write?.observed_at || null
  });
  const append = (history, next) => [...(history || []), next].slice(-180);
  const nodes = new Map((snapshot?.nodes || []).map(node => [node.node_id, node]));
  const nodeHistory = {};
  const coverage = metric => [metric?.coverage?.observed ?? null, metric?.coverage?.expected ?? null];
  const contributor = (node, key) => node?.availability === 'online' && numeric(node[key]) != null
    ? [node.node_id, node.inventory_generation ?? null, ...coverage(node[key])] : null;
  const continuity = (next, previous, direction, identity) => {
    const key = `${direction}Continuity`;
    next[key] = snapshot ? JSON.stringify(identity) : previous?.[key];
    next[`${direction}BreakBefore`] = !!previous && next[key] !== previous[key];
  };
  for (const id of new Set([...Object.keys(state.nodeHistory || {}), ...nodes.keys()])) {
    const prior = state.nodeHistory?.[id] || [], node = nodes.get(id), previous = prior.at(-1);
    const next = node?.availability === 'online' ? point(node.read_bytes_per_second, node.write_bytes_per_second) : point(null, null);
    next.inventoryGeneration = node?.inventory_generation ?? previous?.inventoryGeneration;
    next.breakBefore = !!previous && next.inventoryGeneration !== previous.inventoryGeneration;
    for (const direction of ['read', 'write']) continuity(next, previous, direction, contributor(node, `${direction}_bytes_per_second`));
    nodeHistory[id] = append(prior, next);
  }
  const next = point(snapshot?.cluster?.throughput?.read_bytes_per_second, snapshot?.cluster?.throughput?.write_bytes_per_second);
  for (const direction of ['read', 'write']) {
    const key = `${direction}_bytes_per_second`;
    const contributors = [...nodes.values()].map(node => contributor(node, key)).filter(Boolean).sort((a, b) => a[0].localeCompare(b[0]));
    continuity(next, state.history?.at(-1), direction, [coverage(snapshot?.cluster?.throughput?.[key]), contributors]);
  }
  return { history: append(state.history, next), nodeHistory };
}

export function chartScale(history) {
  const max = Math.max(1, ...history.flatMap(p => [p.read, p.write]).filter(Number.isFinite));
  return { max, maxLabel: rateValueLabel(max), start: history[0]?.at ?? null, end: history.at(-1)?.at ?? null };
}

// Dots preserve single samples and isolated observations on either side of a gap.
export function chartPoints(history, key, width = 600, height = 160) {
  if (!history.length) return [];
  const { max } = chartScale(history);
  const first = history[0].at, duration = Math.max(1, history.at(-1).at - first);
  return history.filter(point => Number.isFinite(point[key])).map(point => ({
    x: (point.at - first) / duration * width,
    y: height - point[key] / max * (height - 8)
  }));
}

// Plot polls on one browser arrival clock. Missing values and known boundaries split paths.
export function chartPath(history, key, width = 600, height = 160) {
  if (history.length < 2) return '';
  const { max } = chartScale(history);
  const first = history[0].at, duration = Math.max(1, history.at(-1).at - first);
  let drawing = false, previous = null;
  return history.map(point => {
    const value = point[key];
    if (!Number.isFinite(value)) { drawing = false; previous = point.at; return ''; }
    if (point.breakBefore || point[`${key}BreakBefore`] || (previous != null && (point.at - previous > 15000 || point.at <= previous))) drawing = false;
    const x = (point.at - first) / duration * width;
    const y = height - value / max * (height - 8);
    const command = `${drawing ? 'L' : 'M'}${x.toFixed(1)},${y.toFixed(1)}`;
    drawing = true; previous = point.at; return command;
  }).join(' ');
}
