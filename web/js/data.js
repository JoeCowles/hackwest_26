// Static cluster data for the Orchard console. Live jitter is applied in model.js.

export const OK = '#5980a6', WARN = '#a68059', CRIT = '#a65964', INK = '#1d1f20';
export const HEAD = "'Barlow Condensed',sans-serif";
export const MONO = 'ui-monospace,Menlo,monospace';

export const NODES = [
  { id: 'gala', name: 'gala', kind: 'imac', model: 'iMac 27" 2019 · i9', ip: '10.0.4.11', cap: 4000, used: 3180, read: 612, write: 388, temp: 68, cpu: 74, gpu: 31, mem: 62, net: 3.2, power: 142, uptime: '48d 06h', smart: 'PASSED', wear: 31, state: 'healthy', bus: 'thunderbolt 3 · nvme', drift: '−4%' },
  { id: 'fuji', name: 'fuji', kind: 'imac', model: 'iMac 27" 2020 · i7', ip: '10.0.4.12', cap: 4000, used: 2410, read: 588, write: 401, temp: 64, cpu: 51, gpu: 18, mem: 44, net: 2.1, power: 128, uptime: '48d 06h', smart: 'PASSED', wear: 22, state: 'healthy', bus: 'thunderbolt 3 · nvme', drift: '−1%' },
  { id: 'braeburn', name: 'braeburn', kind: 'imac', model: 'iMac 24" M1', ip: '10.0.4.13', cap: 2000, used: 1620, read: 1240, write: 910, temp: 52, cpu: 38, gpu: 22, mem: 71, net: 1.4, power: 41, uptime: '31d 19h', smart: 'PASSED', wear: 9, state: 'healthy', bus: 'internal nvme', drift: '+2%' },
  { id: 'pippin', name: 'pippin', kind: 'imac', model: 'iMac 27" 2017 · i5', ip: '10.0.4.14', cap: 3000, used: 2880, read: 210, write: 96, temp: 84, cpu: 91, gpu: 12, mem: 88, net: 4.8, power: 165, uptime: '12d 03h', smart: 'WARN', wear: 94, state: 'degraded', bus: 'usb 3.1 · sata ssd', drift: '−78%' },
  { id: 'cortland', name: 'cortland', kind: 'macbook', model: 'MacBook Pro 16" M2 Pro', ip: '10.0.4.21', cap: 2000, used: 940, read: 2810, write: 2140, temp: 47, cpu: 29, gpu: 9, mem: 33, net: 0.9, power: 38, uptime: '62d 11h', smart: 'PASSED', wear: 6, state: 'healthy', bus: 'internal nvme', drift: '+6%' },
  { id: 'macoun', name: 'macoun', kind: 'macbook', model: 'MacBook Pro 14" M1 Pro', ip: '10.0.4.22', cap: 1000, used: 610, read: 2450, write: 1980, temp: 44, cpu: 22, gpu: 6, mem: 28, net: 0.6, power: 31, uptime: '62d 11h', smart: 'PASSED', wear: 7, state: 'healthy', bus: 'internal nvme', drift: '0%' },
  { id: 'empire', name: 'empire', kind: 'macbook', model: 'MacBook Air M1', ip: '10.0.4.23', cap: 500, used: 470, read: 0, write: 0, temp: 39, cpu: 0, gpu: 0, mem: 9, net: 0, power: 6, uptime: '—', smart: 'UNKNOWN', wear: 18, state: 'offline', bus: 'wifi · internal nvme', drift: 'n/a' }
];

// x/z placement on the 3D shelf
export const POS = { gala: [-5.2, -1.7], fuji: [-1.75, -1.7], braeburn: [1.75, -1.7], pippin: [5.2, -1.7], cortland: [-3.5, 2.0], macoun: [0, 2.0], empire: [3.5, 2.0] };

export const statusColor = (st) => st === 'healthy' ? OK : st === 'degraded' ? WARN : CRIT;

