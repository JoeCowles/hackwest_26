// Disposable verified-TLS test of server-only, client-only, and combined restarts.
// Build the workspace first. No enrollment token survives the initial enrollment.
import fs from 'node:fs/promises';
import path from 'node:path';
import { once } from 'node:events';
import { smokeCiderd } from './smoke-ciderd.mjs';

const insist = (value, message) => { if (!value) throw new Error(message); };
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

async function stop(child) {
  if (child.exitCode !== null || child.signalCode !== null) {
    insist(child.exitCode === 0, 'A restart target exited unexpectedly');
    return;
  }
  const exited = once(child, 'exit');
  child.kill('SIGINT');
  const timeout = setTimeout(() => child.kill('SIGKILL'), 8000);
  try { await exited; } finally { clearTimeout(timeout); }
  insist(child.exitCode === 0, 'A restart target did not shut down cleanly');
}

await smokeCiderd({ cleanupOnFailure: true, verify: async ({ root, temp, base, node, viewer, request, daemon, server, start }) => {
  const state = path.join(temp, 'server');
  const identityFile = path.join(temp, 'node.json');
  const credentialFile = path.join(temp, 'node-token');
  const config = path.join(temp, 'ciderd.toml');
  const beforeIdentity = JSON.parse(await fs.readFile(identityFile, 'utf8'));
  const beforeCredential = await fs.readFile(credentialFile, 'utf8');
  const beforeAdmin = await fs.readFile(path.join(state, 'admin-token'), 'utf8');
  const startServer = () => start(path.join(root, 'target/debug/cider-server'), [
    '--headless', '--bind', `127.0.0.1:${new URL(base).port}`, '--data-dir', state,
    '--tls-cert', path.join(temp, 'server.pem'), '--tls-key', path.join(temp, 'server-key.pem'),
  ]);
  const startClient = () => start(path.join(root, 'target/debug/ciderd'), ['run', '--config', config, '--allow-unprivileged']);

  async function detail() {
    const response = await request('GET', `/api/v1/nodes/${node}`, undefined, viewer);
    insist(response.status === 200, `Saved viewer read failed (${response.status})`);
    return response.json().data;
  }
  async function resumed(boundary, child, previous, clientRestart = false) {
    const deadline = performance.now() + 40000;
    while (performance.now() < deadline) {
      insist(child.exitCode === null && child.signalCode === null, 'Collector exited during restart verification');
      try {
        const next = await detail();
        const identity = JSON.parse(await fs.readFile(identityFile, 'utf8'));
        const contextMatches = clientRestart
          ? identity.node_id === node && String(identity.agent_generation) === next.agent_generation
            && BigInt(next.agent_generation) > BigInt(previous.agent_generation)
            && typeof next.agent_session_id === 'string' && next.agent_session_id !== previous.agent_session_id
          : next.agent_generation === previous.agent_generation && next.agent_session_id === previous.agent_session_id;
        if (Date.parse(next.last_seen_at) > boundary && next.availability === 'online' && contextMatches) return next;
      } catch { /* The server may still be binding or the collector backing off. */ }
      await sleep(1000);
    }
    throw new Error('Existing enrollment did not resume before the 40-second polling deadline');
  }

  let before = await detail();
  await stop(server);
  const serverBoundary = Date.now();
  const server2 = startServer();
  let next = await resumed(serverBoundary, daemon, before);
  insist((await fs.readFile(path.join(state, 'viewer-token'), 'utf8')).trim() === viewer, 'Server restart replaced viewer token');
  insist(await fs.readFile(path.join(state, 'admin-token'), 'utf8') === beforeAdmin, 'Server restart replaced administrator token');
  const capabilities = await request('GET', '/api/v1/capabilities', undefined, viewer);
  insist(capabilities.status === 200 && capabilities.json().data.viewer_credential_expires_at === null, 'Persistent viewer expiry is not explicitly null');

  before = next;
  await stop(daemon);
  const clientBoundary = Date.now();
  const daemon2 = startClient();
  next = await resumed(clientBoundary, daemon2, before, true);
  const restartedIdentity = JSON.parse(await fs.readFile(identityFile, 'utf8'));
  insist(restartedIdentity.node_id === node && restartedIdentity.agent_generation > beforeIdentity.agent_generation, 'Client restart failed stable identity/generation boundary');

  before = next;
  await stop(daemon2);
  await stop(server2);
  const combinedBoundary = Date.now();
  startServer();
  const daemon3 = startClient();
  await resumed(combinedBoundary, daemon3, before, true);
  const finalIdentity = JSON.parse(await fs.readFile(identityFile, 'utf8'));
  insist(finalIdentity.node_id === node && finalIdentity.agent_generation > restartedIdentity.agent_generation, 'Combined restart failed stable enrollment');
  insist(await fs.readFile(credentialFile, 'utf8') === beforeCredential, 'Client restart replaced node credential');
  const nodes = await request('GET', '/api/v1/nodes', undefined, viewer);
  insist(nodes.status === 200 && nodes.json().data.length === 1 && nodes.json().data[0].node_id === node, 'Restart created a duplicate enrollment');
  console.log(JSON.stringify({ restart_verification: 'passed', server_only: true, client_only: true, both: true, stable_node_identity: true, stable_node_credential: true, stable_viewer_credential: true, duplicate_enrollments: 0 }));
} });
