import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { ApiClient } from '../js/api.js';
import * as session from '../js/session.js';
import * as stage from '../js/stage.js';

const envelope = data => ({ data, meta: { api_version: '1', next_cursor: null } });

async function withServer(handler, run) {
  const server = createServer(handler);
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  const previous = globalThis.location;
  globalThis.location = { origin: `http://127.0.0.1:${server.address().port}` };
  try { await run(); }
  finally { globalThis.location = previous; server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); }
}

function json(response, body, status = 200, headers = {}) {
  response.writeHead(status, { 'Content-Type': 'application/json', ...headers });
  response.end(JSON.stringify(body));
}

test('the console rendering dependency imports without network access', async () => {
  await assert.doesNotReject(() => import('../js/lib.js'));
});

test('paginated reads retain accepted pages when the next page is temporarily rate limited', async () => {
  let retries = 0;
  await withServer((request, response) => {
    assert.equal(request.headers.authorization, 'Bearer viewer_test');
    const url = new URL(request.url, 'http://localhost');
    if (!url.searchParams.has('cursor')) return json(response, { data: ['first'], meta: { api_version: '1', next_cursor: 'second' } });
    if (retries++ === 0) return json(response, { error: { message: 'Read budget exhausted' } }, 429, { 'Retry-After': '0' });
    return json(response, envelope(['second']));
  }, async () => {
    const client = new ApiClient('viewer_test');
    try { assert.deepEqual(await client.all('/api/v1/nodes'), ['first', 'second']); }
    finally { client.close(); }
  });
});

test('cross-origin URLs are rejected before sending a bearer credential', async () => {
  const previous = globalThis.location;
  globalThis.location = { origin: 'http://127.0.0.1:8787' };
  const client = new ApiClient('viewer_test');
  try { await assert.rejects(client.get('https://example.invalid/api/v1/nodes'), /server itself/); }
  finally { client.close(); globalThis.location = previous; }
});

test('a closed client rejects further reads without making a request', async () => {
  let calls = 0;
  await withServer((_request, response) => { calls++; json(response, envelope([])); }, async () => {
    const client = new ApiClient('viewer_test'); client.close();
    await assert.rejects(client.get('/api/v1/nodes'), { name: 'AbortError' });
    assert.equal(calls, 0);
  });
});

test('repeated pagination cursors are rejected instead of looping', async () => {
  await withServer((_request, response) => json(response, { data: ['one'], meta: { api_version: '1', next_cursor: 'repeat' } }), async () => {
    const client = new ApiClient('viewer_test');
    try { await assert.rejects(client.all('/api/v1/nodes'), /repeated.*cursor/); }
    finally { client.close(); }
  });
});

const clientFor = inventory => ({
  async get() { return envelope({ marker: 'current cluster' }); },
  async all(path, params) {
    if (path === '/api/v1/nodes') return [{ node_id: 'one', inventory_generation: '12' }];
    if (path.endsWith('/inventory')) { assert.deepEqual(params, { generation: '12' }); return inventory(); }
    return [];
  }
});

test('an empty successful host inventory is complete, not perpetually loading', async () => {
  assert.equal(typeof session.readSnapshot, 'function');
  const result = await session.readInventory(clientFor(() => []), { node_id: 'one', inventory_generation: '12' });
  assert.equal(result.inventoryNode, 'one');
  assert.deepEqual(result.inventory, []);
  assert.equal(result.inventoryError, '');
});

test('an inventory-only failure retains current cluster data and identifies the failed host', async () => {
  assert.equal(typeof session.readSnapshot, 'function');
  const result = await session.readInventory(clientFor(() => { throw new Error('Inventory unavailable'); }), { node_id: 'one', inventory_generation: '12' });
  assert.equal(result.inventoryNode, 'one');
  assert.equal(result.inventoryError, 'Inventory unavailable');
  assert.deepEqual(result.inventory, []);
});

test('inventory authentication rejection still terminates the read session', async () => {
  assert.equal(typeof session.readSnapshot, 'function');
  const error = Object.assign(new Error('Expired credential'), { status: 401 });
  await assert.rejects(session.readInventory(clientFor(() => { throw error; }), { node_id: 'one', inventory_generation: '12' }), error);
});

test('deep links decode host IDs and unavailable routes fall back to overview', () => {
  assert.equal(typeof session.parseRoute, 'function');
  assert.deepEqual(session.parseRoute('#node/%6fne'), { view: 'node', nodeId: 'one' });
  assert.deepEqual(session.parseRoute('#alerts'), { view: 'overview', nodeId: null });
  assert.deepEqual(session.parseRoute('#node/%ZZ'), { view: 'node', nodeId: null });
});

