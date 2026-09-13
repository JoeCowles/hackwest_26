import test from 'node:test';
import assert from 'node:assert/strict';
import * as session from '../js/session.js';
import { App } from '../js/app.js';
import { AttentionIndicator } from '../js/operator-views.js';

const storage = () => {
  const values = new Map();
  return { getItem: key => values.get(key) ?? null, setItem: (key, value) => values.set(key, value), removeItem: key => values.delete(key) };
};
const store = backing => {
  assert.equal(typeof session.ViewerCredentialStore, 'function', 'viewer credentials need a persistent store');
  return new session.ViewerCredentialStore(() => backing);
};

test('a verified viewer credential survives a new browser session using the same site storage', () => {
  const backing = storage();
  store(backing).save('viewer_verified');
  assert.equal(store(backing).load(), 'viewer_verified');
  assert.equal(store(storage()).load(), '', 'another site has independent browser storage');
});

test('persistent storage never accepts administrator, node, or malformed credentials', () => {
  const backing = storage(), credentials = store(backing);
  for (const token of ['admin_private', 'node_private', 'viewer_', 'viewer_bad\nheader', null]) {
    assert.equal(credentials.save(token), false);
    assert.equal(store(backing).load(), '');
  }
  backing.setItem('cider.viewer-token', 'admin_private');
  assert.equal(credentials.load(), '');
  assert.equal(backing.getItem('cider.viewer-token'), null);
});

test('disconnect removes only the saved viewer credential from site storage', () => {
  const backing = storage(), credentials = store(backing);
  backing.setItem('unrelated-setting', 'keep');
  credentials.save('viewer_verified');
  assert.equal(credentials.clear(), true);
  assert.equal(store(backing).load(), '');
  assert.equal(backing.getItem('unrelated-setting'), 'keep');
});

test('blocked browser storage leaves the current session usable and explains persistence is unavailable', () => {
  assert.equal(typeof session.ViewerCredentialStore, 'function');
  const credentials = new session.ViewerCredentialStore(() => { throw new Error('blocked'); });
  assert.equal(credentials.load(), '');
  assert.equal(credentials.save('viewer_verified'), false);
  assert.match(credentials.notice, /cannot remember|could not save/i);
  assert.equal(credentials.clear(), false);
  assert.match(credentials.notice, /clear.*site.*data/i);
});

test('failed writes do not claim the browser remembered a newly connected credential', () => {
  const backing = storage(), credentials = store({ ...backing, setItem() { throw new Error('quota'); } });
  assert.equal(credentials.save('viewer_verified'), false);
  assert.equal(credentials.load(), '');
  assert.match(credentials.notice, /cannot remember|could not save/i);
});

test('disconnect falls back to an empty value when browser storage removal fails', () => {
  const backing = storage(), credentials = store({ ...backing, removeItem() { throw new Error('blocked removal'); } });
  credentials.save('viewer_verified');
  assert.equal(credentials.clear(), true);
  assert.equal(store(backing).load(), '');
});

const settle = () => new Promise(resolve => setImmediate(resolve));
const response = (data, status = 200) => new Response(JSON.stringify({ data, meta: { next_cursor: null, server_time: '2026-09-13T00:00:00Z' } }), { status });
function browser(t, backing = storage()) {
  const names = ['location', 'document', 'window', 'localStorage'];
  const previous = names.map(name => [name, Object.getOwnPropertyDescriptor(globalThis, name)]);
  const events = new Map(), timers = new Map(); let timerId = 0;
  const surface = { addEventListener: (name, fn) => events.set(name, fn), removeEventListener: name => events.delete(name) };
  Object.assign(globalThis, { location: { origin: 'https://cider.local', hash: '#io' }, document: { ...surface, hidden: false }, window: { ...surface, THREE: {} } });
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: backing });
  t.mock.method(globalThis, 'setTimeout', (fn, delay) => { const id = ++timerId; timers.set(id, { fn, delay }); return id; });
  t.mock.method(globalThis, 'clearTimeout', id => timers.delete(id));
  t.mock.method(globalThis, 'setInterval', () => ++timerId);
  t.mock.method(globalThis, 'clearInterval', () => {});
  const state = { online: true, status: 200, delay: null, requests: [] };
  t.mock.method(globalThis, 'fetch', async (url, options) => {
    state.requests.push({ url, options });
    if (state.delay) await state.delay(url, options);
    if (!state.online) throw new TypeError('Server unreachable');
    return response(url.pathname === '/api/v1/cluster' ? { marker: options.headers.Authorization, node_counts: { online: 0, degraded: 0, offline: 0, unknown: 0 }, observed_node_count: 0, expected_node_count: 0, capacity: { local: {}, shared: {}, excluded_node_ids: [] }, throughput: {} } : [], state.status);
  });
  const apps = [];
  const createApp = () => {
    const app = new App({}); apps.push(app);
    app.setState = (patch, callback) => { app.state = { ...app.state, ...(typeof patch === 'function' ? patch(app.state) : patch) }; callback?.(); };
    return app;
  };
  t.after(() => {
    apps.forEach(app => app.componentWillUnmount());
    for (const [name, descriptor] of previous) if (descriptor) Object.defineProperty(globalThis, name, descriptor); else delete globalThis[name];
  });
  return { state, backing, timers, createApp, async connect(app, token = 'viewer_verified') { app.state.tokenDraft = token; app.connect({ preventDefault() {} }); await settle(); } };
}

