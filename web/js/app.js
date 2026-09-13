import { html, Component, render } from './lib.js';
import { ApiClient } from './api.js';
import { ViewerCredentialStore, validViewerToken, parseRoute, readSnapshot, readDisks, StorageInventory, storageSnapshotAge, topologyStamp, CursorPages, viewCollection, collectionForView, snapshotExpired, MAX_SNAPSHOT_AGE_MS, minimumRefreshDelay } from './session.js';
import { TITLES } from './data.js';
import { clusterVM, appendPollHistory } from './model.js';
import { appendDiskHistory } from './storage.js';
import { SecurityPages } from './security.js';
import { SecurityView } from './security-views.js';
import { OperationsView, AttentionIndicator } from './operator-views.js';
import { rackPage } from './rack.js';
import { loadThree, bootStage } from './stage.js';
import { Sidebar, Topbar, Connection, Overview, Filesystem, Throughput, HostDetail } from './views.js';

const route = () => ({diskId:null, objectId:null, ...parseRoute(location.hash)});
export class App extends Component {
  constructor(props) {
    super(props);
    this.state = { ...route(), snapshot: null, collection: null, security: null, inventory: null, diskSummary: null, diskHistory: {}, rackPageIndex: 0, coreFreshAt: 0, coreWallAt: 0, tokenDraft: '', connected: false, busy: false, error: '', lastSuccess: null, clock: '', glReady: false, glFailed: false, hoverId: null, history: [], nodeHistory: {} };
    this.credentials = new ViewerCredentialStore();
    this.storageBudget={requests:[]}; this.client = null; this.stage = null; this.stageKey = ''; this.stageRef = { current: null }; this.epoch = 0;
  }
  componentDidMount() {
    this.clockTimer = setInterval(() => this.setState({ clock: new Date().toLocaleTimeString() }), 1000);
    this.onRoute = () => { const next=route(); this.setState({...next, rackPageIndex:rackPage(this.state.snapshot?.nodes || [],this.state.rackPageIndex,next.nodeId).pageIndex}, () => { this.ensureViewData(); this.refreshSoon(); }); };
    this.onVisible = () => this.setState({ clock: new Date().toLocaleTimeString() }, () => this.refreshSoon());
    window.addEventListener('hashchange', this.onRoute);
    window.addEventListener('popstate', this.onRoute);
    document.addEventListener('visibilitychange', this.onVisible);
    loadThree().then(ok => { if (!this.disposed) this.setState({ glReady: ok, glFailed: !ok }); });
    const saved = this.credentials.load();
    this.setState({ credentialNotice: this.credentials.notice });
    if (saved) this.startSession(saved, true);
  }
  componentWillUnmount() {
    this.disposed = true; this.epoch++; this.stopViewData(); this.client?.close(); this.client = null; clearInterval(this.clockTimer); clearTimeout(this.pollTimer);
    window.removeEventListener('hashchange', this.onRoute); window.removeEventListener('popstate', this.onRoute);
    document.removeEventListener('visibilitychange', this.onVisible); this.teardownStage();
  }
  componentDidUpdate() {
    const objectKey = this.state.view === 'node' && this.state.objectId ? `${this.state.nodeId}:${this.state.objectId}` : '';
    const target = objectKey && this.state.inventory?.matching ? document.getElementById(`storage-object-${this.state.objectId}`) : null;
    if (target && this.objectAnchorKey !== objectKey) {
      this.objectAnchorKey = objectKey;
      target.scrollIntoView({ block: 'center' });
    } else if (!objectKey) this.objectAnchorKey = '';
    const nodes = clusterVM(this.state.snapshot, this.coreStale()).nodes;
    const page = rackPage(nodes,this.state.rackPageIndex);
    const key = page.nodes.map(n => `${n.id}:${n.state}:${n.kind}:${n.name}`).join('|');
    if (this.state.view !== 'overview' || !this.state.connected || !nodes.length) { this.teardownStage(); return; }
    if (this.stage && this.stageKey !== key) this.teardownStage();
    if (!this.stage && this.state.glReady && !this.state.glFailed && this.stageRef.current) {
      try {
        this.stage = bootStage(this.stageRef.current, { nodes:page.nodes, blueprintGrid: true,
          isActive: () => this.state.view === 'overview', onOpen: id => this.go('node', id),
          onHover: id => { if (id !== this.state.hoverId) this.setState({ hoverId: id }); }
        });
        this.stageKey = key;
      } catch { this.setState({ glFailed: true }); }
    }
  }
  teardownStage() { this.stage?.dispose(); this.stage = null; this.stageKey = ''; }
  go(view, nodeId = this.state.nodeId) {
    history.pushState(null, '', view === 'node' && nodeId ? `#node/${encodeURIComponent(nodeId)}` : `#${view}`);
    this.setState({ view, nodeId, diskId:null, objectId:null, hoverId: null, rackPageIndex:rackPage(this.state.snapshot?.nodes || [],this.state.rackPageIndex,view==='node' || this.state.view==='node' ? nodeId : null).pageIndex }, () => { window.scrollTo(0, 0); this.ensureViewData(); this.refreshSoon(); });
  }
  connect(event) {
    event.preventDefault(); const token = this.state.tokenDraft.trim();
    if (!validViewerToken(token)) { this.setState({ error: 'Use the read-only viewer credential from Cider Server. Administrator and node credentials cannot connect here.' }); return; }
    this.startSession(token);
  }
  startSession(token, remembered = false) {
    this.stopViewData(); this.client?.close(); clearTimeout(this.pollTimer); this.epoch += 1; this.inFlight = false;
    const epoch = this.epoch;
    this.remembered = remembered; this.storageBudget={requests:[]}; this.diskReading = false; this.refreshPending = false;
    this.client = new ApiClient(token); this.failures = 0; this.retryAt = 0;
    this.setState({ tokenDraft: '', connected: true, credentialNotice:this.credentials.notice, attentionSummary:null, snapshot: null, collection: null, security: null, inventory: null, diskSummary: null, diskHistory: {}, error: '', lastSuccess: null, history: [], nodeHistory: {} }, () => { if (epoch === this.epoch) this.poll(); });
  }
  disconnect(message = '') {
    this.epoch += 1; this.stopViewData(); this.client?.close(); this.client = null; this.inFlight = false; clearTimeout(this.pollTimer);
    this.credentials.clear(); this.teardownStage(); this.storageBudget={requests:[]}; this.diskReading = false; this.refreshPending = false; this.retryAt = 0; this.failures = 0;
    this.setState({ connected: false, credentialNotice:this.credentials.notice, attentionSummary:null, snapshot: null, collection: null, security: null, inventory: null, diskSummary: null, diskHistory: {}, tokenDraft: '', nodeId:null, diskId:null, objectId:null, hoverId:null, rackPageIndex:0, coreFreshAt:0, coreWallAt:0, error: message, busy: false, lastSuccess: null, history: [], nodeHistory: {} });
  }
  coreStale() {
    const s = this.state;
    return !s.snapshot || !!s.error || snapshotExpired(s.coreFreshAt, s.coreWallAt);
  }
  stopViewData() {
    this.table?.close(); this.table = null; this.tableKey = '';
    clearTimeout(this.tableTimer);
    this.securityPages?.close(); this.securityPages = null;
    this.storageLoader?.close(); this.storageLoader=null; this.storageKey='';
    this.diskClient?.close(); this.diskClient=null;
    clearTimeout(this.diskTimer); clearTimeout(this.storageTimer);
  }
  ensureViewData() {
    if (!this.client || this.disposed) return;
    const node = this.state.snapshot?.nodes.find(n => n.node_id === this.state.nodeId);
    // Security owns three independent frozen cursor reads. Do not also start the
    // legacy single event table while that view is active.
    const config = this.state.view === 'sec' ? null : viewCollection(this.state.view, node?.node_id);
    const key = config ? JSON.stringify(config) : '';
    if (key !== this.tableKey) {
      this.table?.close(); this.table = null; this.tableKey = key;
      clearTimeout(this.tableTimer);
      this.setState({ collection: null });
      if (config) {
        const table = new CursorPages(new ApiClient(this.client.token), config.path, config.params,
          collection => { if (this.table === table) this.setState({ collection }); },
          () => { if (this.table === table) this.disconnect('Viewer credential expired or was rejected. Copy a current viewer credential from the server.'); },
          { follow: config.path === '/api/v1/filesystems' });
        this.table = table;
        table.refresh().finally(() => {
          if (this.table === table && table.followEnabled) this.tableTimer = setTimeout(() => this.pollTable(table), document.hidden ? 30000 : 3000);
        });
      }
    }
    this.ensureSecurity();
    this.ensureStorage(node);
  }
  async pollTable(table) {
    if (this.table !== table || this.disposed) return;
    if (this.inFlight) { this.tableTimer = setTimeout(() => this.pollTable(table), 250); return; }
    try { await table.follow(); }
    finally {
      if (this.table === table) this.tableTimer = setTimeout(() => this.pollTable(table), Math.max(document.hidden ? 30000 : 3000, minimumRefreshDelay(document.hidden, table.retryAt)));
    }
  }
  ensureSecurity() {
    if (this.state.view !== 'sec') {
      if (this.securityPages) { this.securityPages.close(); this.securityPages = null; this.setState({ security: null }); }
      return;
    }
    if (this.securityPages) return;
    const auth = () => { if (this.securityPages === pages) this.disconnect('Viewer credential expired or was rejected. Copy a current viewer credential from the server.'); };
    const pages = new SecurityPages(this.client.token, security => {
      if (this.securityPages === pages) this.setState({ security });
    }, auth);
    this.securityPages = pages;
    this.setState({ security: pages.snapshot() });
    pages.refreshAll();
  }
  ensureStorage(node) {
    const key=this.state.view==='node' && node ? node.node_id : '';
    if(key!==this.storageKey) {
      this.storageLoader?.close(); this.storageLoader=null;
      this.diskClient?.close(); this.diskClient=null;
      clearTimeout(this.diskTimer);clearTimeout(this.storageTimer);this.storageKey=key;
      this.setState({inventory:null,diskSummary:null});
      if(key) {
        const auth=()=>{if(this.storageLoader===loader)this.disconnect('Viewer credential expired or was rejected. Copy a current viewer credential from the server.');};
        const loader=new StorageInventory(new ApiClient(this.client.token),key,inventory=>{
          if(this.storageLoader!==loader)return;
          this.setState(s=>({inventory:{...inventory,disks:inventory.matching?s.diskSummary?.data || []:s.inventory?.disks || []}}),()=>this.recordDiskHistory());
        },{canRead:()=>!this.inFlight && !this.diskReading,onAuth:auth,budget:this.storageBudget});
        this.storageLoader=loader;this.diskClient=new ApiClient(this.client.token);
        this.diskTimer=setTimeout(()=>this.pollDisks(),0);
        this.storageTimer=setTimeout(()=>this.continueStorage(),1000);
      }
    }
    if(key) {
      if(node.boot_id && this.state.diskSummary?.meta?.boot_id && node.boot_id!==this.state.diskSummary.meta.boot_id) this.setState(s=>({diskSummary:{...s.diskSummary,error:'Host boot changed. Waiting for matching disk summaries.'}}));
      this.recordDiskHistory();
    }
  }
  continueStorage() {
    const loader=this.storageLoader;if(!loader || this.disposed)return;
    loader.continue().finally(()=>{
      if(this.storageLoader===loader)this.storageTimer=setTimeout(()=>this.continueStorage(),Math.max(1000,loader.snapshot().retryInMs));
    });
  }
  recordDiskHistory(failure='') {
    const {diskSummary,inventory,diskId,nodeId}=this.state;
    if(!diskId || !diskSummary || (diskSummary.meta?.node_id && diskSummary.meta.node_id!==nodeId))return;
    const disk=diskSummary.data?.find(d=>d.object_id===diskId),key=`${nodeId}:${diskId}`;
    const error=failure || diskSummary.error || (!inventory?.matching || !topologyStamp(diskSummary.meta) || topologyStamp(diskSummary.meta)!==topologyStamp(inventory.meta) ? 'Topology snapshot unmatched' : inventory?.error) || (this.coreStale() ? 'Core snapshot stale' : '');
    const snapshotAgeMs=storageSnapshotAge(diskSummary),timely=snapshotAgeMs<MAX_SNAPSHOT_AGE_MS;
    this.setState(s=>({diskHistory:{...s.diskHistory,[key]:appendDiskHistory(s.diskHistory[key],{disk,at:error || !timely ? performance.timeOrigin+performance.now() : diskSummary.at,error,timely,snapshotAgeMs})}}));
  }
  async pollDisks() {
    const client=this.diskClient,nodeId=this.storageKey;
    if(!client || !nodeId || this.disposed)return;
    // Core requests always get first access to the shared server read budget.
    if(this.inFlight) {this.diskTimer=setTimeout(()=>this.pollDisks(),250);return;}
    this.diskReading=true;let delay=document.hidden?30000:3000;
    try {
      const summary=await readDisks(client,{node_id:nodeId});
      if(this.diskClient!==client)return;
      const node=this.state.snapshot?.nodes.find(n=>n.node_id===nodeId);
      if(node?.boot_id && summary.meta.boot_id!==node.boot_id)throw new Error('Disk summary belongs to an earlier boot. Waiting for a matching host snapshot.');
      this.setState({diskSummary:summary},()=>{
        const loader=this.storageLoader;
        if(!loader || this.diskClient!==client)return;
        loader.expect(summary.meta);loader.refresh(summary.meta.inventory_generation);
        this.recordDiskHistory();
      });
    } catch(error) {
      if(this.diskClient!==client || error.name==='AbortError')return;
      if([401,403].includes(error.status)){this.disconnect('Viewer credential expired or was rejected. Copy a current viewer credential from the server.');return;}
      delay=Math.max(delay,(error.retryAfter || 0)*1000);
      this.setState(s=>({diskSummary:{...s.diskSummary,error:error.message || 'Disk summaries unavailable.'}}),()=>this.recordDiskHistory(error.message));
    } finally {
      if(this.diskClient===client) {this.diskReading=false;this.diskTimer=setTimeout(()=>this.pollDisks(),delay);}
    }
  }
  refreshSoon() {
    clearTimeout(this.pollTimer);
    if (this.inFlight) { this.refreshPending = true; return; }
    if (this.client) this.pollTimer = setTimeout(() => this.poll(), minimumRefreshDelay(document.hidden, this.retryAt));
  }
  async poll() {
    if (!this.client || this.inFlight) return;
    const client = this.client, epoch = this.epoch;
    this.inFlight = true; this.refreshPending = false; this.setState({ busy: true });
    let delay = document.hidden ? 30000 : 3000;
    // Do not keep retained numeric values looking current during a long page
    // traversal. This marks a gap without starting overlapping requests.
    const freshnessTimer = setTimeout(() => {
      if (epoch !== this.epoch || !this.inFlight) return;
      const at = performance.timeOrigin + performance.now();
      this.setState(s => ({ error: 'Snapshot refresh is delayed beyond 15 seconds.', ...appendPollHistory(s, null, at) }));
    }, MAX_SNAPSHOT_AGE_MS);
    try {
      const snapshot = await readSnapshot(client);
      if (epoch !== this.epoch) return;
      if (!this.remembered) {
        this.credentials.save(client.token); this.remembered = true;
        this.setState({ credentialNotice: this.credentials.notice });
      }
      // Successes and failures share one monotonic browser arrival clock.
      // Server/source dates are provenance, not positions on the session x axis.
      const at = performance.timeOrigin + performance.now();
      this.failures = 0; this.retryAt = 0;
      this.setState(s => ({ snapshot,
        error: '', coreFreshAt: performance.now(), coreWallAt: Date.now(), lastSuccess: new Date().toISOString(), rackPageIndex:rackPage(snapshot.nodes,s.rackPageIndex).pageIndex, nodeId: s.nodeId || snapshot.nodes[0]?.node_id || null,
        ...appendPollHistory(s, snapshot, at) }), () => { if (epoch === this.epoch) this.ensureViewData(); });
    } catch (error) {
      if (epoch !== this.epoch || error.name === 'AbortError') return;
      if ([401,403].includes(error.status)) { this.disconnect('Viewer credential expired or was rejected. Copy a current viewer credential from the server.'); return; }
      this.failures = (this.failures || 0) + 1;
      delay = Math.max(error.retryAfter * 1000 || 0, Math.min(30000, 5000 * 2 ** Math.min(this.failures, 3)) + Math.random() * 500);
      this.retryAt = performance.now() + delay;
      const at = performance.timeOrigin + performance.now();
      this.setState(s => ({ error: error.message || 'The server is unreachable.', ...appendPollHistory(s, null, at) }));
    } finally {
      clearTimeout(freshnessTimer);
      if (epoch === this.epoch) {
        this.inFlight = false; this.setState({ busy: false });
        if (this.client) this.pollTimer = setTimeout(() => this.poll(), Math.max(this.refreshPending ? 0 : delay, minimumRefreshDelay(document.hidden, this.retryAt)));
      }
    }
  }
  render() {
    const s = this.state, stale = this.coreStale(), epoch = this.epoch;
    const selectedNode = s.snapshot?.nodes.find(node => node.node_id === s.nodeId);
    const collection = collectionForView(s.collection, s.view, selectedNode?.node_id);
    const inventory = this.storageKey===selectedNode?.node_id ? s.inventory : null;
    const rack=rackPage(clusterVM(s.snapshot,stale).nodes,s.rackPageIndex);
    const vm = clusterVM(s.snapshot ? { ...s.snapshot,
      filesystems: s.view === 'fs' ? collection?.rows || [] : [],
      events: s.view !== 'fs' ? collection?.rows || [] : [], inventory: inventory?.rows || []
    } : null, stale);
    const paging = { ...(collection || { rows: [], busy: true }),
      previous: () => this.table?.previous(), next: () => this.table?.next(), refresh: () => this.table?.refresh() };
    const selected = vm.nodes.find(n => n.id === s.nodeId);
    const actions = { open: id => this.go('node',id), stageRef: this.stageRef, noGl: !s.glReady || s.glFailed, hover: vm.nodes.find(n => n.id === s.hoverId), history: s.history, paging, rack:{...rack,previous:()=>this.setState({rackPageIndex:Math.max(0,rack.pageIndex-1),hoverId:null}),next:()=>this.setState({rackPageIndex:Math.min(rack.pageCount-1,rack.pageIndex+1),hoverId:null})} };
    const content = !s.connected ? html`<${Connection} token=${s.tokenDraft} onInput=${value => this.setState({ tokenDraft: value })} onSubmit=${e => this.connect(e)} error=${s.error}/>`
      : !s.snapshot ? html`<div class="panel panel-pad" role="status">${s.error || 'Loading the server snapshot...'}</div>`
      : s.view === 'overview' ? html`<${Overview} vm=${vm} a=${actions}/>`
      : s.view === 'fs' ? html`<${Filesystem} vm=${vm} paging=${paging}/>`
      : s.view === 'io' ? html`<${Throughput} vm=${vm} history=${s.history}/>`
      : s.view === 'sec' ? html`<${SecurityView} security=${s.security || {}} nodes=${vm.nodes} now=${performance.now()} wallNow=${Date.now()}/>`
      : s.view === 'ops' ? html`<${OperationsView} key=${this.epoch} client=${this.client} route=${{section:s.operation || 'attention',objectId:s.objectId}} objectId=${s.objectId} attentionSummary=${s.attentionSummary}/>`
      : html`<${HostDetail} vm=${vm} node=${selected} storage=${{nodeId:selected?.id,summary:s.diskSummary,inventory,diskId:s.diskId,history:s.diskHistory[`${s.nodeId}:${s.diskId}`] || [],snapshotAgeMs:storageSnapshotAge(s.diskSummary),refresh:()=>this.storageLoader?.refresh(s.diskSummary?.meta?.inventory_generation,true),current:!stale && !!inventory?.matching && topologyStamp(inventory?.meta)===topologyStamp(s.diskSummary?.meta) && (!selectedNode?.boot_id || selectedNode.boot_id===s.diskSummary?.meta?.boot_id) && !inventory?.error && !!s.diskSummary && !s.diskSummary.error && storageSnapshotAge(s.diskSummary)<MAX_SNAPSHOT_AGE_MS}} paging=${paging} history=${s.nodeHistory[selected?.id] || []} open=${actions.open}/>`;
    return html`<div class="shell">
      <${Sidebar} view=${s.view} nodeCount=${vm.nodes.length} go=${view => this.go(view)}/>
      <main class="main"><${Topbar} title=${TITLES[s.view]} vm=${vm} clock=${s.clock} connected=${s.connected} disconnect=${() => this.disconnect()}/>
        <div class="content">
          ${s.credentialNotice ? html`<p class="note" role="status">${s.credentialNotice}</p>` : null}
          ${s.connected ? html`<div class=${'connection-status ' + (stale ? 'is-error' : '')} role="status"><span>${s.error ? `Core updates unavailable: ${s.error} Current summary values are hidden. Reconnecting automatically.` : stale && s.snapshot ? 'Core snapshot is older than 15 seconds. Current summary values are hidden while refreshing.' : `${s.busy ? 'Refreshing' : 'Connected'} to ${location.origin}. Live summaries poll every ${document.hidden ? 30 : 3} seconds; collector heartbeats remain every 5 seconds. Tables have separate snapshot controls.`}</span><span>Last success: ${s.lastSuccess ? new Date(s.lastSuccess).toLocaleTimeString() : 'none'}</span></div>` : null}
          ${s.connected ? html`<${AttentionIndicator} key=${this.epoch} client=${this.client} onAuth=${()=>{if(epoch===this.epoch)this.disconnect('Viewer credential expired or was rejected.');}} onSummary=${attentionSummary=>{if(epoch===this.epoch)this.setState({attentionSummary});}}/>` : null}
          ${content}
        </div>
      </main>
    </div>`;
  }
}
const appRoot = typeof document !== 'undefined' && document.getElementById('app');
if (appRoot) render(html`<${App}/>`, appRoot);