test('rack drags rotate without opening a host, while a touch tap selects the hit host', () => {
  assert.equal(typeof stage.rackPointerHandlers, 'function');
  const opened = [], rotations = [], hovered = [];
  const handlers = stage.rackPointerHandlers({ pick: () => 'one', onOpen: id => opened.push(id), onHover: id => hovered.push(id), onRotate: delta => rotations.push(delta) });
  handlers.down({ clientX: 10, clientY: 10, pointerId: 1, button: 0 });
  handlers.move({ clientX: 50, clientY: 10, pointerId: 1 });
  handlers.up({ clientX: 50, clientY: 10, pointerId: 1 });
  assert.deepEqual(opened, []);
  assert.deepEqual(rotations, [40]);
  handlers.down({ clientX: 10, clientY: 10, pointerId: 2, button: 0 });
  handlers.up({ clientX: 10, clientY: 10, pointerId: 2 });
  assert.deepEqual(opened, ['one']);
  assert.equal(hovered.at(-1), 'one');
});

test('a delayed snapshot is rejected instead of receiving a fresh success timestamp', async () => {
  let calls = 0;
  await assert.rejects(session.readSnapshot(clientFor(() => []), () => calls++ ? 16000 : 0), /refresh.*15 seconds/i);
});

test('authentication rejection takes precedence over a simultaneous unavailable route', async () => {
  const unauthorized = Object.assign(new Error('Expired credential'), { status: 401 });
  const client = clientFor(() => []);
  client.get = async () => { throw new Error('Temporary database failure'); };
  client.all = async () => { throw unauthorized; };
  await assert.rejects(session.readSnapshot(client), unauthorized);
});

test('pending navigation honors retry deadlines and hidden-tab polling intervals', () => {
  assert.equal(typeof session.minimumRefreshDelay, 'function');
  assert.equal(session.minimumRefreshDelay(false, 9000, 1000), 8000);
  assert.equal(session.minimumRefreshDelay(true, 9000, 1000), 30000);
  assert.equal(session.minimumRefreshDelay(false, 1000, 9000), 0);
});


test('core polling completes while a view-specific table request is still pending', async () => {
  let completeTable;
  const tableRequest = new Promise(resolve => { completeTable = resolve; });
  const client = {
    async get(path) { if (path === '/api/v1/filesystems') return tableRequest; return envelope({ marker: 'fresh core' }); },
    async all(path) { assert.equal(path, '/api/v1/nodes'); return []; }
  };
  assert.equal(typeof session.CursorPages, 'function');
  const table = new session.CursorPages(client, '/api/v1/filesystems');
  const loading = table.refresh();
  const core = await session.readSnapshot(client);
  assert.equal(core.cluster.marker, 'fresh core');
  assert.equal(table.snapshot().busy, true);
  completeTable(envelope(['filesystem']));
  await loading;
  assert.deepEqual(table.snapshot().rows, ['filesystem']);
});

test('cursor navigation retains filters and frozen pages without silently truncating rows', async () => {
  assert.equal(typeof session.CursorPages, 'function');
  const requested = [];
  const client = { async get(path, params) {
    requested.push([path, params]);
    return params.cursor ? { data: ['last'], meta: { next_cursor: null, server_time: '2026-09-12T12:00:00Z' } }
      : { data: ['first', 'second'], meta: { next_cursor: 'next-page', server_time: '2026-09-12T12:00:00Z' } };
  } };
  const table = new session.CursorPages(client, '/api/v1/events', { category: 'security' });
  await table.refresh();
  assert.equal(table.snapshot().canNext, true);
  await table.next();
  assert.deepEqual(requested[1], ['/api/v1/events', { category: 'security', limit: 100, cursor: 'next-page' }]);
  assert.deepEqual(table.snapshot().rows, ['last']);
  assert.equal(table.snapshot().rangeStart, 3);
  assert.equal(table.snapshot().rangeEnd, 3);
  assert.equal(table.snapshot().canNext, false);
  table.previous();
  assert.deepEqual(table.snapshot().rows, ['first', 'second']);
  await table.next();
  assert.equal(requested.length, 2);
});

test('expired cursors retain displayed rows until explicit snapshot refresh', async () => {
  assert.equal(typeof session.CursorPages, 'function');
  let version = 'old';
  const client = { async get(_path, params) {
    if (params.cursor) throw Object.assign(new Error('Snapshot is no longer available'), { status: 410 });
    return { data: [version], meta: { next_cursor: 'expired', server_time: '2026-09-12T12:00:00Z' } };
  } };
  const table = new session.CursorPages(client, '/api/v1/filesystems');
  await table.refresh(); await table.next();
  assert.deepEqual(table.snapshot().rows, ['old']);
  assert.equal(table.snapshot().expired, true);
  assert.equal(table.snapshot().canNext, false);
  assert.match(table.snapshot().error, /refresh snapshot/i);
  version = 'new'; await table.refresh();
  assert.deepEqual(table.snapshot().rows, ['new']);
  assert.equal(table.snapshot().expired, false);
});

