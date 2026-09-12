// Presentational views. Each takes the cluster view-model (`vm`) and the app
// actions/state bag (`a`) and returns markup. No state lives here.
import { html } from './lib.js';
import { OK, WARN, CRIT, COLS, OPEN_ALERTS, SEC_EVENTS, SEC_KPIS, SEC_STATS, BLOCKED, VOLUMES, SMS_PREVIEW, SMS_VARS, RULES, ROSTER, THRESHOLDS, MUTE_OPTS, NOTIF_LOG, NAV } from './data.js';
import { sevColor, actColor, heatRows } from './model.js';

const HEAT = heatRows();

const corners = (all) => html`<i class="corner tl"/>${all ? html`<i class="corner tr"/><i class="corner bl"/>` : null}<i class="corner br"/>`;
const Tag = ({ col, sm, children }) => html`<span class=${'tag' + (sm ? ' sm' : '')} style=${{ color: col }}>${children}</span>`;
const Bar = ({ h, style }) => html`<span class="bar" style=${{ height: h }}><i style=${style}/></span>`;
const GridHead = ({ cols, tpl }) => html`<div class="grid-head" style=${{ gridTemplateColumns: tpl }}>${cols.map(c => html`<span key=${c}>${c}</span>`)}</div>`;
const Head = ({ title, sm, children }) => html`<div class="panel-head"><h3 class=${sm ? 'sm' : ''}>${title}</h3>${children}</div>`;
const Lines = ({ view, w, h, read, write, sw, grid }) => html`
  <svg viewBox=${view} preserveAspectRatio="none" style=${{ width: '100%', height: h, display: 'block' }}>
    ${(grid || []).map(y => html`<line key=${y} x1="0" y1=${y} x2=${w} y2=${y} stroke="rgba(29,31,32,.1)" stroke-width="1"/>`)}
    <polyline points=${read} fill="none" stroke=${OK} stroke-width=${sw}/>
    <polyline points=${write} fill="none" stroke="#b7b7ba" stroke-width=${sw}/>
  </svg>`;

/* ── shell ─────────────────────────────────────────────────── */
export const Sidebar = ({ view, nodeCount, go }) => html`
  <aside class="sidebar">
    <div style=${{ padding: '0 18px' }}>
      <div class="brand"><span class="mark"/><span class="word">ORCHARD</span></div>
      <div class="kicker" style=${{ fontSize: '10px', marginTop: '5px' }}>Cluster console v0.9</div>
    </div>
    <nav style=${{ display: 'flex', flexDirection: 'column' }}>
      ${NAV.map(([n, label, id, dot]) => html`
        <button key=${id} class=${'nav-btn' + (view === id ? ' active' : '')} onClick=${() => go(id)}>
          <span class="n">${n}</span><span class="label">${label}</span>
          <span class="dot" style=${{ display: dot ? 'block' : 'none', background: dot || 'transparent' }}/>
        </button>`)}
    </nav>
    <div class="foot" style=${{ marginTop: 'auto', padding: '0 18px', display: 'flex', flexDirection: 'column', gap: '12px' }}>
      <div class="panel" style=${{ padding: '10px 11px' }}>
        <div class="eyebrow" style=${{ letterSpacing: '.14em' }}>On call</div>
        <div class="hd" style=${{ fontSize: '17px', marginTop: '2px' }}>D. Ramos</div>
        <div class="note-11" style=${{ lineHeight: 1.4, color: 'rgba(29,31,32,.55)' }}>+1 415 ••• 2207</div>
        ${corners(false)}
      </div>
      <div class="note" style=${{ lineHeight: 1.5, color: 'rgba(29,31,32,.4)' }}>agent 1.4.2 · poll 10s<br/>${nodeCount} hosts enrolled</div>
    </div>
  </aside>`;

export const Topbar = ({ title, sub, vm, clock, onPage }) => html`
  <header class="topbar">
    <div>
      <h1>${title}</h1>
      <div class="note-11" style=${{ lineHeight: 1.4, letterSpacing: '.08em', textTransform: 'uppercase', color: 'rgba(29,31,32,.5)' }}>${sub}</div>
    </div>
    <div class="counts">
      <div><div class="k">HEALTHY</div><div class="v">${vm.healthyCount}</div></div>
      <div><div class="k">DEGRADED</div><div class="v" style=${{ color: WARN }}>${vm.degradedCount}</div></div>
      <div><div class="k">OFFLINE</div><div class="v" style=${{ color: CRIT }}>${vm.offlineCount}</div></div>
    </div>
    <div style=${{ display: 'flex', alignItems: 'center', gap: '8px', paddingLeft: '4px' }}>
      <span style=${{ width: '7px', height: '7px', background: OK, display: 'block', animation: 'om-pulse 2.4s ease-in-out infinite' }}/>
      <span style=${{ font: '500 12px/1 ui-monospace,Menlo,monospace', letterSpacing: '.06em' }}>${clock}</span>
    </div>
    <button class="btn btn-primary" onClick=${onPage}>Page on-call</button>
  </header>`;

