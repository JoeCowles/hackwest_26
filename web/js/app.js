import { html, Component, render } from './lib.js';
import { ApiClient } from './api.js';
import { TITLES } from './data.js';
import { clusterVM, numeric } from './model.js';
import { loadThree, bootStage } from './stage.js';
import { Sidebar, Topbar, Connection, Overview, Filesystem, Throughput, Access, Alerting, HostDetail } from './views.js';

const VIEWS = ['overview','fs','io','sec','alerts','node'];
function route() {
  const [view, nodeId] = location.hash.replace(/^#\/?/, '').split('/');
  return { view: VIEWS.includes(view) ? view : 'overview', nodeId: nodeId || null };
}
class App extends Component {
  constructor(props) {
    super(props);
    this.state = { ...route(), snapshot: null, tokenDraft: '', connected: false, busy: false, error: '', lastSuccess: null, clock: '', glReady: false, glFailed: false, hoverId: null, history: [], nodeHistory: {} };
    this.client = null; this.stage = null; this.stageKey = ''; this.stageRef = { current: null }; this.epoch = 0;
  }
  componentDidMount() {
    this.clockTimer = setInterval(() => this.setState({ clock: new Date().toLocaleTimeString() }), 1000);
    this.onRoute = () => { this.setState(route(), () => this.refreshSoon()); };
    this.onVisible = () => this.refreshSoon();
    window.addEventListener('hashchange', this.onRoute);
    window.addEventListener('popstate', this.onRoute);
    document.addEventListener('visibilitychange', this.onVisible);
    loadThree().then(ok => { if (!this.disposed) this.setState({ glReady: ok, glFailed: !ok }); });
  }
  componentWillUnmount() {
    this.disposed = true; this.client?.close(); clearInterval(this.clockTimer); clearTimeout(this.pollTimer);
    window.removeEventListener('hashchange', this.onRoute); window.removeEventListener('popstate', this.onRoute);
    document.removeEventListener('visibilitychange', this.onVisible); this.teardownStage();
  }
  componentDidUpdate() {
    const nodes = clusterVM(this.state.snapshot, !!this.state.error).nodes;
    const key = nodes.slice(0,24).map(n => `${n.id}:${n.state}:${n.kind}:${n.name}`).join('|');
    if (this.state.view !== 'overview' || !this.state.connected || !nodes.length) { this.teardownStage(); return; }
    if (this.stage && this.stageKey !== key) this.teardownStage();
    if (!this.stage && this.state.glReady && !this.state.glFailed && this.stageRef.current) {
      try {
        this.stage = bootStage(this.stageRef.current, { nodes, blueprintGrid: true,
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
    this.setState({ view, nodeId, hoverId: null }, () => this.refreshSoon());
  }
  connect(event) {
    event.preventDefault(); const token = this.state.tokenDraft.trim();
    if (!token.startsWith('viewer_')) { this.setState({ error: 'Use the read-only viewer credential from Orchard Server, not an administrator or node credential.' }); return; }
    this.client?.close(); clearTimeout(this.pollTimer); this.epoch += 1; this.inFlight = false;
    this.client = new ApiClient(token); this.failures = 0;
    this.setState({ tokenDraft: '', connected: true, snapshot: null, error: '', lastSuccess: null, history: [], nodeHistory: {} }, () => this.poll());
  }
  disconnect(message = '') {
    this.epoch += 1; this.client?.close(); this.client = null; this.inFlight = false; clearTimeout(this.pollTimer);
    this.setState({ connected: false, snapshot: null, tokenDraft: '', error: message, busy: false, lastSuccess: null, history: [], nodeHistory: {} });
  }
  refreshSoon() {
    clearTimeout(this.pollTimer);
    if (this.inFlight) { this.refreshPending = true; return; }
    if (this.client) this.pollTimer = setTimeout(() => this.poll(), document.hidden ? 30000 : 0);
  }
  async poll() {
    if (!this.client || this.inFlight) return;
    const client = this.client, epoch = this.epoch, requestedView = this.state.view, requestedNode = this.state.nodeId;
    this.inFlight = true; this.refreshPending = false; this.setState({ busy: true });
    let delay = document.hidden ? 30000 : 5000;
    try {
      const [cluster, nodes, filesystems, events] = await Promise.all([
        client.get('/api/v1/cluster'), client.all('/api/v1/nodes'), client.all('/api/v1/filesystems'), client.all('/api/v1/events')
      ]);
      const selected = nodes.find(n => n.node_id === requestedNode) || (!requestedNode ? nodes[0] : null);
      const inventory = requestedView === 'node' && selected ? await client.all(`/api/v1/nodes/${encodeURIComponent(selected.node_id)}/inventory`) : [];
      if (epoch !== this.epoch) return;
      const at = Date.parse(cluster.meta.server_time);
      const point = { at, read: numeric(cluster.data.throughput.read_bytes_per_second), write: numeric(cluster.data.throughput.write_bytes_per_second) };
      const nodeHistory = { ...this.state.nodeHistory };
      for (const node of nodes) nodeHistory[node.node_id] = [...(nodeHistory[node.node_id] || []), { at, read: numeric(node.read_bytes_per_second), write: numeric(node.write_bytes_per_second) }].slice(-180);
      this.failures = 0;
      this.setState(s => ({ snapshot: { cluster: cluster.data, nodes, filesystems, events, inventory, inventoryNode: selected?.node_id },
        error: '', lastSuccess: new Date().toISOString(), nodeId: s.nodeId || selected?.node_id || null,
        history: [...s.history, point].slice(-180), nodeHistory }));
    } catch (error) {
      if (epoch !== this.epoch || error.name === 'AbortError') return;
      if ([401,403].includes(error.status)) { this.disconnect('Viewer credential expired or was rejected. Copy a current viewer credential from the server.'); return; }
      this.failures = (this.failures || 0) + 1;
      delay = Math.max(error.retryAfter * 1000 || 0, Math.min(30000, 5000 * 2 ** Math.min(this.failures, 3)) + Math.random() * 500);
      this.setState(s => ({ error: error.message || 'The server is unreachable.', history: [...s.history, { at: Date.now(), read: null, write: null }].slice(-180) }));
    } finally {
      if (epoch === this.epoch) {
        this.inFlight = false; this.setState({ busy: false });
        if (this.client) this.pollTimer = setTimeout(() => this.poll(), this.refreshPending ? 0 : Math.max(delay, document.hidden ? 30000 : 0));
      }
    }
  }
  render() {
    const s = this.state, vm = clusterVM(s.snapshot, !!s.error);
    const selected = vm.nodes.find(n => n.id === s.nodeId);
    const actions = { open: id => this.go('node',id), stageRef: this.stageRef, noGl: !s.glReady || s.glFailed, hover: vm.nodes.find(n => n.id === s.hoverId), history: s.history };
    const content = !s.connected ? html`<${Connection} token=${s.tokenDraft} onInput=${value => this.setState({ tokenDraft: value })} onSubmit=${e => this.connect(e)} error=${s.error}/>`
      : !s.snapshot ? html`<div class="panel panel-pad" role="status">${s.error || 'Loading the server snapshot...'}</div>`
      : s.view === 'overview' ? html`<${Overview} vm=${vm} a=${actions}/>`
      : s.view === 'fs' ? html`<${Filesystem} vm=${vm}/>`
      : s.view === 'io' ? html`<${Throughput} vm=${vm} history=${s.history}/>`
      : s.view === 'sec' ? html`<${Access} vm=${vm}/>`
      : s.view === 'alerts' ? html`<${Alerting}/>`
      : html`<${HostDetail} vm=${vm} node=${selected} inventoryReady=${s.snapshot.inventoryNode === selected?.id && s.snapshot.inventory.length > 0} history=${s.nodeHistory[selected?.id] || []} open=${actions.open}/>`;
    return html`<div class="shell">
      <${Sidebar} view=${s.view} nodeCount=${vm.nodes.length} go=${view => this.go(view)}/>
      <main class="main"><${Topbar} title=${TITLES[s.view]} vm=${vm} clock=${s.clock} connected=${s.connected} disconnect=${() => this.disconnect()}/>
        <div class="content">
          ${s.connected ? html`<div class=${'connection-status ' + (s.error ? 'is-error' : '')} role="status"><span>${s.error ? `Updates unavailable: ${s.error} Current values are hidden.` : `${s.busy ? 'Refreshing' : 'Connected'} to ${location.origin}. Polling every ${document.hidden ? 30 : 5} seconds.`}</span><span>Last success: ${s.lastSuccess ? new Date(s.lastSuccess).toLocaleTimeString() : 'none'}</span></div>` : null}
          ${content}
        </div>
      </main>
    </div>`;
  }
}
render(html`<${App}/>`, document.getElementById('app'));
