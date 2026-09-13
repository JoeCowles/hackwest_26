// Independent live summary, inventory, and frozen table snapshots.
import { NAV } from './data.js';

const VIEWER_TOKEN_KEY = 'cider.viewer-token';
export const validViewerToken = token => typeof token === 'string' && /^viewer_[A-Za-z0-9_-]+$/.test(token);

// localStorage is scoped by the browser to this console's origin. Store only
// verified read-only credentials, never snapshots or administrator credentials.
export class ViewerCredentialStore {
  constructor(storage = () => globalThis.localStorage) { this.storage = storage; this.notice = ''; }
  load() {
    try {
      const token = this.storage().getItem(VIEWER_TOKEN_KEY);
      if (validViewerToken(token)) return token;
      if (token) this.clear();
    } catch { this.notice = 'This browser cannot remember the viewer credential. You can still connect for this page session.'; }
    return '';
  }
  save(token) {
    if (!validViewerToken(token)) return false;
    try {
      this.storage().setItem(VIEWER_TOKEN_KEY, token);
      this.notice = ''; return true;
    } catch {
      this.notice = 'This browser could not save the viewer credential. Keep this page open, or reconnect after reloading.';
      return false;
    }
  }
  clear() {
    try { this.storage().removeItem(VIEWER_TOKEN_KEY); this.notice = ''; return true; }
    catch {
      try { this.storage().setItem(VIEWER_TOKEN_KEY, ''); this.notice = ''; return true; }
      catch {
        this.notice = "Disconnected from this page. The browser could not remove its saved viewer credential; clear this site's browser data to remove it.";
        return false;
      }
    }
  }
}

