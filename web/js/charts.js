// Deterministic pseudo-random series for sparklines and the throughput chart.

export function rnd(seed) { let s = seed; return () => (s = (s * 9301 + 49297) % 233280) / 233280; }

export function series(seed, n, base, amp, min) {
  const r = rnd(seed), a = []; let v = base;
  for (let i = 0; i < n; i++) { v += (r() - 0.5) * amp; v = Math.max(min, Math.min(base * 1.8, v)); a.push(v); }
  return a;
}

// polyline points for an SVG of w×h, values clipped at max
export function pts(arr, w, h, max) {
  const n = arr.length - 1;
  return arr.map((v, i) => `${(i / n * w).toFixed(1)},${(h - Math.min(1, v / max) * h).toFixed(1)}`).join(' ');
}