export const TITLES = {
  overview: ['Cluster overview', 'shelf a · 7 hosts · orchardfs pool'],
  fs: ['Filesystem health', 'capacity, wear and growth per host'],
  io: ['Throughput', 'passive i/o sampling + nightly capability baseline'],
  sec: ['Access control', 'authentication attempts across sshd, smb and webdav'],
  alerts: ['Alerting', 'sms escalation, thresholds and delivery log'],
  node: ['Host detail', '']
};

export const NAV = [
  ['01', 'Overview', 'overview', null],
  ['02', 'Filesystem', 'fs', WARN],
  ['03', 'Throughput', 'io', null],
  ['04', 'Access', 'sec', CRIT],
  ['05', 'Alerting', 'alerts', null],
  ['06', 'Host detail', 'node', null]
];

export const COLS = {
  host: ['Host', 'Model', 'State', 'Capacity', 'Read', 'Write', 'Temp', 'CPU', 'Uptime'],
  vol: ['Mount', 'Type', 'Size', 'Used', 'Host', 'Note'],
  sec: ['Time', 'Source', 'Geo', 'Host', 'User', 'Method', 'Action'],
  disk: ['Device', 'Model', 'Size', 'Power-on', 'Wear', 'SMART'],
  log: ['Sent', 'Host', 'Message', 'To', 'Status']
};

export const OPEN_ALERTS = [
  { sev: 'CRIT', msg: 'pippin read throughput 78% below baseline', meta: 'rule 01 · fio comparator · paged D. Ramos 09:41', age: '4m' },
  { sev: 'CRIT', msg: 'empire unreachable — 24 missed heartbeats', meta: 'rule 02 · agent · paged D. Ramos 09:37', age: '8m' },
  { sev: 'WARN', msg: 'pippin volume orchard-04 at 96% capacity', meta: 'rule 04 · projected full in 9 days', age: '46m' },
  { sev: 'WARN', msg: 'gala repeated ssh failures from 45.148.10.62', meta: 'rule 06 · 31 attempts in 10 min · source blocked', age: '1h' }
];

export const SEC_EVENTS = [
  ['09:41:02', '45.148.10.62', 'Amsterdam, NL', 'gala', 'root', 'ssh key', 'BLOCKED'],
  ['09:40:55', '45.148.10.62', 'Amsterdam, NL', 'gala', 'admin', 'ssh pass', 'BLOCKED'],
  ['09:38:14', '10.0.4.2', 'LAN · studio', 'braeburn', 'h.lam', 'smb', 'ALLOWED'],
  ['09:31:40', '118.24.77.9', 'Chengdu, CN', 'fuji', 'ubuntu', 'ssh pass', 'BLOCKED'],
  ['09:28:03', '203.0.113.7', 'Unknown · VPN', 'pippin', 'orchard', 'webdav', 'THROTTLED'],
  ['09:19:22', '10.0.4.9', 'LAN · studio', 'cortland', 'd.ramos', 'ssh key', 'ALLOWED'],
  ['09:04:11', '92.63.197.14', 'Moscow, RU', 'gala', 'test', 'ssh pass', 'BLOCKED'],
  ['08:52:47', '10.0.4.2', 'LAN · studio', 'macoun', 'h.lam', 'smb', 'ALLOWED'],
  ['08:41:09', '45.148.10.62', 'Amsterdam, NL', 'fuji', 'root', 'ssh pass', 'BLOCKED'],
  ['08:22:55', '172.58.4.91', 'Oakland, US · LTE', 'pippin', 'd.ramos', 'webdav', 'ALLOWED'],
  ['07:58:31', '185.220.101.7', 'Tor exit', 'braeburn', 'git', 'ssh key', 'BLOCKED'],
  ['07:12:04', '10.0.4.14', 'LAN · studio', 'gala', 'orchard', 'internal', 'ALLOWED']
].map(([time, ip, geo, host, user, method, action]) => ({ time, ip, geo, host, user, method, action }));

export const SEC_KPIS = [
  { k: 'Attempts', v: '1 284', col: null },
  { k: 'Blocked', v: '1 197', col: CRIT },
  { k: 'New sources', v: '9', col: WARN }
];

