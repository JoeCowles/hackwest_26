// View models: turns raw node data + UI state into the values the views render.
import { NODES, OK, WARN, CRIT, INK, HEAD, MONO, statusColor, JOBS, eventsFor, disksFor } from './data.js';
import { rnd, series, pts } from './charts.js';

const bar = (pct, col) => ({ width: pct + '%', background: col || OK });
const big = (col) => ({ fontFamily: HEAD, fontWeight: 600, fontSize: '30px', lineHeight: 1, color: col || INK });

export const sevColor = (s) => s === 'CRIT' ? CRIT : s === 'WARN' ? WARN : OK;
export const actColor = (a) => a === 'BLOCKED' ? CRIT : a === 'ALLOWED' ? OK : WARN;
export const smartColor = (s) => s === 'PASSED' ? OK : s === 'WARN' ? WARN : 'rgba(29,31,32,.5)';
export const tempColor = (t) => t >= 80 ? CRIT : t >= 70 ? WARN : OK;

export function nodeVM(n, t, maxIo = 3000) {
  const jitter = (v, k) => v === 0 ? 0 : Math.max(1, Math.round(v * (1 + Math.sin(t * 0.9 + k) * 0.06)));
  const read = jitter(n.read, n.name.length), write = jitter(n.write, n.name.length + 2);
  const col = statusColor(n.state);
  const usedPct = Math.round(n.used / n.cap * 100);
  const tempPct = Math.min(100, Math.round(n.temp / 95 * 100));
  const tcol = tempColor(n.temp);
  return Object.assign({}, n, {
    read, write, usedPct, col,
    capLabel: `${(n.used / 1000).toFixed(2)}/${(n.cap / 1000).toFixed(1)} TB`,
    freeLabel: `${((n.cap - n.used) / 1000).toFixed(2)} TB`,
    fullIn: n.state === 'offline' ? '—' : usedPct > 90 ? '9 d' : usedPct > 70 ? '11 wk' : '7 mo',
    smartStyle: { fontFamily: HEAD, fontWeight: 600, fontSize: '15px', color: n.smart === 'PASSED' ? INK : n.smart === 'WARN' ? WARN : 'rgba(29,31,32,.5)' },
    tempTextStyle: { font: `500 12px/1 ${MONO}`, color: tcol },
    tempBarStyle: bar(tempPct, tcol),
    capBarStyle: bar(usedPct, usedPct > 90 ? CRIT : usedPct > 78 ? WARN : OK),
    poolStyle: { display: 'block', flex: n.cap, borderRight: '1px solid rgba(242,242,243,.9)', background: `linear-gradient(90deg, ${usedPct > 90 ? CRIT : OK} 0 ${usedPct}%, rgba(29,31,32,.1) ${usedPct}% 100%)` },
    readBarStyle: bar(Math.round(read / maxIo * 100), OK),
    writeBarStyle: bar(Math.round(write / maxIo * 100), '#b7b7ba'),
    driftStyle: { fontFamily: HEAD, fontWeight: 600, fontSize: '17px', color: n.drift.startsWith('−7') ? CRIT : n.drift === 'n/a' ? 'rgba(29,31,32,.45)' : INK },
    sparkPts: pts(series(n.name.length * 37 + 11, 28, n.read || 40, (n.read || 40) * 0.3, 0), 120, 30, (n.read || 40) * 1.6)
  });
}