test('each view requests only its supported optional collection', () => {
  assert.equal(typeof session.viewCollection, 'function');
  assert.equal(session.viewCollection('io', 'one'), null);
  assert.deepEqual(session.viewCollection('fs', 'one'), { path: '/api/v1/filesystems', params: {} });
  assert.deepEqual(session.viewCollection('sec', 'one'), { path: '/api/v1/events', params: { category: 'security' } });
  assert.deepEqual(session.viewCollection('node', 'one'), { path: '/api/v1/events', params: { node_id: 'one' } });
});

test('visible table navigation calls real paging actions and disables unavailable directions', async () => {
  const views = await import('../js/views.js');
  assert.equal(typeof views.PageControls, 'function');
  const table = new session.CursorPages({ async get(_path, params) {
    return params.cursor ? { data: ['second'], meta: { next_cursor: null } } : { data: ['first'], meta: { next_cursor: 'second' } };
  } }, '/api/v1/filesystems');
  await table.refresh();
  const controls = () => {
    const result = [];
    const visit = node => {
      if (Array.isArray(node)) { node.forEach(visit); return; }
      if (!node || typeof node !== 'object') return;
      if (typeof node.type === 'function') { visit(node.type(node.props)); return; }
      if (node.type === 'button') result.push(node);
      visit(node.props?.children);
    };
    visit(views.PageControls({ paging: { ...table.snapshot(), previous: () => table.previous(), next: () => table.next(), refresh: () => table.refresh() } }));
    return Object.fromEntries(result.map(button => [button.props.children, button.props]));
  };
  assert.equal(controls().Previous.disabled, true);
  assert.equal(controls().Next.disabled, false);
  await controls().Next.onClick();
  assert.deepEqual(table.snapshot().rows, ['second']);
  assert.equal(controls().Next.disabled, true);
  controls().Previous.onClick();
  assert.deepEqual(table.snapshot().rows, ['first']);
  await controls()['Refresh snapshot'].onClick();
  assert.equal(table.snapshot().pageNumber, 1);
});

test('route changes never reuse a different table schema while the new page loads', async () => {
  assert.equal(typeof session.collectionForView, 'function');
  const table = new session.CursorPages({ async get() { return envelope([{ event_id: 'one' }]); } }, '/api/v1/events');
  await table.refresh();
  const collection = table.snapshot();
  assert.equal(session.collectionForView(collection, 'overview', null), collection);
  assert.equal(session.collectionForView(collection, 'fs', null), null);
  assert.equal(session.collectionForView(collection, 'sec', null), null);
});

test('expiry does not falsely label a partial table as the end of its snapshot', async () => {
  const table = new session.CursorPages({ async get(_path, params) {
    if (params.cursor) throw Object.assign(new Error('expired'), { status: 410 });
    return { data: ['first'], meta: { next_cursor: 'next' } };
  } }, '/api/v1/filesystems');
  await table.refresh(); await table.next();
  assert.equal(table.snapshot().canNext, false);
  assert.equal(table.snapshot().moreAvailable, true);
});

test('closing an obsolete table suppresses its late rows and callbacks', async () => {
  let finish, changes = 0;
  const table = new session.CursorPages({ async get() { return new Promise(resolve => { finish = resolve; }); } }, '/api/v1/events', {}, () => changes++);
  const request = table.refresh();
  table.close();
  finish(envelope(['obsolete']));
  await request;
  assert.deepEqual(table.snapshot().rows, []);
  assert.equal(changes, 1);
});

test('resuming from sleep expires summary values even when the monotonic clock paused', () => {
  assert.equal(typeof session.snapshotExpired, 'function');
  assert.equal(session.snapshotExpired(1000, 100000, 1100, 120000), true);
  assert.equal(session.snapshotExpired(1000, 100000, 17000, 90000), true);
  assert.equal(session.snapshotExpired(1000, 100000, 6000, 105000), false);
});

test('a table authentication failure is surfaced to the shared session', async () => {
  const rejected = Object.assign(new Error('Expired viewer'), { status: 401 });
  let authError;
  const table = new session.CursorPages({ async get() { throw rejected; } }, '/api/v1/events', {}, () => {}, error => { authError = error; });
  await table.refresh();
  assert.equal(authError, rejected);
  assert.deepEqual(table.snapshot().rows, []);
});

test('a core response received after sleep cannot be stamped fresh when its monotonic clock paused', async () => {
  let wallReads = 0;
  await assert.rejects(session.readSnapshot(clientFor(() => []), () => 1000, () => wallReads++ ? 120000 : 100000), /refresh.*15 seconds/i);
});

test('an inventory response received after sleep cannot be accepted with a fresh arrival time', async () => {
  let wallReads = 0;
  await assert.rejects(session.readInventory(clientFor(() => []), { node_id: 'one', inventory_generation: '12' }, () => 1000, () => wallReads++ ? 120000 : 100000), /inventory refresh.*15 seconds/i);
});