export const SEC_STATS = [
  { k: 'Attempts · 24h', v: '1 284', note: '+18% vs 7d median', col: null },
  { k: 'Blocked', v: '1 197', note: 'fail2ban + geo rule', col: CRIT },
  { k: 'Allowed', v: '87', note: '6 distinct operators', col: OK },
  { k: 'New sources', v: '9', note: 'first seen in 24h', col: WARN },
  { k: 'MFA coverage', v: '5/7', note: 'pippin, empire pending', col: WARN }
];

export const BLOCKED = [
  { ip: '45.148.10.62', geo: 'Amsterdam, NL', n: 418, rule: 'ssh burst' },
  { ip: '118.24.77.9', geo: 'Chengdu, CN', n: 297, rule: 'geo deny' },
  { ip: '92.63.197.14', geo: 'Moscow, RU', n: 211, rule: 'geo deny' },
  { ip: '185.220.101.7', geo: 'Tor exit', n: 164, rule: 'exit list' },
  { ip: '203.0.113.7', geo: 'Unknown · VPN', n: 107, rule: 'rate limit' }
];

export const VOLUMES = [
  { path: '/Volumes/orchard-01', kind: 'APFS', size: '4.0 TB', used: '3.18 TB', host: 'gala', note: 'primary render cache' },
  { path: '/Volumes/orchard-02', kind: 'APFS', size: '4.0 TB', used: '2.41 TB', host: 'fuji', note: 'dailies + proxies' },
  { path: '/Volumes/orchard-03', kind: 'APFS', size: '2.0 TB', used: '1.62 TB', host: 'braeburn', note: 'active project' },
  { path: '/Volumes/orchard-04', kind: 'HFS+', size: '3.0 TB', used: '2.88 TB', host: 'pippin', note: 'archive · migrate off' },
  { path: '/Volumes/orchard-scratch', kind: 'APFS', size: '3.5 TB', used: '2.02 TB', host: 'cortland + macoun', note: 'striped scratch' }
];

export const SMS_PREVIEW = [
  { text: 'ORCHARD CRIT · pippin · read 210 MB/s (−78% vs baseline), disk wear 94%. Ack: orchard.local/a/8841', meta: 'delivered 09:41 · 102 chars' },
  { text: 'ORCHARD CRIT · empire · unreachable, 24 missed heartbeats since 09:33. Ack: orchard.local/a/8842', meta: 'delivered 09:37 · 96 chars' }
];
export const SMS_VARS = ['{severity}', '{host}', '{metric}', '{value}', '{baseline}', '{ack_url}'];

export const RULES = [
  { n: '01', cond: 'read or write < 40% of baseline', dur: '10 min', then: 'SMS primary', sev: 'CRIT' },
  { n: '02', cond: 'heartbeat missing', dur: '2 min', then: 'SMS primary + secondary', sev: 'CRIT' },
  { n: '03', cond: 'package temp > 85 °C', dur: '5 min', then: 'SMS primary', sev: 'CRIT' },
  { n: '04', cond: 'volume free < 5%', dur: '1 hour', then: 'SMS primary', sev: 'WARN' },
  { n: '05', cond: 'SMART wear > 90%', dur: 'immediate', then: 'digest 08:00', sev: 'WARN' },
  { n: '06', cond: 'failed auth > 25 / 10 min', dur: 'immediate', then: 'SMS primary + block source', sev: 'WARN' }
];

export const ROSTER = [
  { name: 'D. Ramos', tag: 'PRIMARY', phone: '+1 415 ••• 2207', shift: 'Mon–Thu 08:00', pct: 62 },
  { name: 'M. Osei', tag: 'SECONDARY', phone: '+1 415 ••• 9930', shift: 'Mon–Thu 08:00', pct: 62 },
  { name: 'H. Lam', tag: 'WEEKEND', phone: '+1 510 ••• 4412', shift: 'Fri–Sun 08:00', pct: 0 },
  { name: 'Studio ops', tag: 'FALLBACK', phone: '+1 415 ••• 0001', shift: 'always', pct: 100 }
];

