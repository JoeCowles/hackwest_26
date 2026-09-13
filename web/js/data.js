// Presentation constants only. Operational data always comes from the server.
export const OK = '#5980a6', WARN = '#a68059', CRIT = '#a65964', INK = '#1d1f20';
export const HEAD = "'Barlow Condensed',sans-serif";
export const MONO = 'ui-monospace,Menlo,monospace';
export const statusColor = state => state === 'healthy' || state === 'online' ? OK
  : state === 'degraded' || state === 'warning' ? WARN
  : state === 'offline' || state === 'critical' ? CRIT : '#888b90';
export const NAV = [['01','Overview','overview'],['02','Filesystem','fs'],['03','Throughput','io'],['04','Security monitoring','sec'],['05','Host detail','node'],['06','Operations','ops']];
export const TITLES = { overview: 'Cluster overview', fs: 'Filesystem observations', io: 'Throughput', sec: 'Security monitoring', node: 'Host detail', ops:'Storage operations' };