export const PageModal = ({ onClose, onSend }) => html`
  <div class="backdrop" onClick=${(e) => { if (e.target === e.currentTarget) onClose(); }}>
    <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="page-title">
      <h3 id="page-title">Page the on-call admin</h3>
      <p style=${{ fontSize: '13.5px', lineHeight: 1.5, color: 'rgba(29,31,32,.75)', margin: '8px 0 0' }}>Sends now to <strong>D. Ramos · +1 415 ••• 2207</strong> and logs the page. Secondary (M. Osei) is paged if unacknowledged in 5 minutes.</p>
      <div style=${{ border: '1px solid rgba(29,31,32,.2)', background: '#e9e9ea', padding: '10px 11px', marginTop: '14px', fontSize: '12.5px', lineHeight: 1.45 }}>ORCHARD CRIT · pippin · disk wear 94%, read 210 MB/s (−78% vs baseline) · 09:41 · ack: orchard.local/a/8841</div>
      <div style=${{ display: 'flex', justifyContent: 'flex-end', gap: '8px', marginTop: '18px' }}>
        <button class="btn" onClick=${onClose}>Cancel</button>
        <button class="btn btn-primary" onClick=${onSend}>Send page</button>
      </div>
      ${corners(false)}
    </div>
  </div>`;

/* ── overview ───────────────────────────────────────────────── */
const HOST_TPL = '1.1fr 1.5fr .9fr 2fr .8fr .8fr .6fr .7fr .9fr';