export const THRESHOLDS = [
  { metric: 'Volume free space', warn: '< 10%', crit: '< 5%' },
  { metric: 'Read / write vs baseline', warn: '−15%', crit: '−40%' },
  { metric: 'Package temperature', warn: '78 °C', crit: '85 °C' },
  { metric: 'SMART wear level', warn: '80%', crit: '95%' },
  { metric: 'Memory pressure', warn: '75%', crit: '90%' },
  { metric: 'Failed auth / 10 min', warn: '10', crit: '25' }
];

export const MUTE_OPTS = ['30m', '1h', '4h', 'until ack'];

export const NOTIF_LOG = [
  ['09:41:06', 'pippin', 'CRIT read throughput −78% vs baseline', 'D. Ramos', 'DELIVERED'],
  ['09:37:44', 'empire', 'CRIT unreachable, 24 missed heartbeats', 'D. Ramos', 'DELIVERED'],
  ['09:42:44', 'empire', 'CRIT unreachable — escalated, no ack in 5m', 'M. Osei', 'DELIVERED'],
  ['08:55:12', 'pippin', 'WARN volume orchard-04 at 96%', 'D. Ramos', 'DELIVERED'],
  ['08:41:30', 'gala', 'WARN 31 failed ssh in 10 min · source blocked', 'D. Ramos', 'DELIVERED'],
  ['04:16:02', 'cluster', 'Nightly baseline report · 7 hosts', 'digest', 'SENT'],
  ['02:19:51', 'pippin', 'WARN package temp 81 °C for 5 min', 'D. Ramos', 'FAILED'],
  ['Fri 21:04', 'macoun', 'WARN SMART wear 80%', 'digest', 'SENT']
].map(([time, host, msg, to, st]) => ({ time, host, msg, to, st }));

export const JOBS = [
  { name: 'render · seq_0440_comp', owner: 'h.lam', pct: 72 },
  { name: 'proxy transcode · dailies', owner: 'batch', pct: 41 },
  { name: 'rsync → offsite', owner: 'root', pct: 88 },
  { name: 'fio baseline (queued)', owner: 'orchard', pct: 0 }
];

export const eventsFor = (n) => n.state === 'degraded'
  ? [{ time: '09:41', msg: 'Read throughput 210 MB/s — 78% below nightly baseline', src: 'fio comparator' },
     { time: '09:38', msg: 'SMART attribute 233 (wear leveling) crossed 90%', src: 'smartd' },
     { time: '09:12', msg: 'Package temperature 84 °C for 6 min', src: 'powermetrics' },
     { time: '08:55', msg: 'Volume /Volumes/orchard-04 at 96% capacity', src: 'agent' },
     { time: '04:15', msg: 'Nightly fio baseline completed in 34 s', src: 'scheduler' }]
  : [{ time: '09:40', msg: 'Heartbeat ok · 10s poll, 0 missed in 24h', src: 'agent' },
     { time: '06:02', msg: 'Job render · seq_0440_comp accepted', src: 'queue' },
     { time: '04:15', msg: 'Nightly fio baseline completed in 31 s', src: 'scheduler' },
     { time: '02:10', msg: 'SMB session opened from 10.0.4.2 (h.lam)', src: 'smbd' },
     { time: '00:00', msg: 'Snapshot orchardfs@daily-0912 taken', src: 'snapshotd' }];

export const disksFor = (n) => {
  const degraded = n.state === 'degraded';
  return n.kind === 'imac'
    ? [{ dev: 'disk0s2', model: 'APPLE SSD', size: '1.0 TB', hours: '11 204 h', wear: n.wear + '%', st: n.smart },
       { dev: 'disk2s1', model: 'Samsung T7', size: '2.0 TB', hours: '6 880 h', wear: '17%', st: 'PASSED' },
       { dev: 'disk3s1', model: 'OWC Envoy', size: '1.0 TB', hours: '9 312 h', wear: '44%', st: degraded ? 'WARN' : 'PASSED' }]
    : [{ dev: 'disk0s2', model: 'APPLE SSD AP', size: '1.0 TB', hours: '4 120 h', wear: n.wear + '%', st: n.smart },
       { dev: 'disk1s1', model: 'SanDisk Pro', size: '1.0 TB', hours: '2 904 h', wear: '11%', st: 'PASSED' }];
};