export function selVM(n) {
  const tiles = [
    { k: 'CPU', v: n.cpu, u: '%', style: big(n.cpu > 85 ? CRIT : null), barStyle: bar(n.cpu, n.cpu > 85 ? CRIT : OK) },
    { k: 'GPU', v: n.gpu, u: '%', style: big(), barStyle: bar(n.gpu) },
    { k: 'MEM PRESSURE', v: n.mem, u: '%', style: big(n.mem > 85 ? CRIT : n.mem > 70 ? WARN : null), barStyle: bar(n.mem, n.mem > 85 ? CRIT : n.mem > 70 ? WARN : OK) },
    { k: 'TEMP', v: n.temp, u: '°C', style: big(n.temp >= 80 ? CRIT : n.temp >= 70 ? WARN : null), barStyle: bar(Math.round(n.temp / 95 * 100), tempColor(n.temp)) },
    { k: 'READ', v: n.read, u: 'MB/s', style: big(), barStyle: bar(Math.min(100, Math.round(n.read / 30))) },
    { k: 'WRITE', v: n.write, u: 'MB/s', style: big(), barStyle: bar(Math.min(100, Math.round(n.write / 30))) },
    { k: 'POWER', v: n.power, u: 'W', style: big(), barStyle: bar(Math.round(n.power / 200 * 100)) },
    { k: 'CAPACITY', v: n.usedPct, u: '% used', style: big(n.usedPct > 90 ? CRIT : null), barStyle: bar(n.usedPct, n.usedPct > 90 ? CRIT : OK) }
  ];
  const disks = disksFor(n).map(d => Object.assign(d, { stStyle: { font: `600 10px/1 ${MONO}`, letterSpacing: '.08em', color: smartColor(d.st) } }));
  const jobs = JOBS.map(j => Object.assign({}, j, { barStyle: bar(j.pct, j.pct ? OK : 'rgba(29,31,32,.2)') }));
  return Object.assign({}, n, {
    tiles, disks, jobs, events: eventsFor(n),
    readPts: pts(series(n.name.length * 13 + 3, 60, n.read || 40, (n.read || 40) * 0.35, 0), 300, 106, (n.read || 40) * 1.7),
    writePts: pts(series(n.name.length * 29 + 7, 60, n.write || 25, (n.write || 25) * 0.35, 0), 300, 106, (n.read || 40) * 1.7)
  });
}

export function heatRows() {
  return ['MON', 'TUE', 'WED', 'THU', 'FRI', 'SAT', 'SUN'].map((day, di) => {
    const r = rnd(di * 91 + 17);
    return {
      day,
      cells: Array.from({ length: 24 }, (_, hi) => {
        let a = r();
        if (hi >= 1 && hi <= 5) a = Math.min(1, a + 0.45);
        if (di >= 5) a = Math.min(1, a + 0.2);
        const n = Math.round(a * 64);
        return {
          title: `${day} ${String(hi).padStart(2, '0')}:00 · ${n} attempts`,
          style: { flex: 1, height: '13px', display: 'block', background: a < 0.12 ? 'rgba(29,31,32,.07)' : a > 0.8 ? CRIT : `rgba(89,128,166,${(0.18 + a * 0.8).toFixed(2)})` }
        };
      })
    };
  });
}

export function clusterVM(t) {
  const nodes = NODES.map(n => nodeVM(n, t));
  const cap = NODES.reduce((a, n) => a + n.cap, 0), used = NODES.reduce((a, n) => a + n.used, 0);
  const totalRead = nodes.reduce((a, n) => a + n.read, 0), totalWrite = nodes.reduce((a, n) => a + n.write, 0);
  return {
    nodes, cap, used,
    healthyCount: NODES.filter(n => n.state === 'healthy').length,
    degradedCount: NODES.filter(n => n.state === 'degraded').length,
    offlineCount: NODES.filter(n => n.state === 'offline').length,
    capTB: (cap / 1000).toFixed(1), freeTB: ((cap - used) / 1000).toFixed(2), usedPct: Math.round(used / cap * 100),
    totalRead: totalRead.toLocaleString(), totalWrite: totalWrite.toLocaleString(),
    aggReadPts: pts(series(7, 60, 7900, 1800, 500), 280, 52, 13000),
    aggWritePts: pts(series(23, 60, 5900, 1500, 400), 280, 52, 13000),
    bigReadPts: pts(series(7, 120, 7900, 1700, 500), 600, 186, 13000),
    bigWritePts: pts(series(23, 120, 5900, 1400, 400), 600, 186, 13000),
    fsKpis: [
      { k: 'Pool free', v: ((cap - used) / 1000).toFixed(2), u: 'TB', note: `of ${(cap / 1000).toFixed(1)} TB across 7 hosts` },
      { k: 'Pool used', v: Math.round(used / cap * 100), u: '%', note: '+412 GB in the last 24h' },
      { k: 'Hosts over 90%', v: '2', u: 'hosts', note: 'pippin 96% · empire 94%' },
      { k: 'Projected full', v: '11', u: 'weeks', note: 'at current growth, cluster-wide' }
    ]
  };
}