export const Overview = ({ vm, a }) => html`
  <div class="stack">
    <div style=${{ display: 'flex', flexWrap: 'wrap', gap: '20px', alignItems: 'stretch' }}>

      <div class="stage">
        <div class="stage-host" ref=${a.stageRef}/>
        ${a.noGl ? html`
          <div class="stage-empty">
            <span style=${{ width: '15px', height: '15px', background: OK, display: 'block' }}/>
            <span class="hd" style=${{ fontSize: '19px', letterSpacing: '.04em', textTransform: 'uppercase' }}>${a.glTitle}</span>
            <span class="note-11" style=${{ lineHeight: 1.6, color: 'rgba(29,31,32,.55)', maxWidth: '330px' }}>${a.glNote}</span>
          </div>` : null}
        <div class="stage-chrome" style=${{ top: 0, flexWrap: 'wrap', alignItems: 'flex-start', justifyContent: 'space-between', gap: '8px 18px' }}>
          <div style=${{ display: 'flex', flexDirection: 'column', gap: '3px' }}>
            <div class="eyebrow" style=${{ lineHeight: 1.4, letterSpacing: '.16em' }}>Rack elevation · live</div>
            <div class="hd" style=${{ fontSize: '15px', letterSpacing: '.04em', textTransform: 'uppercase', whiteSpace: 'nowrap' }}>Shelf A · Studio closet</div>
          </div>
          <div style=${{ display: 'flex', flexWrap: 'wrap', gap: '6px 14px', justifyContent: 'flex-end' }}>
            <div class="legend"><i style=${{ background: OK }}/><span>HEALTHY</span></div>
            <div class="legend"><i style=${{ background: WARN }}/><span>DEGRADED</span></div>
            <div class="legend"><i style=${{ background: CRIT }}/><span>OFFLINE</span></div>
          </div>
        </div>
        <div class="stage-chrome" style=${{ bottom: 0, justifyContent: 'space-between', alignItems: 'flex-end' }}>
          <div class="note" style=${{ lineHeight: 1.5 }}>drag to orbit · hover a host for vitals · click to open</div>
          <div class="note" style=${{ lineHeight: 1.5, textAlign: 'right' }}>4 × iMac · 3 × MacBook<br/>thunderbolt mesh · 10GbE uplink</div>
        </div>
        ${a.hover ? html`
          <div class="hover-card" style=${a.hoverStyle}>
            <div style=${{ display: 'flex', alignItems: 'baseline', gap: '8px' }}>
              <span class="hd" style=${{ fontSize: '19px', letterSpacing: '.02em', textTransform: 'uppercase' }}>${a.hover.name}</span>
              <${Tag} col=${a.hover.col}>${a.hover.state}<//>
            </div>
            <div class="note" style=${{ lineHeight: 1.5, color: 'rgba(29,31,32,.55)', marginBottom: '8px' }}>${a.hover.model} · ${a.hover.ip}</div>
            <div style=${{ display: 'grid', gridTemplateColumns: 'repeat(2,1fr)', gap: '7px 14px' }}>
              ${a.hover.vitals.map(v => html`<div key=${v.k}><div class="eyebrow" style=${{ letterSpacing: '.1em' }}>${v.k}</div><div class="hd" style=${{ fontSize: '16px' }}>${v.v}</div></div>`)}
            </div>
          </div>` : null}
      </div>

      <div style=${{ display: 'flex', flexDirection: 'column', gap: '14px', flex: '1 1 300px', minWidth: 0 }}>
        <div class="panel panel-pad">
          <div class="eyebrow-accent">Filesystem · orchardfs</div>
          <div style=${{ display: 'flex', alignItems: 'flex-end', gap: '8px', marginTop: '6px' }}>
            <span class="hd" style=${{ fontSize: '44px', lineHeight: .92 }}>${vm.freeTB}</span>
            <span class="hd" style=${{ fontSize: '17px', paddingBottom: '6px', color: 'rgba(29,31,32,.55)' }}>TB free of ${vm.capTB}</span>
          </div>
          <div style=${{ display: 'flex', height: '10px', marginTop: '12px', border: '1px solid rgba(29,31,32,.16)' }}>
            ${vm.nodes.map(n => html`<span key=${n.id} style=${n.poolStyle} title=${n.name + ' · ' + n.capLabel}/>`)}
          </div>
          <div class="note" style=${{ display: 'flex', justifyContent: 'space-between', marginTop: '6px' }}><span>${vm.usedPct}% used</span><span>+412 GB / 24h</span></div>
          ${corners(true)}
        </div>

        <div class="panel panel-pad">
          <div class="eyebrow-accent">Aggregate I/O</div>
          <div style=${{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '10px', marginTop: '8px' }}>
            <div><div class="eyebrow" style=${{ letterSpacing: '.1em' }}>READ</div><div class="hd" style=${{ fontSize: '27px', lineHeight: 1.05 }}>${vm.totalRead}<span style=${{ fontSize: '13px', color: 'rgba(29,31,32,.55)' }}> MB/s</span></div></div>
            <div><div class="eyebrow" style=${{ letterSpacing: '.1em' }}>WRITE</div><div class="hd" style=${{ fontSize: '27px', lineHeight: 1.05 }}>${vm.totalWrite}<span style=${{ fontSize: '13px', color: 'rgba(29,31,32,.55)' }}> MB/s</span></div></div>
          </div>
          <div style=${{ marginTop: '10px' }}><${Lines} view="0 0 280 56" w="280" h="56px" read=${vm.aggReadPts} write=${vm.aggWritePts} sw="1.5"/></div>
          <div class="note">last 60 min · iostat sample</div>
        </div>

        <div class="panel panel-pad">
          <div style=${{ display: 'flex', justifyContent: 'space-between', alignItems: 'baseline' }}>
            <div class="eyebrow-accent">Thermal</div><div class="mono-10">limit 85°C</div>
          </div>
          <div style=${{ display: 'flex', flexDirection: 'column', gap: '6px', marginTop: '10px' }}>
            ${vm.nodes.map(n => html`
              <div key=${n.id} style=${{ display: 'grid', gridTemplateColumns: '64px minmax(0,1fr) 34px', alignItems: 'center', gap: '8px' }}>
                <span class="mono-10" style=${{ color: 'rgba(29,31,32,.6)' }}>${n.name}</span>
                <${Bar} h="5px" style=${n.tempBarStyle}/>
                <span style=${{ font: '500 10px/1 ui-monospace,Menlo,monospace', textAlign: 'right' }}>${n.temp}°</span>
              </div>`)}
          </div>
        </div>
      </div>
    </div>

    <div class="panel">
      <div class="panel-head" style=${{ justifyContent: 'flex-start' }}><h3>Hosts</h3><span class="note">click a row for the deep dive</span></div>
      <div class="table-scroll"><div style=${{ minWidth: '820px' }}>
        <${GridHead} cols=${COLS.host} tpl=${HOST_TPL}/>
        ${vm.nodes.map(n => html`
          <div key=${n.id} class="grid-row host-row" style=${{ gridTemplateColumns: HOST_TPL }} onClick=${() => a.open(n.id)} role="button" tabindex="0" onKeyDown=${(e) => { if (e.key === 'Enter') a.open(n.id); }}>
            <span class="host-name" style=${{ fontSize: '16px' }}>${n.name}</span>
            <span class="dim">${n.model}</span>
            <${Tag} col=${n.col}>${n.state}<//>
            <span style=${{ display: 'flex', alignItems: 'center', gap: '9px' }}>
              <${Bar} style=${Object.assign({}, n.capBarStyle)}/>
              <span class="mono-10" style=${{ color: 'rgba(29,31,32,.6)', width: '58px', flex: 'none', whiteSpace: 'nowrap' }}>${n.capLabel}</span>
            </span>
            <span class="num">${n.read}</span>
            <span class="num">${n.write}</span>
            <span style=${n.tempTextStyle}>${n.temp}°</span>
            <span style=${{ font: '400 12px/1 ui-monospace,Menlo,monospace', color: 'rgba(29,31,32,.7)' }}>${n.cpu}%</span>
            <span class="mono-11" style=${{ color: 'rgba(29,31,32,.6)' }}>${n.uptime}</span>
          </div>`)}
      </div></div>
    </div>

    <div class="two-col" style=${{ display: 'grid', gridTemplateColumns: 'repeat(2,minmax(0,1fr))', gap: '20px' }}>
      <div class="panel">
        <${Head} title="Open alerts"><a href="#alerts" onClick=${(e) => { e.preventDefault(); a.go('alerts'); }}>All rules →</a><//>
        ${OPEN_ALERTS.map((al, i) => html`
          <div key=${i} class="grid-row" style=${{ gridTemplateColumns: '52px minmax(0,1fr) auto', gap: '12px', padding: '11px 16px', alignItems: 'start' }}>
            <${Tag} col=${sevColor(al.sev)}>${al.sev}<//>
            <span><span style=${{ display: 'block', fontSize: '14px', lineHeight: 1.35 }}>${al.msg}</span><span class="note" style=${{ display: 'block', lineHeight: 1.5 }}>${al.meta}</span></span>
            <span class="note" style=${{ lineHeight: 1.5, whiteSpace: 'nowrap' }}>${al.age}</span>
          </div>`)}
      </div>
      <div class="panel">
        <${Head} title="Access attempts · 24h"><a href="#sec" onClick=${(e) => { e.preventDefault(); a.go('sec'); }}>Full feed →</a><//>
        <div style=${{ display: 'grid', gridTemplateColumns: 'repeat(3,minmax(0,1fr))', borderBottom: '1px solid rgba(29,31,32,.08)' }}>
          ${SEC_KPIS.map(k => html`
            <div key=${k.k} style=${{ padding: '12px 16px', borderRight: '1px solid rgba(29,31,32,.08)' }}>
              <div class="eyebrow" style=${{ letterSpacing: '.1em' }}>${k.k}</div>
              <div class="kpi-30" style=${{ color: k.col || undefined }}>${k.v}</div>
            </div>`)}
        </div>
        ${SEC_EVENTS.slice(0, 4).map((e, i) => html`
          <div key=${i} class="grid-row" style=${{ gridTemplateColumns: '58px 1.1fr 1fr auto' }}>
            <span class="mono-10">${e.time}</span>
            <span class="num-11">${e.ip}</span>
            <span class="dim-12">${e.geo}</span>
            <${Tag} sm col=${actColor(e.action)}>${e.action}<//>
          </div>`)}
      </div>
    </div>
  </div>`;

