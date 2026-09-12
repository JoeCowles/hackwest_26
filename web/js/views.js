import { html } from './lib.js';
import { OK, NAV, statusColor } from './data.js';
import { bytes, measured, timeLabel, metricLabel, chartPath } from './model.js';

const Panel = ({ title, children, note }) => html`<section class="panel"><div class="panel-head"><h3>${title}</h3>${note ? html`<span class="note">${note}</span>` : null}</div><div class="panel-pad">${children}</div></section>`;
const Empty = ({ children }) => html`<p class="empty-note">${children}</p>`;
const Tag = ({ state }) => html`<span class="tag sm" style=${{color:statusColor(state)}}>${state || 'unknown'}</span>`;
const Kpi = ({ label, value, note }) => html`<div class="panel panel-pad"><div class="eyebrow-accent">${label}</div><div class="live-kpi">${value}</div><p class="note">${note}</p></div>`;
const Chart = ({ history = [] }) => html`<div class="live-chart">
  ${history.length < 2 ? html`<${Empty}>Waiting for enough live polls to draw a chart. Unknown values are never plotted as zero.<//>` : html`<svg viewBox="0 0 600 160" role="img" aria-label="Observed read and write rates in this browser session" preserveAspectRatio="none"><path d=${chartPath(history,'read')} fill="none" stroke=${OK} stroke-width="2"/><path d=${chartPath(history,'write')} fill="none" stroke="#888b90" stroke-width="2"/></svg>`}
  <div class="note">Blue: read. Gray: write. Auto-scaled; last ${history.length} polls in this session, not historical backfill. Gaps mean unavailable observations.</div>
</div>`;
export const Sidebar = ({ view, nodeCount, go }) => html`<aside class="sidebar">
  <div style=${{padding:'0 18px'}}><div class="brand"><span class="mark"/><span class="word">ORCHARD</span></div><div class="kicker">Live storage console</div></div>
  <nav>${NAV.map(([number,label,id]) => html`<button key=${id} class=${'nav-btn'+(view===id?' active':'')} onClick=${()=>go(id)}><span class="n">${number}</span><span class="label">${label}</span></button>`)}</nav>
  <div class="foot note" style=${{marginTop:'auto',padding:'0 18px'}}>Heartbeat 5s<br/>${nodeCount} hosts in the latest snapshot<br/>Read-only console</div>
</aside>`;
export const Topbar = ({ title, vm, clock, connected, disconnect }) => html`<header class="topbar">
  <div><h1>${title}</h1><div class="note">Collector observations, not simulated data</div></div>
  <div class="counts">${[['ONLINE',vm.healthyCount],['DEGRADED',vm.degradedCount],['OFFLINE',vm.offlineCount],['UNKNOWN',vm.unknownCount]].map(([key,value])=>html`<div key=${key}><div class="k">${key}</div><div class="v">${value}</div></div>`)}</div>
  <span class="note">${clock}</span>${connected ? html`<button class="btn" onClick=${disconnect}>Disconnect</button>` : null}
</header>`;
export const Connection = ({ token, onInput, onSubmit, error }) => html`<section class="panel panel-pad connect-panel">
  <div class="eyebrow-accent">Connect to your cluster</div><h2>Real observations. No fixtures.</h2>
  <p>Open the console from your running Orchard Server. In the Mac app, select <strong>Copy viewer credential</strong>, then connect below. For headless operation, the owner-only credential is in the server data directory as <code>viewer-token</code>.</p>
  <form onSubmit=${onSubmit}><label for="viewer-token">Read-only viewer credential</label><div class="connect-fields"><input id="viewer-token" type="password" autocomplete="off" spellcheck="false" value=${token} onInput=${e=>onInput(e.target.value)} required placeholder="viewer_..."/><button class="btn btn-primary" type="submit">Connect</button></div></form>
  ${error ? html`<p class="connection-error" role="alert">${error}</p>` : null}
  <p class="note">Credentials stay in memory, expire after eight hours, and are cleared on disconnect or reload. Restarting the server rotates the viewer credential. No administrator credential is needed here.</p>
