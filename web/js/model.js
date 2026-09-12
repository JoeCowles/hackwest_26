import { OK, WARN, CRIT, statusColor } from './data.js';

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
export const rateLabel = m => numeric(m) == null ? 'Unknown' : `${display(numeric(m) / 1e6)} MB/s`;
export const timeLabel = text => text ? new Date(text).toLocaleString() : 'Not reported';
export const metricLabel = m => {
  const value = measured(m);
  if (value == null) return m?.state && m.state !== 'ok' ? m.state.replaceAll('_', ' ') : 'Unknown';
  return m.unit === 'bytes' ? bytes(value) : typeof value === 'number' ? `${display(value)} ${m.unit || ''}` : String(value);
};
export function nodeVM(node, stale = false) {
  const read = stale ? null : numeric(node.read_bytes_per_second);
  const write = stale ? null : numeric(node.write_bytes_per_second);
  const usedRatio = stale ? null : numeric(node.capacity?.used_ratio);
  const model = node.model || 'Model not reported';
  const state = stale ? 'unknown' : node.availability === 'offline' ? 'offline'
    : node.availability === 'unknown' ? 'unknown' : node.availability === 'degraded' || ['warning','critical'].includes(node.health?.overall) ? 'degraded' : 'healthy';
  return { ...node, id: node.node_id, state, col: statusColor(state), model,
    kind: /macbook/i.test(model) ? 'macbook' : 'imac',
    read, write, usedPct: usedRatio == null ? null : usedRatio * 100,
    readLabel: read == null ? 'Unknown' : `${display(read / 1e6)} MB/s`,
    writeLabel: write == null ? 'Unknown' : `${display(write / 1e6)} MB/s`,
    capacityLabel: stale ? 'Unknown' : `${bytes(measured(node.capacity?.used_bytes))} / ${bytes(measured(node.capacity?.capacity_bytes))}`,
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
    filesystems: snapshot?.filesystems || [], events: snapshot?.events || [], inventory: snapshot?.inventory || [],
    coverage: unavailable ? 'No current snapshot' : `${cluster.observed_node_count} / ${cluster.expected_node_count} hosts online; ${cluster.capacity.excluded_node_ids.length} excluded from local capacity`
  };
}

// Plot actual polls observed in this browser session. Missing samples split paths.
export function chartPath(history, key, width = 600, height = 160) {
  if (history.length < 2) return '';
  const max = Math.max(1, ...history.flatMap(p => [p.read, p.write]).filter(Number.isFinite));
  const first = history[0].at, duration = Math.max(1, history.at(-1).at - first);
  let drawing = false, previous = null;
  return history.map(point => {
    const value = point[key];
    if (!Number.isFinite(value)) { drawing = false; previous = point.at; return ''; }
    if (previous != null && point.at - previous > 15000) drawing = false;
    const x = (point.at - first) / duration * width;
    const y = height - value / max * (height - 8);
    const command = `${drawing ? 'L' : 'M'}${x.toFixed(1)},${y.toFixed(1)}`;
    drawing = true; previous = point.at; return command;
  }).join(' ');
}