/* ── filesystem ─────────────────────────────────────────────── */
const VOL_TPL = '1.2fr .8fr .8fr .8fr 1fr 1fr';

export const Filesystem = ({ vm }) => html`
  <div class="stack">
    <div class="panel" style=${{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit,minmax(200px,1fr))' }}>
      ${vm.fsKpis.map(k => html`
        <div key=${k.k} class="stat-cell">
          <div class="eyebrow" style=${{ letterSpacing: '.14em' }}>${k.k}</div>
          <div style=${{ display: 'flex', alignItems: 'baseline', gap: '5px', marginTop: '5px' }}>
            <span class="hd" style=${{ fontSize: '36px', lineHeight: 1 }}>${k.v}</span>
            <span class="hd" style=${{ fontSize: '14px', color: 'rgba(29,31,32,.55)' }}>${k.u}</span>
          </div>
          <div class="note" style=${{ marginTop: '3px' }}>${k.note}</div>
        </div>`)}
    </div>

    <div style=${{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit,minmax(280px,1fr))', gap: '16px' }}>
      ${vm.nodes.map(n => html`
        <div key=${n.id} class="panel" style=${{ padding: '15px 16px' }}>
          <div style=${{ display: 'flex', justifyContent: 'space-between', alignItems: 'baseline' }}>
            <span class="host-name" style=${{ fontSize: '19px' }}>${n.name}</span>
            <${Tag} col=${n.col}>${n.state}<//>
          </div>
          <div class="note" style=${{ lineHeight: 1.5 }}>${n.model}</div>
          <div style=${{ display: 'flex', alignItems: 'flex-end', gap: '6px', marginTop: '12px' }}>
            <span class="hd" style=${{ fontSize: '30px', lineHeight: 1 }}>${n.freeLabel}</span>
            <span class="hd" style=${{ fontSize: '13px', color: 'rgba(29,31,32,.55)', paddingBottom: '3px' }}>free</span>
          </div>
          <div style=${{ marginTop: '10px' }}><${Bar} h="8px" style=${n.capBarStyle}/></div>
          <div class="note" style=${{ display: 'flex', justifyContent: 'space-between', color: 'rgba(29,31,32,.55)', marginTop: '6px' }}><span>${n.capLabel}</span><span>${n.usedPct}%</span></div>
          <div style=${{ display: 'grid', gridTemplateColumns: 'repeat(3,1fr)', gap: '8px', marginTop: '13px', paddingTop: '11px', borderTop: '1px solid rgba(29,31,32,.1)' }}>
            <div><div class="eyebrow" style=${{ letterSpacing: '.1em' }}>SMART</div><div style=${n.smartStyle}>${n.smart}</div></div>
            <div><div class="eyebrow" style=${{ letterSpacing: '.1em' }}>WEAR</div><div class="hd" style=${{ fontSize: '15px' }}>${n.wear}%</div></div>
            <div><div class="eyebrow" style=${{ letterSpacing: '.1em' }}>FULL IN</div><div class="hd" style=${{ fontSize: '15px' }}>${n.fullIn}</div></div>
          </div>
          ${corners(false)}
        </div>`)}
    </div>

    <div class="panel">
      <${Head} title="Volume layout · orchardfs"/>
      <div class="table-scroll"><div style=${{ minWidth: '720px' }}>
        <${GridHead} cols=${COLS.vol} tpl=${VOL_TPL}/>
        ${VOLUMES.map(v => html`
          <div key=${v.path} class="grid-row" style=${{ gridTemplateColumns: VOL_TPL }}>
            <span class="num">${v.path}</span><span class="dim">${v.kind}</span>
            <span style=${{ font: '400 12px/1 ui-monospace,Menlo,monospace' }}>${v.size}</span>
            <span style=${{ font: '400 12px/1 ui-monospace,Menlo,monospace' }}>${v.used}</span>
            <span class="dim">${v.host}</span><span class="dim">${v.note}</span>
          </div>`)}
      </div></div>
    </div>
  </div>`;