</section>`;
const Hosts = ({ nodes, open }) => html`<${Panel} title="Enrolled hosts" note="Select a host for its inventory">
  ${nodes.length ? html`<div class="table-scroll"><table class="live-table"><thead><tr><th>Host</th><th>Availability</th><th>Local used / total</th><th>Read</th><th>Write</th><th>Last heartbeat</th></tr></thead><tbody>
    ${nodes.map(n=>html`<tr key=${n.id}><td><button class="host-link" onClick=${()=>open(n.id)}>${n.name}</button><div class="note">${n.model}</div></td><td><${Tag} state=${n.state}/><div class="note">${n.health?.unknown_dimensions?.length || 0} health dimensions unknown</div></td><td>${n.capacityLabel}<span class="bar"><i style=${n.capBarStyle}/></span></td><td>${n.readLabel}</td><td>${n.writeLabel}</td><td>${n.lastSeenLabel}</td></tr>`)}</tbody></table></div>`
    : html`<${Empty}>No nodes are enrolled. Create an enrollment token in the Mac app and connect a collector.<//>`}
<//>`;
const Events = ({ events, empty = 'No collector events were reported in the last hour.' }) => html`<${Panel} title="Collector events" note="Last hour; reported events only">
  ${events.length ? html`<div class="table-scroll"><table class="live-table"><thead><tr><th>Observed</th><th>Severity</th><th>Source</th><th>Summary</th></tr></thead><tbody>${events.map(e=>html`<tr key=${e.event_id}><td>${timeLabel(e.occurred_at)}</td><td><${Tag} state=${e.severity}/></td><td>${e.source}<div class="note">${e.node_id}</div></td><td>${e.summary}</td></tr>`)}</tbody></table></div>` : html`<${Empty}>${empty}<//>`}
<//>`;
export const Overview = ({ vm, a }) => html`<div class="stack">
  <div class="live-split"><div class="stage"><div class="stage-host" ref=${a.stageRef}/>
    ${a.noGl || !vm.nodes.length ? html`<div class="stage-empty"><h3>${vm.nodes.length ? 'Rack renderer unavailable or loading' : 'Waiting for enrolled hosts'}</h3><p>The host table remains available without 3D.</p></div>` : null}
    <div class="stage-chrome" style=${{top:0,justifyContent:'space-between'}}><div><div class="eyebrow">Enrolled hosts</div><div class="note">Rack preview: ${Math.min(24,vm.nodes.length)} of ${vm.nodes.length}</div></div><${Tag} state=${vm.stale?'unknown':'online'}/></div>
    <div class="stage-chrome" style=${{bottom:0}}><div class="note">${a.hover ? `${a.hover.name}: read ${a.hover.readLabel}, write ${a.hover.writeLabel}` : 'Drag to orbit. Select a host to inspect. Placement is illustrative; models may be unknown.'}</div></div>
  </div><div class="stack"><${Kpi} label="Local capacity" value=${vm.localFree} note=${`Free of ${vm.localTotal}. ${vm.coverage}`}/><${Kpi} label="Read / write" value=${vm.totalRead} note=${`Write ${vm.totalWrite}. Physical devices counted once.`}/><${Kpi} label="Shared capacity" value=${vm.sharedTotal} note=${`${vm.cluster?.capacity?.unresolved_shared_mounts || 0} NFS mounts lack an authoritative shared identity; excluded from shared totals.`}/></div></div>
  <${Hosts} nodes=${vm.nodes} open=${a.open}/><${Events} events=${vm.events}/>
</div>`;
export const Filesystem = ({ vm }) => html`<div class="stack"><div class="live-kpis"><${Kpi} label="Local free" value=${vm.localFree} note=${vm.coverage}/><${Kpi} label="Local total" value=${vm.localTotal} note="APFS containers and identified independent local filesystems"/><${Kpi} label="Shared total" value=${vm.sharedTotal} note="Authoritative shared identities only; per-mount rows are not additive"/></div>
  <${Panel} title="Filesystem observations" note="Missing quota or capacity is unknown, not unlimited">
    ${vm.filesystems.length ? html`<div class="table-scroll"><table class="live-table"><thead><tr><th>Mount / object</th><th>Host</th><th>Type</th><th>Used</th><th>Free</th><th>Available</th><th>Observation</th></tr></thead><tbody>${vm.filesystems.map(f=>html`<tr key=${f.object_id}><td>${f.mount_point || f.object_id}</td><td>${vm.nodes.find(n=>n.id===f.node_id)?.name || f.node_id}</td><td>${f.filesystem_type}<div class="note">${f.classification}</div></td><td>${vm.stale?'Unknown':bytes(measured(f.capacity.used_bytes))}</td><td>${vm.stale?'Unknown':bytes(measured(f.capacity.free_bytes))}</td><td>${vm.stale?'Unknown':bytes(measured(f.capacity.available_bytes))}</td><td>${vm.stale?'disconnected':f.capacity.capacity_bytes.state}<div class="note">${timeLabel(f.capacity.capacity_bytes.observed_at)}</div></td></tr>`)}</tbody></table></div>` : html`<${Empty}>No active filesystem inventory has been reported.<//>`}
  <//>
