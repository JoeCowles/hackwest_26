#!/usr/bin/env node
// Synthetic API client only. This does not collect data from this computer.
import { readFile } from 'node:fs/promises';
import { homedir } from 'node:os';
import { randomUUID } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';

const base = process.env.ORCHARD_URL || 'http://127.0.0.1:8787';
const credentialPath = process.env.ORCHARD_ADMIN_TOKEN_FILE || `${homedir()}/Library/Application Support/Orchard Server/admin-token`;
const admin = (await readFile(credentialPath, 'utf8')).trim();
const count = Number(process.env.ORCHARD_DEMO_BATCHES || 12);
if (!Number.isInteger(count) || count < 2 || count > 17280) throw new Error('ORCHARD_DEMO_BATCHES must be 2-17280');

async function send(method, path, credential, body) {
  const headers = { 'Content-Type': 'application/json', 'X-Request-ID': randomUUID(), 'X-Request-Timestamp': new Date().toISOString() };
  if (credential) headers.Authorization = `Bearer ${credential}`;
  const response = await fetch(`${base}${path}`, { method, headers, body: JSON.stringify(body), signal: AbortSignal.timeout(10000) });
  const value = await response.json();
  if (!response.ok) throw new Error(`${method} ${path}: ${response.status} ${value.error?.code}`);
  return value.data;
}
const enrollment = await send('POST', '/api/v1/enrollment-tokens', admin, {});
const agent = { version: '0.1.0-demo', os_build: 'synthetic' };
const node = await send('POST', '/api/v1/nodes/enroll', null, { enrollment_token: enrollment.enrollment_token, name: 'Orchard demo node', agent });
const root = `/api/v1/nodes/${node.node_id}`;
const boot = randomUUID();
const inventory = await send('PUT', `${root}/inventory`, node.credential, { generation: 1, objects: [
  { local_id: 'demo-device', kind: 'device', properties: { model: 'Synthetic NVMe' } },
  { local_id: 'demo-container', kind: 'apfs_container', parents: ['demo-device'] },
  { local_id: 'demo-volume', kind: 'apfs_volume', parents: ['demo-container'], properties: { filesystem_type: 'apfs' } }
] });
const objects = Object.fromEntries(inventory.objects.map(o => [o.local_id, o.object_id]));
console.log(`Enrolled synthetic node ${node.node_id}. Sending ${count} batches at 5-second intervals.`);
let stopping = false;
process.on('SIGINT', () => { stopping = true; });
try {
  for (let sequence = 0; sequence < count && !stopping; sequence++) {
    const time = new Date().toISOString();
    await send('POST', `${root}/heartbeat`, node.credential, { boot_id: boot, agent });
    const sample = (object_id, name, kind, value, unit) => ({ object_id, name, kind, value, unit, state: 'ok', source: 'synthetic-demo', observed_at: time });
    await send('POST', `${root}/telemetry`, node.credential, {
      schema_version: '1.0', node_id: node.node_id, boot_id: boot, sequence,
      observed_at: time, sent_at: time, agent, inventory_generation: 1,
      samples: [
        sample(objects['demo-device'], 'device_read_bytes_total', 'counter', String(1000000000 + sequence * 500000000), 'bytes'),
        sample(objects['demo-device'], 'device_write_bytes_total', 'counter', String(500000000 + sequence * 250000000), 'bytes'),
        sample(objects['demo-device'], 'smart_healthy', 'boolean', true, 'boolean'),
        sample(objects['demo-container'], 'capacity_bytes', 'gauge', 1000000000000, 'bytes'),
        sample(objects['demo-container'], 'available_bytes', 'gauge', 350000000000, 'bytes')
      ], events: []
    });
    console.log(`Accepted sequence ${sequence}`);
    if (sequence + 1 < count) await delay(5000);
  }
} finally {
  await send('POST', `${root}/goodbye`, node.credential, { boot_id: boot, reason: 'Synthetic demo completed' });
}