test('successful authentication remembers the viewer and a mounted replacement app reconnects', async t => {
  const h = browser(t), first = h.createApp();
  await h.connect(first);
  assert.equal(first.state.connected, true);
  assert.equal(first.state.error, '');
  assert.equal(store(h.backing).load(), 'viewer_verified');
  first.componentWillUnmount();
  const reloaded = h.createApp(); reloaded.componentDidMount(); await settle();
  assert.equal(reloaded.state.connected, true);
  assert.equal(reloaded.state.snapshot.cluster.marker, 'Bearer viewer_verified');
  assert.equal(reloaded.state.tokenDraft, '');
});

test('a restored credential survives downtime and retries without a new connection form', async t => {
  const h = browser(t); store(h.backing).save('viewer_verified'); h.state.online = false;
  const app = h.createApp(); app.componentDidMount(); await settle();
  assert.equal(app.state.connected, true);
  assert.equal(app.state.snapshot, null);
  assert.match(app.state.error, /unreachable/i);
  assert.equal(store(h.backing).load(), 'viewer_verified');
  const retry = h.timers.get(app.pollTimer); assert(retry, 'an outage schedules an automatic retry');
  h.state.online = true; retry.fn(); await settle();
  assert.equal(app.state.error, '');
  assert.equal(app.state.snapshot.cluster.marker, 'Bearer viewer_verified');
  assert.equal(h.timers.get(app.pollTimer).delay, 3000);
});

test('explicit disconnect removes credentials, clears private state, and does not reconnect on reload', async t => {
  const h = browser(t), app = h.createApp();
  await h.connect(app); app.state.attentionSummary = { private: true }; app.state.nodeId = 'private-host'; app.state.diskId = 'private-disk';
  app.disconnect();
  assert.equal(app.client, null); assert.equal(app.state.connected, false);
  assert.equal(app.state.snapshot, null); assert.equal(app.state.attentionSummary, null);
  assert.equal(app.state.nodeId, null); assert.equal(app.state.diskId, null);
  assert.deepEqual(app.state.history, []); assert.deepEqual(app.state.nodeHistory, {});
  assert.equal(store(h.backing).load(), '');
  const reloaded = h.createApp(); reloaded.componentDidMount(); await settle();
  assert.equal(reloaded.state.connected, false);
});

test('authentication rejection removes a saved credential instead of retrying it indefinitely', async t => {
  const h = browser(t); store(h.backing).save('viewer_rejected'); h.state.status = 401;
  const app = h.createApp(); app.componentDidMount(); await settle();
  assert.equal(app.state.connected, false);
  assert.match(app.state.error, /credential.*rejected/i);
  assert.equal(store(h.backing).load(), '');
  assert.equal(h.timers.has(app.pollTimer), false);
});

test('temporary failure cannot persist an unverified newly entered viewer credential', async t => {
  const h = browser(t), app = h.createApp(); h.state.online = false;
  await h.connect(app, 'viewer_unverified');
  assert.equal(app.state.connected, true);
  assert.equal(store(h.backing).load(), '');
});

test('late reads from a disconnected session cannot overwrite its replacement credential or data', async t => {
  const h = browser(t), app = h.createApp(); let finish;
  const pending = new Promise(resolve => { finish = resolve; });
  h.state.delay = async (_url, options) => { if (options.headers.Authorization === 'Bearer viewer_old') await pending; };
  await h.connect(app, 'viewer_old'); app.disconnect(); await h.connect(app, 'viewer_new');
  finish(); await settle();
  assert.equal(store(h.backing).load(), 'viewer_new');
  assert.equal(app.state.snapshot.cluster.marker, 'Bearer viewer_new');
});

test('a connected session remains usable when browser persistence is unavailable', async t => {
  const h = browser(t, { getItem() { throw new Error('blocked'); }, setItem() { throw new Error('blocked'); }, removeItem() { throw new Error('blocked'); } });
  const app = h.createApp(); app.componentDidMount(); await h.connect(app);
  assert.equal(app.state.connected, true);
  assert.equal(app.state.error, '');
  assert.match(app.state.credentialNotice, /could not save/i);
  app.disconnect();
  assert.equal(app.state.snapshot, null);
  assert.match(app.state.credentialNotice, /clear.*site.*data/i);
});

function findComponent(node, type) {
  if (!node || typeof node !== 'object') return null;
  if (node.type === type) return node;
  for (const child of Array.isArray(node) ? node : [node.props?.children]) { const found = findComponent(child, type); if (found) return found; }
  return null;
}