/* ── throughput ─────────────────────────────────────────────── */
export const Throughput = ({ vm }) => html`
  <div class="stack">
    <div class="io-col" style=${{ display: 'grid', gridTemplateColumns: 'minmax(0,1fr) 330px', gap: '20px', alignItems: 'start' }}>
      <div class="panel" style=${{ padding: '16px 18px' }}>
        <div style=${{ display: 'flex', justifyContent: 'space-between', alignItems: 'baseline' }}>
          <h3 style=${{ fontSize: '19px', letterSpacing: '.03em', textTransform: 'uppercase' }}>Cluster throughput</h3>
          <div style=${{ display: 'flex', gap: '14px' }}>
            <span class="legend"><i style=${{ width: '14px', height: '2px', background: OK }}/><span>READ</span></span>
            <span class="legend"><i style=${{ width: '14px', height: '2px', background: '#b7b7ba' }}/><span>WRITE</span></span>
          </div>
        </div>
        <div style=${{ marginTop: '14px' }}><${Lines} view="0 0 600 190" w="600" h="230px" read=${vm.bigReadPts} write=${vm.bigWritePts} sw="2" grid=${[47.5, 95, 142.5]}/></div>
        <div class="note" style=${{ display: 'flex', justifyContent: 'space-between', marginTop: '4px' }}><span>−6h</span><span>−4h</span><span>−2h</span><span>now</span></div>
        ${corners(true)}
      </div>

      <div class="panel" style=${{ padding: '15px 16px', background: '#1d2d3d', color: '#f2f2f3' }}>
        <div class="eyebrow-accent" style=${{ color: '#94bce3' }}>Instrumentation note</div>
        <h4 style=${{ fontSize: '20px', marginTop: '6px', letterSpacing: '.01em' }}>Passive first, synthetic nightly</h4>
        <p style=${{ fontSize: '13px', lineHeight: 1.5, margin: '8px 0 0', color: 'rgba(242,242,243,.82)' }}>Live numbers are passive: the agent samples <span class="mono" style=${{ fontSize: '12px', color: '#94bce3' }}>iostat</span> every 10s per volume, so the chart is real traffic, not capability. That is a fine proxy for “is it slow right now?” but it can’t tell a quiet disk from a failing one.</p>
        <p style=${{ fontSize: '13px', lineHeight: 1.5, margin: '10px 0 0', color: 'rgba(242,242,243,.82)' }}>So a 30-second <span class="mono" style=${{ fontSize: '12px', color: '#94bce3' }}>fio</span> pass runs on each host at 04:15 during the maintenance window and writes a capability baseline. Drift over 15% against the baseline opens a degradation alert.</p>
        <div style=${{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '10px', marginTop: '14px', paddingTop: '12px', borderTop: '1px solid rgba(242,242,243,.2)' }}>
          <div><div class="eyebrow" style=${{ letterSpacing: '.1em', color: 'rgba(242,242,243,.55)' }}>LAST BASELINE</div><div class="hd" style=${{ fontSize: '17px' }}>04:15 today</div></div>
          <div><div class="eyebrow" style=${{ letterSpacing: '.1em', color: 'rgba(242,242,243,.55)' }}>HOSTS DRIFTING</div><div class="hd" style=${{ fontSize: '17px', color: WARN }}>1 · pippin</div></div>
        </div>
      </div>
    </div>

    <div class="panel">
      <${Head} title="Per-host read / write"/>
      <div class="table-scroll"><div style=${{ minWidth: '640px' }}>
        ${vm.nodes.map(n => html`
          <div key=${n.id} class="grid-row" style=${{ gridTemplateColumns: '120px 1.5fr 130px 100px', gap: '16px', padding: '13px 16px' }}>
            <div><div class="host-name" style=${{ fontSize: '17px' }}>${n.name}</div><div class="eyebrow" style=${{ letterSpacing: 0, textTransform: 'none' }}>${n.bus}</div></div>
            <div style=${{ display: 'flex', flexDirection: 'column', gap: '5px' }}>
              <div style=${{ display: 'flex', alignItems: 'center', gap: '9px' }}>
                <span class="eyebrow" style=${{ width: '12px', letterSpacing: 0 }}>R</span>
                <span style=${{ flex: 1 }}><${Bar} h="9px" style=${n.readBarStyle}/></span>
                <span class="num-11" style=${{ width: '64px', textAlign: 'right' }}>${n.read} MB/s</span>
              </div>
              <div style=${{ display: 'flex', alignItems: 'center', gap: '9px' }}>
                <span class="eyebrow" style=${{ width: '12px', letterSpacing: 0 }}>W</span>
                <span style=${{ flex: 1 }}><${Bar} h="9px" style=${n.writeBarStyle}/></span>
                <span class="num-11" style=${{ width: '64px', textAlign: 'right' }}>${n.write} MB/s</span>
              </div>
            </div>
            <svg viewBox="0 0 120 34" preserveAspectRatio="none" style=${{ width: '100%', height: '34px', display: 'block' }}>
              <polyline points=${n.sparkPts} fill="none" stroke=${OK} stroke-width="1.2"/>
            </svg>
            <div style=${{ textAlign: 'right' }}><div class="eyebrow" style=${{ letterSpacing: '.1em' }}>VS BASELINE</div><div style=${n.driftStyle}>${n.drift}</div></div>
          </div>`)}
      </div></div>
    </div>
  </div>`;

/* ── access control ─────────────────────────────────────────── */
const SEC_TPL = '62px 1.1fr 1.3fr .9fr .9fr .8fr .8fr';

