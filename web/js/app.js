// Orchard Cluster Console — app shell, state and routing.
import { html, Component, render } from './lib.js';
import { NODES, TITLES, NOTIF_LOG } from './data.js';
import { clusterVM, selVM } from './model.js';
import { loadThree, bootStage } from './stage.js';
import { Sidebar, Topbar, PageModal, Overview, Filesystem, Throughput, Access, Alerting, HostDetail } from './views.js';

const VIEWS = ['overview', 'fs', 'io', 'sec', 'alerts', 'node'];
const LIVE_DATA = true;        // jitter read/write every 2.5s
const BLUEPRINT_GRID = true;   // grid helper on the 3D shelf floor

function readHash() {
  const [view, nodeId] = location.hash.replace(/^#\/?/, '').split('/');
  return { view: VIEWS.includes(view) ? view : 'overview', nodeId: NODES.some(n => n.id === nodeId) ? nodeId : null };
}

class App extends Component {
  constructor(props) {
    super(props);
    const h = readHash();
    this.state = {
      view: h.view, nodeId: h.nodeId || 'pippin', hoverId: null, hoverXY: [0, 0], t: 0, clock: '',
      smsOpen: false, mute: '1h', mwHosts: 'pippin, empire', mwReason: '', windowActive: false,
      notifLog: NOTIF_LOG.slice(), noGl: true, glReady: false, glFailed: false
    };
    this.stageRef = { current: null };
    this.stage = null;
  }

  componentDidMount() {
    this.tickClock();
    this.clockTimer = setInterval(() => this.tickClock(), 1000);
    if (LIVE_DATA) this.liveTimer = setInterval(() => this.setState(s => ({ t: s.t + 1 })), 2500);
    this.onHash = () => { const h = readHash(); this.setState(h.nodeId ? { view: h.view, nodeId: h.nodeId } : { view: h.view }); };
    window.addEventListener('hashchange', this.onHash);
    this.onKey = (e) => { if (e.key === 'Escape' && this.state.smsOpen) this.setState({ smsOpen: false }); };
    window.addEventListener('keydown', this.onKey);
    loadThree().then(ok => this.setState({ noGl: !ok, glReady: ok, glFailed: !ok }));
  }

  componentWillUnmount() {
    clearInterval(this.clockTimer); clearInterval(this.liveTimer);
    window.removeEventListener('hashchange', this.onHash);
    window.removeEventListener('keydown', this.onKey);
    this.teardownStage();
  }

  componentDidUpdate() {
    if (this.state.view !== 'overview') { this.teardownStage(); return; }
    if (!this.state.glReady || this.stage) return;
    const host = this.stageRef.current;
    if (host) this.bootStage(host);
  }

  bootStage(host) {
    this.stage = bootStage(host, {
      blueprintGrid: BLUEPRINT_GRID,
      isActive: () => this.state.view === 'overview',
      onHover: (id, xy) => {
        if (id !== this.state.hoverId) this.setState({ hoverId: id, hoverXY: xy });
        else if (id) this.setState({ hoverXY: xy });
      },
      onOpen: (id) => this.open(id)
    });
  }
  teardownStage() { if (this.stage) { this.stage.dispose(); this.stage = null; } }

  tickClock() {
    const d = new Date(), p = (n) => String(n).padStart(2, '0');
    this.setState({ clock: `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}` });
  }

  go(view) {
    const hash = view === 'node' ? `#node/${this.state.nodeId}` : `#${view}`;
    if (location.hash !== hash) history.pushState(null, '', hash);
    this.setState({ view, hoverId: null });
  }
  open(id) {
    const hash = `#node/${id}`;
    if (location.hash !== hash) history.pushState(null, '', hash);
    this.setState({ view: 'node', nodeId: id, hoverId: null });
  }

  sendPage() {
    const d = new Date(), p = (n) => String(n).padStart(2, '0');
    const entry = { time: `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`, host: 'pippin', msg: 'CRIT manual page · disk wear 94%, read −78% vs baseline', to: 'D. Ramos', st: 'DELIVERED' };
    this.setState(s => ({ smsOpen: false, notifLog: [entry, ...s.notifLog] }));
  }

  render() {
    const s = this.state;
    const vm = clusterVM(s.t);
    const sel = selVM(vm.nodes.find(n => n.id === s.nodeId) || vm.nodes[0]);
    const hovered = vm.nodes.find(n => n.id === s.hoverId);
    const [vw] = this.stage ? this.stage.size() : [640];
    const [title, sub] = s.view === 'node' ? ['Host detail', `${sel.name} · ${sel.ip}`] : TITLES[s.view];

    const a = {
      go: (v) => this.go(v), open: (id) => this.open(id),
      stageRef: this.stageRef, noGl: s.noGl,
      glTitle: s.glFailed ? '3D stage unavailable' : 'Initialising 3D stage',
      glNote: s.glFailed ? 'The renderer could not load. Every host metric below is live and unaffected.' : 'Loading the renderer — host vitals below are already live.',
      hover: hovered ? Object.assign({}, hovered, {
        vitals: [
          { k: 'CAPACITY', v: hovered.usedPct + '%' }, { k: 'FREE', v: hovered.freeLabel },
          { k: 'READ', v: hovered.read + ' MB/s' }, { k: 'WRITE', v: hovered.write + ' MB/s' },
          { k: 'TEMP', v: hovered.temp + ' °C' }, { k: 'CPU', v: hovered.cpu + '%' }
        ]
      }) : null,
      hoverStyle: { left: Math.max(10, Math.min(s.hoverXY[0] + 18, vw - 268)) + 'px', top: Math.max(12, s.hoverXY[1] - 60) + 'px' },
      mute: s.mute, setMute: (m) => this.setState({ mute: m }),
      mwHosts: s.mwHosts, mwReason: s.mwReason, setField: (k, v) => this.setState({ [k]: v }),
      windowActive: s.windowActive, startWindow: () => this.setState(st => ({ windowActive: !st.windowActive })),
      notifLog: s.notifLog
    };

    const view = s.view === 'overview' ? html`<${Overview} vm=${vm} a=${a}/>`
      : s.view === 'fs' ? html`<${Filesystem} vm=${vm}/>`
      : s.view === 'io' ? html`<${Throughput} vm=${vm}/>`
      : s.view === 'sec' ? html`<${Access}/>`
      : s.view === 'alerts' ? html`<${Alerting} a=${a}/>`
      : html`<${HostDetail} vm=${vm} sel=${sel} a=${a}/>`;

    return html`
      <div class="shell">
        <${Sidebar} view=${s.view} nodeCount=${NODES.length} go=${a.go}/>
        <main class="main">
          <${Topbar} title=${title} sub=${sub} vm=${vm} clock=${s.clock} onPage=${() => this.setState({ smsOpen: true })}/>
          <div class="content">${view}</div>
        </main>
        ${s.smsOpen ? html`<${PageModal} onClose=${() => this.setState({ smsOpen: false })} onSend=${() => this.sendPage()}/>` : null}
      </div>`;
  }
}

render(html`<${App}/>`, document.getElementById('app'));