test('obsolete child callbacks cannot disconnect or repopulate a replacement session', async t => {
  const h = browser(t), app = h.createApp(); await h.connect(app, 'viewer_old');
  const old = findComponent(app.render(), AttentionIndicator).props;
  app.disconnect(); await h.connect(app, 'viewer_new');
  old.onSummary({ private: 'old-session' }); old.onAuth();
  assert.equal(app.state.connected, true);
  assert.equal(app.state.attentionSummary, null);
  assert.equal(store(h.backing).load(), 'viewer_new');
});

test('disk and attention summaries schedule their next visible refresh after three seconds', async t => {
  const h = browser(t), app = h.createApp();
  await h.connect(app);
  const diskMeta = { node_id: 'host', topology_revision: 'revision', inventory_generation: '1', disk_inventory: {} };
  app.storageKey = 'host'; app.diskClient = { get: async () => ({ data: [], meta: diskMeta }), close() {} };
  await app.pollDisks();
  assert.equal(h.timers.get(app.diskTimer).delay, 3000);
  const indicator = new AttentionIndicator({ client: app.client });
  indicator.setState = patch => { indicator.state = { ...indicator.state, ...patch }; };
  await indicator.poll();
  assert.equal(h.timers.get(indicator.timer).delay, 3000);
  document.hidden = true; await indicator.poll();
  assert.equal(h.timers.get(indicator.timer).delay, 30000);
  indicator.componentWillUnmount();
});

test('filesystem following sees added and removed rows without changing a frozen page traversal', async () => {
  let rows = ['existing'], snapshot = 0;
  const table = new session.CursorPages({ get: async (_path, params) => params.cursor
    ? { data: ['frozen second page'], meta: { next_cursor: null, server_time: String(snapshot) } }
    : { data: rows, meta: { next_cursor: 'next', server_time: String(++snapshot) } } }, '/api/v1/filesystems', {}, () => {}, () => {}, { follow: true });
  await table.refresh();
  assert.equal(typeof table.follow, 'function', 'a live first page needs an independent refresh action');
  rows = ['existing', 'added']; await table.follow();
  assert.deepEqual(table.snapshot().rows, ['existing', 'added']);
  await table.next(); rows = ['replacement']; await table.follow();
  assert.deepEqual(table.snapshot().rows, ['frozen second page']);
  table.previous(); await table.follow();
  assert.deepEqual(table.snapshot().rows, ['existing', 'added']);
  assert.equal(table.snapshot().following, false);
  await table.refresh();
  assert.deepEqual(table.snapshot().rows, ['replacement']);
  assert.equal(table.snapshot().following, true);
});

test('filesystem following respects Retry-After while retaining dated rows', async () => {
  let limited = false, requests = 0;
  const table = new session.CursorPages({ get: async () => {
    requests++;
    if (limited) throw Object.assign(new Error('Read budget exhausted'), { status: 429, retryAfter: 10 });
    return { data: ['dated filesystem'], meta: { next_cursor: null } };
  } }, '/api/v1/filesystems', {}, () => {}, () => {}, { follow: true });
  await table.refresh(); limited = true;
  assert.equal(typeof table.follow, 'function');
  await table.follow(); await table.follow();
  assert.deepEqual(table.snapshot().rows, ['dated filesystem']);
  assert.match(table.snapshot().error, /budget/i);
  assert.equal(requests, 2);
});

test('active filesystem rows refresh automatically and obsolete tables stop scheduling reads', async t => {
  const h = browser(t), app = h.createApp(); app.state.view = 'fs';
  await h.connect(app);
  const next = h.timers.get(app.tableTimer);
  assert.equal(next?.delay, 3000);
  next.fn(); await settle();
  assert.equal(h.state.requests.filter(request => request.url.pathname === '/api/v1/filesystems').length, 2);
  app.disconnect();
  const requestCount = h.state.requests.length;
  next.fn(); await settle();
  assert.equal(h.state.requests.length, requestCount);
});

test('attention refresh waits for Retry-After without dropping previously received evidence', async t => {
  const h = browser(t), indicator = new AttentionIndicator({ client: { get: async () => { throw Object.assign(new Error('Read budget exhausted'), { status: 429, retryAfter: 12 }); } } });
  indicator.state.data = { open: 3 };
  indicator.setState = patch => { indicator.state = { ...indicator.state, ...patch }; };
  await indicator.poll();
  assert.equal(h.timers.get(indicator.timer).delay, 12000);
  assert.deepEqual(indicator.state.data, { open: 3 });
  assert.match(indicator.state.error, /budget/i);
  indicator.componentWillUnmount();
});

test('an unmounted page cannot remember a credential when its pending read settles later', async t => {
  const h = browser(t), app = h.createApp(); let finish;
  const pending = new Promise(resolve => { finish = resolve; }); h.state.delay = async () => pending;
  await h.connect(app); app.componentWillUnmount(); finish(); await settle();
  assert.equal(store(h.backing).load(), '');
  assert.equal(app.state.snapshot, null);
});