export const Access = () => html`
  <div class="stack">
    <div class="panel" style=${{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit,minmax(160px,1fr))' }}>
      ${SEC_STATS.map(k => html`
        <div key=${k.k} class="stat-cell">
          <div class="eyebrow" style=${{ letterSpacing: '.14em' }}>${k.k}</div>
          <div class="kpi-30" style=${{ color: k.col || undefined }}>${k.v}</div>
          <div class="note">${k.note}</div>
        </div>`)}
    </div>

    <div style=${{ display: 'flex', flexWrap: 'wrap', gap: '20px', alignItems: 'flex-start' }}>
      <div class="panel" style=${{ flex: '1 1 560px' }}>
        <${Head} title="Authentication feed"><span class="note">sshd · smb · webdav · tailing</span><//>
        <div class="table-scroll"><div style=${{ minWidth: '640px' }}>
          <${GridHead} cols=${COLS.sec} tpl=${SEC_TPL}/>
          ${SEC_EVENTS.map((e, i) => html`
            <div key=${i} class="grid-row" style=${{ gridTemplateColumns: SEC_TPL }}>
              <span class="mono-10">${e.time}</span>
              <span class="num-11">${e.ip}</span>
              <span class="dim-12" style=${{ color: 'rgba(29,31,32,.75)' }}>${e.geo}</span>
              <span class="mono-11">${e.host}</span>
              <span class="mono-11">${e.user}</span>
              <span class="dim-12" style=${{ color: 'rgba(29,31,32,.6)' }}>${e.method}</span>
              <${Tag} sm col=${actColor(e.action)}>${e.action}<//>
            </div>`)}
        </div></div>
      </div>

      <div style=${{ display: 'flex', flexDirection: 'column', gap: '16px', flex: '1 1 286px', minWidth: 0 }}>
        <div class="panel" style=${{ padding: '15px 16px' }}>
          <div class="eyebrow-accent">Attempts · 7d × 24h</div>
          <div style=${{ display: 'flex', flexDirection: 'column', gap: '3px', marginTop: '11px' }}>
            ${HEAT.map(row => html`
              <div key=${row.day} style=${{ display: 'flex', alignItems: 'center', gap: '7px' }}>
                <span class="eyebrow" style=${{ width: '22px', letterSpacing: 0 }}>${row.day}</span>
                <span style=${{ display: 'flex', gap: '2px', flex: 1 }}>${row.cells.map((c, i) => html`<span key=${i} style=${c.style} title=${c.title}/>`)}</span>
              </div>`)}
          </div>
          <div class="eyebrow" style=${{ display: 'flex', justifyContent: 'space-between', letterSpacing: 0, color: 'rgba(29,31,32,.45)', marginTop: '6px', paddingLeft: '29px' }}>
            <span>00</span><span>06</span><span>12</span><span>18</span><span>23</span>
          </div>
        </div>

        <div class="panel">
          <${Head} sm title="Blocked sources"/>
          ${BLOCKED.map(b => html`
            <div key=${b.ip} style=${{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', gap: '8px', padding: '9px 16px', borderBottom: '1px solid rgba(29,31,32,.08)' }}>
              <span><span class="num-11" style=${{ display: 'block', lineHeight: 1.3 }}>${b.ip}</span><span class="note" style=${{ display: 'block', lineHeight: 1.3 }}>${b.geo}</span></span>
              <span style=${{ textAlign: 'right' }}><span class="hd" style=${{ display: 'block', fontSize: '16px' }}>${b.n}</span><span class="eyebrow" style=${{ display: 'block', letterSpacing: 0, textTransform: 'none' }}>${b.rule}</span></span>
            </div>`)}
        </div>
      </div>
    </div>
  </div>`;

/* ── alerting ───────────────────────────────────────────────── */
const LOG_TPL = '96px .8fr 1.9fr .9fr .8fr';
const logColor = (st) => st === 'FAILED' ? CRIT : st === 'SENT' ? 'rgba(29,31,32,.6)' : OK;

export const Alerting = ({ a }) => html`
  <div class="stack">
    <div style=${{ display: 'flex', flexWrap: 'wrap', gap: '20px', alignItems: 'flex-start' }}>

      <div class="panel" style=${{ flex: '0 1 320px', minWidth: '280px', padding: '16px 18px' }}>
        <h3 style=${{ fontSize: '19px', letterSpacing: '.03em', textTransform: 'uppercase' }}>SMS template</h3>
        <div class="note" style=${{ marginBottom: '14px' }}>rendered preview · 160 char budget</div>
        <div style=${{ border: '1px solid rgba(29,31,32,.3)', padding: '9px 9px 16px', background: '#e9e9ea' }}>
          <div class="eyebrow" style=${{ display: 'flex', justifyContent: 'space-between', letterSpacing: 0, textTransform: 'none', padding: '2px 3px 9px' }}><span>9:41</span><span>ORCHARD ALERTS</span></div>
          ${SMS_PREVIEW.map((m, i) => html`
            <div key=${i} style=${{ border: '1px solid rgba(29,31,32,.22)', background: '#f2f2f3', padding: '9px 10px', marginBottom: '8px' }}>
              <div style=${{ fontSize: '12.5px', lineHeight: 1.45 }}>${m.text}</div>
              <div class="eyebrow" style=${{ letterSpacing: 0, textTransform: 'none', color: 'rgba(29,31,32,.45)', marginTop: '6px' }}>${m.meta}</div>
            </div>`)}
        </div>
        <div style=${{ marginTop: '14px', display: 'flex', flexDirection: 'column', gap: '7px' }}>
          <div class="eyebrow">Variables</div>
          <div style=${{ display: 'flex', flexWrap: 'wrap', gap: '6px' }}>
            ${SMS_VARS.map(v => html`<span key=${v} style=${{ font: '400 10px/1 ui-monospace,Menlo,monospace', border: '1px solid #5980a6', color: '#416180', padding: '4px 8px' }}>${v}</span>`)}
          </div>
        </div>
        ${corners(false)}
      </div>

      <div style=${{ display: 'flex', flexDirection: 'column', gap: '16px', flex: '1 1 520px', minWidth: 0 }}>
        <div class="panel">
          <${Head} title="Escalation"><span class="note">evaluated top to bottom</span><//>
          ${RULES.map(r => html`
            <div key=${r.n} class="grid-row" style=${{ gridTemplateColumns: '26px minmax(0,1fr) auto', gap: '12px', padding: '13px 16px' }}>
              <span style=${{ font: '500 10px/1 ui-monospace,Menlo,monospace', color: 'rgba(29,31,32,.45)' }}>${r.n}</span>
              <span style=${{ display: 'flex', flexWrap: 'wrap', alignItems: 'center', gap: '7px' }}>
                <span class="eyebrow" style=${{ letterSpacing: '.1em', color: 'rgba(29,31,32,.45)' }}>IF</span>
                <span style=${{ fontSize: '13px', border: '1px solid rgba(29,31,32,.2)', padding: '4px 9px' }}>${r.cond}</span>
                <span class="eyebrow" style=${{ letterSpacing: '.1em', color: 'rgba(29,31,32,.45)' }}>FOR</span>
                <span style=${{ fontSize: '13px', border: '1px solid rgba(29,31,32,.2)', padding: '4px 9px' }}>${r.dur}</span>
                <span class="eyebrow" style=${{ letterSpacing: '.1em', color: 'rgba(29,31,32,.45)' }}>THEN</span>
                <span style=${{ fontSize: '13px', border: '1px solid #5980a6', background: 'rgba(89,128,166,.12)', color: '#2c455d', padding: '4px 9px' }}>${r.then}</span>
              </span>
              <${Tag} col=${sevColor(r.sev)}>${r.sev}<//>
            </div>`)}
        </div>

        <div style=${{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit,minmax(238px,1fr))', gap: '16px' }}>
          <div class="panel">
            <${Head} sm title="On-call roster"/>
            ${ROSTER.map(p => html`
              <div key=${p.name} style=${{ padding: '11px 16px', borderBottom: '1px solid rgba(29,31,32,.08)' }}>
                <div style=${{ display: 'flex', justifyContent: 'space-between', alignItems: 'baseline' }}>
                  <span class="hd" style=${{ fontSize: '16px' }}>${p.name}</span>
                  <span style=${{ font: '600 9px/1 ui-monospace,Menlo,monospace', letterSpacing: '.1em', color: p.tag === 'PRIMARY' ? '#f2f2f3' : OK, background: p.tag === 'PRIMARY' ? OK : 'transparent', border: '1px solid ' + OK, padding: '4px 6px' }}>${p.tag}</span>
                </div>
                <div class="note">${p.phone} · ${p.shift}</div>
                <div style=${{ marginTop: '7px' }}><${Bar} h="4px" style=${{ width: p.pct + '%', background: p.pct ? OK : 'transparent' }}/></div>
              </div>`)}
          </div>
          <div class="panel">
            <${Head} sm title="Thresholds"/>
            ${THRESHOLDS.map(t => html`
              <div key=${t.metric} class="grid-row" style=${{ gridTemplateColumns: 'minmax(0,1fr) 58px 58px', gap: '8px' }}>
                <span style=${{ fontSize: '13px' }}>${t.metric}</span>
                <span class="num-11" style=${{ color: WARN, textAlign: 'right' }}>${t.warn}</span>
                <span class="num-11" style=${{ color: CRIT, textAlign: 'right' }}>${t.crit}</span>
              </div>`)}
            <div class="eyebrow" style=${{ display: 'flex', gap: '10px', padding: '10px 16px', letterSpacing: '.1em', color: 'rgba(29,31,32,.45)', justifyContent: 'flex-end' }}><span>WARN</span><span>CRIT</span></div>
          </div>
        </div>
      </div>
    </div>

    <div style=${{ display: 'flex', flexWrap: 'wrap-reverse', gap: '20px', alignItems: 'flex-start' }}>
      <div class="panel" style=${{ flex: '1 1 280px', padding: '15px 16px' }}>
        <h3 style=${{ fontSize: '17px', letterSpacing: '.03em', textTransform: 'uppercase' }}>Maintenance window</h3>
        <div class="note" style=${{ marginBottom: '12px' }}>suppress pages while you work</div>
        <form style=${{ display: 'flex', flexDirection: 'column', gap: '11px' }} onSubmit=${(e) => { e.preventDefault(); a.startWindow(); }}>
          <div class="field"><label for="mw-hosts">Hosts</label><input id="mw-hosts" class="input" value=${a.mwHosts} onInput=${(e) => a.setField('mwHosts', e.target.value)}/></div>
          <div class="field">
            <label>Duration</label>
            <div class="seg" role="group" aria-label="Duration">
              ${MUTE_OPTS.map(o => html`<button key=${o} type="button" class=${'seg-btn' + (a.mute === o ? ' active' : '')} aria-pressed=${a.mute === o} onClick=${() => a.setMute(o)}>${o}</button>`)}
            </div>
          </div>
          <div class="field"><label for="mw-reason">Reason</label><input id="mw-reason" class="input" placeholder="swapping pippin’s boot SSD" value=${a.mwReason} onInput=${(e) => a.setField('mwReason', e.target.value)}/></div>
          <button type="submit" class="btn btn-primary btn-block">${a.windowActive ? 'End window' : 'Start window'}</button>
          ${a.windowActive ? html`<div class="note" style=${{ color: '#416180' }}>window active · ${a.mute} · pages for ${a.mwHosts} suppressed</div>` : null}
        </form>
      </div>

      <div class="panel" style=${{ flex: '1 1 560px' }}>
        <${Head} title="Notification log"/>
        <div class="table-scroll"><div style=${{ minWidth: '640px' }}>
          <${GridHead} cols=${COLS.log} tpl=${LOG_TPL}/>
          ${a.notifLog.map((l, i) => html`
            <div key=${i} class="grid-row" style=${{ gridTemplateColumns: LOG_TPL }}>
              <span class="mono-10">${l.time}</span>
              <span class="mono-11">${l.host}</span>
              <span style=${{ fontSize: '13px' }}>${l.msg}</span>
              <span class="dim-12">${l.to}</span>
              <${Tag} sm col=${logColor(l.st)}>${l.st}<//>
            </div>`)}
        </div></div>
      </div>
    </div>
  </div>`;

/* ── host detail ────────────────────────────────────────────── */
const DISK_TPL = '1.1fr .9fr .7fr .8fr .8fr .8fr';

export const HostDetail = ({ vm, sel, a }) => html`
  <div class="stack">
    <div style=${{ display: 'flex', alignItems: 'flex-end', gap: '18px', flexWrap: 'wrap' }}>
      <div>
        <div class="kicker">Host</div>
        <div style=${{ display: 'flex', alignItems: 'baseline', gap: '12px' }}>
          <h2 style=${{ fontSize: '40px', letterSpacing: '.01em', textTransform: 'uppercase' }}>${sel.name}</h2>
          <${Tag} col=${sel.col}>${sel.state}<//>
        </div>
        <div class="note-11">${sel.model} · ${sel.ip} · up ${sel.uptime}</div>
      </div>
      <div style=${{ marginLeft: 'auto', display: 'flex', gap: '8px', flexWrap: 'wrap' }}>
        ${vm.nodes.map(n => html`<button key=${n.id} class=${'pick' + (n.id === sel.id ? ' active' : '')} aria-pressed=${n.id === sel.id} onClick=${() => a.open(n.id)}>${n.name}</button>`)}
      </div>
    </div>

    <div class="panel" style=${{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit,minmax(160px,1fr))' }}>
      ${sel.tiles.map(t => html`
        <div key=${t.k} style=${{ padding: '15px 17px', borderRight: '1px solid rgba(29,31,32,.1)', borderBottom: '1px solid rgba(29,31,32,.1)' }}>
          <div class="eyebrow" style=${{ letterSpacing: '.14em' }}>${t.k}</div>
          <div style=${{ display: 'flex', alignItems: 'baseline', gap: '4px', marginTop: '4px' }}>
            <span style=${t.style}>${t.v}</span>
            <span class="hd" style=${{ fontSize: '13px', color: 'rgba(29,31,32,.5)' }}>${t.u}</span>
          </div>
          <div style=${{ marginTop: '9px' }}><${Bar} h="4px" style=${t.barStyle}/></div>
        </div>`)}
    </div>

    <div class="node-col" style=${{ display: 'grid', gridTemplateColumns: 'minmax(0,1.2fr) minmax(0,1fr)', gap: '20px', alignItems: 'start' }}>
      <div class="panel">
        <${Head} title="Disks · SMART"/>
        <div class="table-scroll"><div style=${{ minWidth: '560px' }}>
          <${GridHead} cols=${COLS.disk} tpl=${DISK_TPL}/>
          ${sel.disks.map(d => html`
            <div key=${d.dev} class="grid-row" style=${{ gridTemplateColumns: DISK_TPL }}>
              <span class="num-11">${d.dev}</span><span class="dim-12">${d.model}</span>
              <span style=${{ font: '400 11px/1 ui-monospace,Menlo,monospace' }}>${d.size}</span>
              <span class="mono-11">${d.hours}</span><span class="mono-11">${d.wear}</span>
              <span style=${d.stStyle}>${d.st}</span>
            </div>`)}
        </div></div>
        <div class="panel-head" style=${{ borderTop: '1px solid rgba(29,31,32,.16)' }}><h3>Job queue</h3></div>
        ${sel.jobs.map(j => html`
          <div key=${j.name} class="grid-row" style=${{ gridTemplateColumns: '1.3fr .8fr minmax(0,1fr) 58px', gap: '12px', padding: '11px 16px' }}>
            <span style=${{ fontSize: '13px' }}>${j.name}</span>
            <span class="mono-11" style=${{ color: 'rgba(29,31,32,.6)' }}>${j.owner}</span>
            <${Bar} style=${j.barStyle}/>
            <span class="num-11" style=${{ textAlign: 'right' }}>${j.pct}%</span>
          </div>`)}
      </div>

      <div style=${{ display: 'flex', flexDirection: 'column', gap: '16px' }}>
        <div class="panel" style=${{ padding: '15px 16px' }}>
          <div class="eyebrow-accent">Read / write · 6h</div>
          <div style=${{ marginTop: '10px' }}><${Lines} view="0 0 300 110" w="300" h="130px" read=${sel.readPts} write=${sel.writePts} sw="1.8" grid=${[36.6, 73.3]}/></div>
        </div>
        <div class="panel">
          <${Head} sm title="Host events"/>
          ${sel.events.map((e, i) => html`
            <div key=${i} class="grid-row" style=${{ gridTemplateColumns: '62px minmax(0,1fr)', alignItems: 'start' }}>
              <span class="note" style=${{ lineHeight: 1.5 }}>${e.time}</span>
              <span><span style=${{ display: 'block', fontSize: '13px', lineHeight: 1.4 }}>${e.msg}</span><span class="eyebrow" style=${{ display: 'block', letterSpacing: '.08em', color: 'rgba(29,31,32,.45)' }}>${e.src}</span></span>
            </div>`)}
        </div>
      </div>
    </div>
  </div>`;