</div>`;
export const Throughput = ({ vm, history }) => html`<div class="stack"><div class="live-kpis"><${Kpi} label="Aggregate read" value=${vm.totalRead} note="Derived from compatible cumulative device counters"/><${Kpi} label="Aggregate write" value=${vm.totalWrite} note="Reboots, counter resets, and stale samples break the series"/></div><${Panel} title="Live throughput" note="Browser session only"><${Chart} history=${history}/><//>
  <${Panel} title="Per-host rates"><div class="table-scroll"><table class="live-table"><thead><tr><th>Host</th><th>Read</th><th>Write</th><th>Coverage: read / write</th></tr></thead><tbody>${vm.nodes.map(n=>html`<tr key=${n.id}><td>${n.name}</td><td>${n.readLabel}</td><td>${n.writeLabel}</td><td>${n.read_bytes_per_second.coverage?.observed ?? 0}/${n.read_bytes_per_second.coverage?.expected ?? 0} devices / ${n.write_bytes_per_second.coverage?.observed ?? 0}/${n.write_bytes_per_second.coverage?.expected ?? 0} devices</td></tr>`)}</tbody></table></div><//>
  <p class="note">No synthetic jitter, benchmarks, growth projections, CPU/GPU estimates, or historical charts are generated by this console.</p>
</div>`;
export const Access = ({ vm }) => html`<div class="stack"><${Panel} title="Security visibility"><p>Only security events explicitly reported by a collector appear here. Authentication-attempt counters, geographic enrichment, automatic blocking, and access-management actions are not implemented.</p><//><${Events} events=${vm.events.filter(e=>e.category==='security')} empty="No security events reported. This does not establish that no security activity occurred."/></div>`;
export const Alerting = () => html`<${Panel} title="Alert evaluation is not implemented"><p>The server currently stores collector observations and events. Alert rules, acknowledgements, on-call schedules, SMS delivery, and maintenance windows remain planned.</p><p class="note">There are no mock alert counts, fake deliveries, or buttons that claim to page an operator.</p><//>`;
export const HostDetail = ({ vm, node, inventoryReady, history, open }) => !node ? html`<${Panel} title="Select an enrolled host"><${Hosts} nodes=${vm.nodes} open=${open}/><//>` : html`<div class="stack">
  <${Panel} title=${node.name} note=${`Inventory generation ${node.inventory_generation}`}><div class="host-summary"><${Tag} state=${node.state}/><span>${node.model}</span><span>Agent ${node.agent_version || 'unknown'}</span><span>Last heartbeat ${node.lastSeenLabel}</span></div><div class="note">${node.id}</div><p class="note">Unknown health dimensions: ${node.health.unknown_dimensions.join(', ') || 'none'}. Availability is not proof of healthy storage.</p><//>
  <div class="live-kpis"><${Kpi} label="Local used / total" value=${node.capacityLabel}/><${Kpi} label="Read" value=${node.readLabel}/><${Kpi} label="Write" value=${node.writeLabel}/><${Kpi} label="Device temperature" value=${node.temperatureLabel}/></div>
  <${Panel} title="Observed throughput"><${Chart} history=${history}/><//>
  <${Panel} title="Storage inventory" note="Current objects and source observations">
    ${!inventoryReady ? html`<${Empty}>Loading this host's inventory, or no objects have been reported.<//>` : vm.inventory.map(object=>html`<details class="inventory-object" key=${object.object_id}><summary><strong>${object.local_id}</strong> <span>${object.kind}</span><span class="note">${object.object_id}</span></summary><p class="note">Parents: ${object.parent_ids.join(', ') || 'none'}</p><div class="table-scroll"><table class="live-table"><thead><tr><th>Metric</th><th>Value</th><th>State</th><th>Source / scope</th><th>Observed</th></tr></thead><tbody>${(object.latest_metric_series || []).map((entry,index)=>html`<tr key=${entry.name+index}><td>${entry.name}</td><td title=${String(entry.measurement.value ?? '')}>${vm.stale?'Unknown':metricLabel(entry.measurement)}</td><td>${vm.stale?'disconnected':entry.measurement.state}</td><td>${entry.measurement.source} / ${entry.measurement.scope}</td><td>${timeLabel(entry.measurement.observed_at)}</td></tr>`)}</tbody></table></div>${!object.latest_metric_series?.length ? html`<${Empty}>No metrics reported for this object.<//>` : null}</details>`)}
  <//><${Events} events=${vm.events.filter(e=>e.node_id===node.id)}/>
</div>`;