export function parseRoute(hash) {
  const [candidate, encodedId, detailPart, encodedDetail] = hash.replace(/^#\/?/, '').split('/');
  const view = NAV.some(([, , id]) => id === candidate) ? candidate : 'overview';
  if(view==='ops') {
    const operation=['attention','notifications','quotas','diagnostics','history'].includes(encodedId)?encodedId:'attention';
    let objectId=null;
    try{if(operation==='history' && detailPart)objectId=decodeURIComponent(detailPart);}catch{/* invalid object link remains unselected */}
    return {view,nodeId:null,operation,objectId};
  }
  let nodeId = null;
  if (view === 'node' && encodedId) {
    try { nodeId = decodeURIComponent(encodedId); } catch { /* malformed link: show host selection */ }
  }
  if (view === 'node' && detailPart === 'disk') {
    let diskId = null;
    try { if (nodeId && encodedDetail) diskId = decodeURIComponent(encodedDetail); } catch { /* malformed disk link */ }
    return { view, nodeId, diskId };
  }
  if (view === 'node' && detailPart === 'object') {
    let objectId = null;
    try { if (nodeId && encodedDetail) objectId = decodeURIComponent(encodedDetail); } catch { /* malformed source-object link */ }
    return { view, nodeId, objectId };
  }
  return { view, nodeId };
}

export const MAX_SNAPSHOT_AGE_MS = 15000;
export const snapshotExpired = (arrivedAt, wallAt, now = performance.now(), wallNow = Date.now()) => Math.max(now - arrivedAt, wallNow - wallAt) >= MAX_SNAPSHOT_AGE_MS;
export const minimumRefreshDelay = (hidden, retryAt = 0, now = performance.now()) => Math.max(hidden ? 30000 : 0, retryAt - now, 0);

// Only the live summary belongs to the periodic core poll. Tables and inventory
// have independent requests and cannot delay these values.
export async function readSnapshot(client, now = () => performance.now(), wallNow = () => Date.now()) {
  const started = now(), wallStarted = wallNow();
  const results = await Promise.allSettled([client.get('/api/v1/cluster'), client.all('/api/v1/nodes', {}, { releasePrevious: true })]);
  const failures = results.filter(result => result.status === 'rejected');
  const failure = failures.find(result => [401, 403].includes(result.reason?.status)) || failures[0];
  if (failure) throw failure.reason;
  const [cluster, nodes] = results.map(result => result.value);
  if (snapshotExpired(started, wallStarted, now(), wallNow())) throw new Error('Core refresh exceeded 15 seconds. Current values are hidden until a timely refresh succeeds.');
  return { cluster: cluster.data, nodes };
}

export async function readInventory(client, node, now = () => performance.now(), wallNow = () => Date.now()) {
  const started = now(), wallStarted = wallNow();
  const result = { inventoryNode: node.node_id, inventoryGeneration: node.inventory_generation, inventory: [], inventoryError: '' };
  try {
    result.inventory = await client.all(`/api/v1/nodes/${encodeURIComponent(node.node_id)}/inventory`, { generation: node.inventory_generation });
  } catch (error) {
    if ([401, 403].includes(error.status) || error.name === 'AbortError') throw error;
    result.inventoryError = error.status === 400 ? 'Host inventory changed during this refresh. Retrying on the next poll.' : error.message || 'Inventory is unavailable. Retrying on the next poll.';
  }
  if (snapshotExpired(started, wallStarted, now(), wallNow())) throw new Error('Inventory refresh exceeded 15 seconds. Retrying independently on the next core poll.');
  return result;
}

export function viewCollection(view, nodeId) {
  if (view === 'fs') return { path: '/api/v1/filesystems', params: {} };
  if (view === 'overview') return { path: '/api/v1/events', params: {} };
  if (view === 'sec') return { path: '/api/v1/events', params: { category: 'security' } };
  if (view === 'node' && nodeId) return { path: '/api/v1/events', params: { node_id: nodeId } };
  return null;
}

export function collectionForView(collection, view, nodeId) {
  const config = viewCollection(view, nodeId);
  return config && collection?.key === JSON.stringify(config) ? collection : null;
}

// Each table owns a frozen server cursor snapshot. Cached previous pages remain
// reviewable after cursor expiry; Refresh explicitly starts a new traversal.
export class CursorPages {
  constructor(client, path, params = {}, onChange = () => {}, onAuth = () => {}, options = {}) {
    this.client = client; this.path = path; this.params = params;
    this.onChange = onChange; this.onAuth = onAuth;
    this.pages = []; this.index = 0; this.busy = false; this.error = ''; this.expired = false; this.closed = false;
    this.followEnabled = options.follow === true; this.following = this.followEnabled; this.retryAt = 0;
  }
  snapshot() {
    const page = this.pages[this.index];
    const offset = this.pages.slice(0, this.index).reduce((sum, p) => sum + p.data.length, 0);
    return { key: JSON.stringify({ path: this.path, params: this.params }), rows: page?.data || [], meta: page?.meta, pageNumber: this.index + 1,
      rangeStart: page?.data.length ? offset + 1 : 0, rangeEnd: offset + (page?.data.length || 0),
      moreAvailable: !!page?.meta.next_cursor, canPrevious: this.index > 0, canNext: !!this.pages[this.index + 1] || (!this.expired && !!page?.meta.next_cursor),
      busy: this.busy, error: this.error, expired: this.expired, following: this.followEnabled ? this.following : null };
  }
  notify() { if (!this.closed) this.onChange(this.snapshot()); }
  close() { this.closed = true; this.client.close?.(); }
  previous() { if (!this.busy && this.index > 0) { this.index--; this.notify(); } }
  async next() {
    if (this.busy || this.closed) return;
    if (this.snapshot().canNext) this.following = false;
    if (this.pages[this.index + 1]) { this.index++; this.notify(); return; }
    const cursor = this.pages[this.index]?.meta.next_cursor;
    if (cursor && !this.expired) await this.load(cursor);
  }
  async refresh() { if (!this.busy && !this.closed && performance.now() >= this.retryAt) { this.following = this.followEnabled; await this.load(null); } }
  async follow() {
    if (this.following && !this.busy && !this.closed && performance.now() >= this.retryAt) await this.load(null, this.pages[0]?.meta.next_cursor);
  }
  async load(cursor, releaseCursor = null) {
    // The server may release this handle before a replacement fails. Keep the
    // dated local rows, but prevent Next until a new first page is accepted.
    if (releaseCursor) this.expired = true;
    this.busy = true; this.error = ''; this.notify();
    try {
      const page = await this.client.get(this.path, { ...this.params, limit: 100, ...(cursor ? { cursor } : {}) }, { releaseCursor });
      if (this.closed) return;
      if (!Array.isArray(page.data)) throw new Error('Expected a paginated collection.');
      this.retryAt = 0;
      if (cursor) {
        if (page.meta.next_cursor && this.pages.some(p => p.meta.next_cursor === page.meta.next_cursor)) throw new Error('The server repeated a pagination cursor. Refresh snapshot to retry.');
        this.pages.push(page); this.index++;
      } else { this.pages = [page]; this.index = 0; this.expired = false; }
    } catch (error) {
      if (this.closed || error.name === 'AbortError') return;
      if ([401, 403].includes(error.status)) this.onAuth(error);
      if (error.status === 429) this.retryAt = performance.now() + Math.max(1000, (error.retryAfter || 0) * 1000);
      this.expired = this.expired || error.status === 410;
      this.error = error.status === 410 ? 'This cursor expired. Refresh snapshot to load current rows.' : error.message || 'Table data is unavailable. Refresh snapshot to retry.';
      if (releaseCursor) this.error += ' The prior cursor may have been released. Refresh snapshot or wait for automatic refresh before paging.';
    } finally { this.busy = false; this.notify(); }
  }
}

// Structural identity intentionally excludes inventory generation and freshness.
// Raw observations retain their frozen generation, even when reused with new rates.
export function topologyStamp(meta) {
  return meta && typeof meta.node_id === 'string' && typeof meta.topology_revision === 'string'
    ? JSON.stringify([meta.node_id, meta.boot_id ?? null, meta.topology_revision]) : null;
}
const frozenMetadata = meta => JSON.stringify([topologyStamp(meta),meta?.inventory_generation,meta?.disk_inventory]);
function validateStoragePage(page, nodeId, firstMeta) {
  if (!Array.isArray(page.data) || !topologyStamp(page.meta) || page.meta.node_id !== nodeId) throw new Error('Storage snapshot metadata is missing or belongs to another host.');
  if (firstMeta && frozenMetadata(firstMeta) !== frozenMetadata(page.meta)) throw new Error('Storage snapshot metadata changed across frozen pages. Retrying a complete snapshot.');
}
/** Read one selected host's complete DiskSummary collection, preserving meta and
 * traversal-start freshness clocks and final arrival clocks. No per-disk
 * requests. Pagination time consumes the frozen snapshot's freshness budget.
 */
export async function readDisks(client, node, now = () => performance.now(), wallNow = () => Date.now()) {
  const started=now(), wallStarted=wallNow(), data=[], seen=new Set(); let cursor=null, meta;
  do {
    const page=await client.get(`/api/v1/nodes/${encodeURIComponent(node.node_id)}/disks`, {limit:500, ...(cursor ? {cursor} : {})}, {releasePrevious:!cursor});
    validateStoragePage(page,node.node_id,meta); meta ||= page.meta; data.push(...page.data); cursor=page.meta.next_cursor;
    if (snapshotExpired(started,wallStarted,now(),wallNow())) throw new Error('Disk summary refresh exceeded 15 seconds. Waiting for a timely refresh.');
    if(cursor && seen.has(cursor)) throw new Error('The server repeated a storage pagination cursor.');
    if(cursor) seen.add(cursor);
  } while(cursor);
  const arrivedAt=now(),wallAt=wallNow();
  return {data,meta,startedAt:started,startedWallAt:wallStarted,arrivedAt,wallAt,at:performance.timeOrigin+arrivedAt};
}
// Use elapsed local clocks, never server_time minus the browser wall clock.
export function storageSnapshotAge(summary,now=performance.now(),wallNow=Date.now()) {
  return Number.isFinite(summary?.startedAt) && Number.isFinite(summary?.startedWallAt)
    ? Math.max(0,now-summary.startedAt,wallNow-summary.startedWallAt) : Infinity;
}
/** One independent frozen inventory traversal with atomic publication.
 * expect(meta) supplies the latest disks stamp. snapshot() exposes only complete
 * rows, plus loadedCount/busy/error while a replacement is loading. Clock/gate
 * injection supports sleep, throttling and core-read priority tests.
 */
export class StorageInventory {
  constructor(client,nodeId,onChange=()=>{},options={}) {
    this.client=client;this.nodeId=nodeId;this.onChange=onChange;
    this.now=options.now || (()=>performance.now());this.wallNow=options.wallNow || (()=>Date.now());
    this.canRead=options.canRead || (()=>true);this.onAuth=options.onAuth || (()=>{});
    this.budget=options.budget || {requests:[]};this.closed=false;this.busy=false;this.serial=0;this.error='';this.retryAt=0;this.retryWallAt=0;
  }
  get requests(){return this.budget.requests;}
  set requests(value){this.budget.requests=value;}
  expect(meta) {
    const stamp=topologyStamp(meta);
    if(stamp !== topologyStamp(this.expected)) {this.serial++;this.pending=null;}
    this.expected=meta;this.notify();
  }
  snapshot() {
    const c=this.cached,p=this.pending;
    return {rows:c?.rows || [],meta:c?.meta,complete:!!c,matching:!!c && topologyStamp(c.meta)===topologyStamp(this.expected),
      loadedCount:p?.rows.length || (c?.rows.length ?? 0),loading:!!p,busy:this.busy,error:this.error,
      loadedAt:c?.wallAt,snapshotStartedAt:c?.startedWallAt,ageMs:c?Math.max(0,this.now()-c.startedAt,this.wallNow()-c.startedWallAt):null,
      retryInMs:Math.max(0,this.retryAt-this.now(),this.retryWallAt-this.wallNow(),this.requests.length>=8 ? this.requests[0]+30000-this.now() : 0)};
  }
  notify(){if(!this.closed)this.onChange(this.snapshot());}
  close(){this.closed=true;this.serial++;this.client.close?.();}
  async refresh(generation,force=false) {
    if(this.closed)return;
    this.generation=generation;
    const s=this.snapshot();
    if(!force && s.matching && s.ageMs<30000 && !this.pending)return;
    if(!this.pending)this.pending={rows:[],meta:null,cursor:null,seen:new Set(),generation,started:this.now(),wallStarted:this.wallNow()};
    await this.continue();
  }
  async continue() {
    if(this.closed || this.busy)return;
    if(!this.pending) {
      if(this.error && this.expected) this.pending={rows:[],meta:null,cursor:null,seen:new Set(),generation:this.generation,started:this.now(),wallStarted:this.wallNow()};
      else return;
    }
    if(Math.max(this.now()-this.pending.started,this.wallNow()-this.pending.wallStarted)>=300000) {
      this.pending=null;this.error='Storage cursor expired after five minutes. Starting a new traversal.';this.notify();return;
    }
    this.requests=this.requests.filter(at=>this.now()-at<30000);
    if(this.now()<this.retryAt || this.wallNow()<this.retryWallAt || this.requests.length>=8 || !this.canRead()) {this.notify();return;}
    this.busy=true;const serial=this.serial,pending=this.pending;this.notify();
    try {
      while(!this.closed && serial===this.serial && this.requests.length<8 && this.canRead()) {
        this.requests.push(this.now());
        const page=await this.client.get(`/api/v1/nodes/${encodeURIComponent(this.nodeId)}/inventory`,{limit:500,generation:pending.generation,...(pending.cursor?{cursor:pending.cursor}:{})});
        if(this.closed || serial!==this.serial)return;
        validateStoragePage(page,this.nodeId,pending.meta);
        if(topologyStamp(page.meta)!==topologyStamp(this.expected))throw new Error('Disk summaries and inventory topology stamps differ. Retaining dated context while retrying.');
        if(Math.max(this.now()-pending.started,this.wallNow()-pending.wallStarted)>=300000)throw Object.assign(new Error('Storage cursor expired during traversal.'),{status:410});
        pending.meta ||= page.meta;pending.rows.push(...page.data);pending.cursor=page.meta.next_cursor;
        if(pending.cursor && pending.seen.has(pending.cursor))throw new Error('The server repeated a storage pagination cursor.');
        if(pending.cursor)pending.seen.add(pending.cursor);
        this.error='';this.notify();
        if(!pending.cursor) {
          const ids=new Set(pending.rows.map(row=>row.object_id));
          if(ids.size!==pending.rows.length)throw new Error('Storage snapshot repeated an object identity.');
          this.cached={rows:pending.rows,meta:pending.meta,startedAt:pending.started,startedWallAt:pending.wallStarted,arrivedAt:this.now(),wallAt:this.wallNow()};this.pending=null;break;
        }
      }
    } catch(error) {
      if(this.closed || serial!==this.serial || error.name==='AbortError')return;
      if([401,403].includes(error.status))this.onAuth(error);
      if(error.status===429) {
        const wait=Math.max(1000,(error.retryAfter||0)*1000);this.retryAt=this.now()+wait;this.retryWallAt=this.wallNow()+wait;
        this.error=`Storage read budget reached. Retrying after ${Math.ceil(wait/1000)} seconds; loaded pages are retained.`;
      } else {this.pending=null;this.error=error.status===410?'Storage cursor expired. Starting a new traversal.':error.message || 'Storage inventory unavailable.';}
    } finally {this.busy=false;this.notify();}
  }
}
