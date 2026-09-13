// End-to-end local TLS test. Build the workspace first. --packaged explicitly
// checks the separately built debug app instead of silently testing a stale bundle.
import fs from 'node:fs/promises';
import path from 'node:path';
import https from 'node:https';
import net from 'node:net';
import { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { once } from 'node:events';
import { fileURLToPath } from 'node:url';

process.umask(0o077);
const root = fileURLToPath(new URL('../', import.meta.url));
export async function smokeCiderd({ packaged = false, nativeWindow = false, beforeEnroll = async () => {}, verify = async () => {}, cleanupOnFailure = false } = {}) {
const serverBinary = path.join(root, packaged ? 'dist/Orchard Server.app/Contents/MacOS/orchard-server' : 'target/debug/orchard-server');
const staging = path.join(root, '.codex-staging');
await fs.mkdir(staging, { recursive: true, mode: 0o700 });
const temp = await fs.mkdtemp(path.join(staging, 'cider-smoke-'));
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const children = [];
const logs = new Map();
let interrupted = false;
const stopOnSignal = () => {
  interrupted = true;
  for (const child of children) if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
};
process.once('SIGINT', stopOnSignal);
process.once('SIGTERM', stopOnSignal);
const insist = (value, message) => { if (!value) throw new Error(message); };

function start(executable, args) {
  insist(!interrupted, 'Smoke test interrupted');
  const child = spawn(executable, args, { cwd: temp, stdio: ['ignore', 'pipe', 'pipe'] });
  let output = '';
  child.stdout.on('data', chunk => { output = (output + chunk).slice(-32768); });
  child.stderr.on('data', chunk => { output = (output + chunk).slice(-32768); });
  logs.set(child, () => output);
  children.push(child);
  return child;
}

async function run(executable, args) {
  const child = start(executable, args);
  const timer = setTimeout(() => child.kill('SIGKILL'), 20000);
  const [code, signal] = await once(child, 'exit');
  clearTimeout(timer);
  insist(code === 0, `${path.basename(executable)} failed (${code ?? signal})`);
  return logs.get(child)();
}

let succeeded = false;
try {
  const caConfig = path.join(temp, 'ca.cnf');
  const leafConfig = path.join(temp, 'leaf.cnf');
  await fs.writeFile(caConfig, '[req]\nprompt=no\ndistinguished_name=dn\nx509_extensions=ca\n[dn]\nCN=Orchard integration test CA\n[ca]\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash\n');
  await fs.writeFile(leafConfig, 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1,DNS:localhost\n');
  const ca = path.join(temp, 'ca.pem'), caKey = path.join(temp, 'ca-key.pem');
  const key = path.join(temp, 'server-key.pem'), cert = path.join(temp, 'server.pem'), csr = path.join(temp, 'server.csr');
  await run('/usr/bin/openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-config', caConfig, '-keyout', caKey, '-out', ca]);
  await run('/usr/bin/openssl', ['req', '-new', '-newkey', 'rsa:2048', '-nodes', '-subj', '/CN=127.0.0.1', '-keyout', key, '-out', csr]);
  await run('/usr/bin/openssl', ['x509', '-req', '-in', csr, '-CA', ca, '-CAkey', caKey, '-set_serial', '1', '-days', '1', '-extfile', leafConfig, '-out', cert]);
  const reservation = net.createServer();
  reservation.listen(0, '127.0.0.1');
  await once(reservation, 'listening');
  const port = reservation.address().port;
  await new Promise(resolve => reservation.close(resolve));
  const base = `https://127.0.0.1:${port}`;
  const state = path.join(temp, 'server');
  const server = start(serverBinary, [...(nativeWindow ? [] : ['--headless']), '--bind', `127.0.0.1:${port}`, '--data-dir', state, '--tls-cert', cert, '--tls-key', key]);
  let admin;
  for (let i = 0; i < 100; i++) {
    try { admin = (await fs.readFile(path.join(state, 'admin-token'), 'utf8')).trim(); break; } catch {}
    insist(server.exitCode === null, 'Server exited during startup');
    await sleep(100);
  }
  insist(admin, 'Server did not create its private administrator credential');
  const trust = await fs.readFile(ca);
  async function request(method, url, body, credential = admin) {
    const payload = body === undefined ? undefined : JSON.stringify(body);
    return new Promise((resolve, reject) => {
      const headers = { authorization: `Bearer ${credential}`, 'x-request-id': randomUUID(), 'x-request-timestamp': new Date().toISOString() };
      if (payload !== undefined) { headers['content-type'] = 'application/json'; headers['content-length'] = Buffer.byteLength(payload); }
      const req = https.request(new URL(url, base), { method, headers, ca: trust, rejectUnauthorized: true, timeout: 4000 }, res => {
        let text = '';
        res.setEncoding('utf8');
        res.on('data', chunk => { text += chunk; if (text.length > 2 * 1024 * 1024) res.destroy(new Error('Response exceeds smoke-test bound')); });
        res.on('error', reject);
        res.on('end', () => resolve({ status: res.statusCode, text, json: () => JSON.parse(text) }));
      });
      req.on('error', reject);
      req.on('timeout', () => req.destroy(new Error('HTTPS request timed out')));
      req.end(payload);
    });
  }
  let issued;
  for (let i = 0; i < 50; i++) {
    try { issued = await request('POST', '/api/v1/enrollment-tokens', {}); break; } catch { await sleep(100); }
  }
  insist(issued?.status === 201, 'Could not create enrollment token over verified TLS');
  await beforeEnroll({ request, base });
  const tokenFile = path.join(temp, 'one-use-token');
  await fs.writeFile(tokenFile, issued.json().data.enrollment_token, { mode: 0o600 });
  const identity = path.join(temp, 'node.json'), credential = path.join(temp, 'node-token'), nodeState = path.join(temp, 'node-state');
  let config = await fs.readFile(path.join(root, 'crates/ciderd/examples/ciderd.toml'), 'utf8');
  for (const [field, value] of Object.entries({ identity_file: identity, state_directory: nodeState, bearer_token_file: credential, endpoint: base + '/api/v2/ciderd/heartbeat' })) {
    config = config.replace(new RegExp(`^${field}\\s*=.*$`, 'm'), `${field} = ${JSON.stringify(value)}`);
  }
  config = config.replace('[heartbeat]', '[heartbeat]\nca_certificate_file = ' + JSON.stringify(ca));
  const configFile = path.join(temp, 'ciderd.toml');
  await fs.writeFile(configFile, config);
  const cider = path.join(root, 'target/debug/ciderd');
  await run(cider, ['enroll-server', '--config', configFile, '--name', 'Local TLS integration test', '--enrollment-token-file', tokenFile]);
  const node = JSON.parse(await fs.readFile(identity, 'utf8')).node_id;
  insist(((await fs.stat(credential)).mode & 0o077) === 0, 'Node credential is not owner-only');
  insist(((await fs.stat(identity)).mode & 0o077) === 0, 'Node identity is not owner-only');
  const directory = await fs.stat(nodeState);
  insist(directory.isDirectory() && (directory.mode & 0o077) === 0, 'Enrollment did not prepare a private collector state directory');
  await fs.unlink(tokenFile);
  const daemon = start(cider, ['run', '--config', configFile, '--allow-unprivileged']);
  const seen = new Set();
  let metricCount = 0;
  function countMetrics(value) {
    if (!value || typeof value !== 'object') return 0;
    return (value.ciderd?.collection_id ? 1 : 0) + Object.values(value).reduce((n, child) => n + countMetrics(child), 0);
  }
  for (let i = 0; i < 25; i++) {
    insist(daemon.exitCode === null, 'Collector exited before delivering real telemetry');
    const detail = await request('GET', `/api/v1/nodes/${node}`);
    insist(detail.status === 200, 'Node read failed');
    const last = detail.json().data.last_seen_at;
    if (last) seen.add(last);
    if (i % 2 === 0) {
      const inventory = await request('GET', `/api/v1/nodes/${node}/inventory?limit=100`);
      insist(inventory.status === 200, 'Inventory read failed');
      metricCount = countMetrics(inventory.json());
    }
    if (seen.size >= 2 && metricCount > 0) break;
    await sleep(1000);
  }
  insist(seen.size >= 2, 'Did not observe two accepted five-second heartbeats');
  insist(metricCount > 0, 'No real collector measurements appeared in the read API');
  const viewer = (await fs.readFile(path.join(state, 'viewer-token'), 'utf8')).trim();
  const readRoutes = ['/api/v1/capabilities', '/api/v1/cluster', '/api/v1/nodes', `/api/v1/nodes/${node}`, `/api/v1/nodes/${node}/inventory`, '/api/v1/filesystems', '/api/v1/events'];
  for (const route of readRoutes) {
    const response = await request('GET', route, undefined, viewer);
    insist(response.status === 200, `Viewer read failed: ${route} (${response.status})`);
    const body = response.json();
    insist(body.meta?.api_version === '1' && Object.hasOwn(body, 'data'), `Invalid read envelope: ${route}`);
    if (route.endsWith('/inventory')) {
      insist(body.data.length > 0, 'Current inventory is empty after real collection');
      const detail = await request('GET', `/api/v1/objects/${body.data[0].object_id}`, undefined, viewer);
      insist(detail.status === 200, 'Viewer object detail read failed');
    }
  }
  await verify({ root, temp, base, node, viewer, request, daemon, server, trust, start, run });
  const page = await request('GET', '/');
  insist(page.status === 200 && /Orchard/.test(page.text), 'Embedded web console did not load');
  const help = await run(cider, ['enroll-server', '--help']);
  insist(help.includes('Enroll with Orchard'), 'Enrollment help description is incorrect');
  console.log(JSON.stringify({ result: 'passed', server_binary: packaged ? 'packaged' : 'workspace', verified_tls: true, cli_enrollment: true, private_credentials: true, private_state_directory: true, distinct_heartbeats: seen.size, read_api_measurement_occurrences: metricCount, viewer_read_routes: 8, embedded_console: true, help_text: true }));
  succeeded = true;
} catch (error) {
  let i = 0;
  if (!cleanupOnFailure) for (const log of logs.values()) await fs.writeFile(path.join(temp, `process-${++i}.log`), log(), { mode: 0o600 });
  console.error(error.message);
  if (!cleanupOnFailure) console.error('Private diagnostic artifacts: ' + temp);
  process.exitCode = 1;
} finally {
  for (const child of [...children].reverse()) if (child.exitCode === null && child.signalCode === null) {
    const stopped = once(child, 'exit');
    child.kill('SIGTERM');
    const timeout = setTimeout(() => child.kill('SIGKILL'), 8000);
    await stopped;
    clearTimeout(timeout);
  }
  if (succeeded || cleanupOnFailure) await fs.rm(temp, { recursive: true, force: true });
  process.removeListener('SIGINT', stopOnSignal);
  process.removeListener('SIGTERM', stopOnSignal);
}

}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.slice(2).some(arg => arg !== '--packaged')) throw new Error('Usage: node scripts/smoke-ciderd.mjs [--packaged]');
  await smokeCiderd({ packaged: process.argv.includes('--packaged') });
}
