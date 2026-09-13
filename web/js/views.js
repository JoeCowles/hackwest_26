import { html } from './lib.js';
import { NodeStorage } from './storage-views.js';
import { OK, NAV, statusColor } from './data.js';
import { bytes, measured, timeLabel, metricLabel, chartPath, chartScale, chartPoints, inventoryLabel, inventoryProperties, filesystemMeasurement } from './model.js';

const Panel = ({ title, children, note }) => html`<section class="panel"><div class="panel-head"><h3>${title}</h3>${note ? html`<span class="note">${note}</span>` : null}</div><div class="panel-pad">${children}</div></section>`;
const Empty = ({ children }) => html`<p class="empty-note">${children}</p>`;
const Tag = ({ state }) => html`<span class="tag sm" style=${{color:statusColor(state)}}>${state || 'unknown'}</span>`;
const Kpi = ({ label, value, note }) => html`<div class="panel panel-pad"><div class="eyebrow-accent">${label}</div><div class="live-kpi">${value}</div><p class="note">${note}</p></div>`;
const SharedCapacity = ({ vm }) => html`<${Panel} title="Shared capacity" note=${`${vm.sharedMounts.length} NFS mounts`}>
  ${vm.sharedMounts.length ? vm.sharedMounts.map(m => html`<div class="panel-pad" key=${m.object_id}>
    <strong style=${{overflowWrap:'anywhere'}}>${m.source || 'NFS share'}</strong>
    <div class="note" style=${{overflowWrap:'anywhere'}}>${m.hostLabel} · ${m.mount_point || 'Mount path unavailable'}</div>
    <p>Used ${m.usedLabel} · Free ${m.freeLabel} · Total ${m.totalLabel}</p>
    <a class="host-link" href=${`#node/${encodeURIComponent(m.node_id)}`}>View host storage</a>
  </div>`) : html`<p class="empty-note">No NFS mounts reported. Mount a share on a monitored host to see it here.</p>`}
  <p class="note">${vm.sharedTotal !== 'Unknown' ? `Unique shared total: ${vm.sharedTotal}. ${vm.sharedCoverage}. ` : ''}Capacity is shown per mount. A combined total requires confirmed filesystem identities so overlapping exports are not counted twice.</p>
<//>`;
export const PageControls = ({ paging }) => !paging ? null : html`<div class="page-controls">
  <div class="page-buttons"><button class="btn" disabled=${!!paging.busy || !paging.canPrevious} onClick=${paging.previous}>Previous</button><button class="btn" disabled=${!!paging.busy || !paging.canNext} onClick=${paging.next}>Next</button><button class="btn" disabled=${!!paging.busy} onClick=${paging.refresh}>Refresh snapshot</button></div>
  <div class="note" role="status">${paging.meta ? `Page ${paging.pageNumber}; rows ${paging.rangeStart}–${paging.rangeEnd}${paging.moreAvailable ? '; more rows available' : '; end of snapshot'}. Snapshot: ${timeLabel(paging.meta.server_time)}.` : paging.busy ? 'Loading the first page…' : 'No table snapshot loaded.'} ${paging.busy && paging.meta ? 'Loading…' : ''}</div>
  <p class="note">${paging.following === true ? 'This first filesystem page refreshes every 3 seconds while visible. Next pauses updates to keep all pages in the same snapshot.' : paging.following === false ? 'Filesystem updates are paused while reviewing a frozen snapshot. Refresh snapshot returns to the live first page.' : 'Table observations are frozen while paging. Refresh snapshot to see changes.'} Up to 100 rows per page. Live summaries refresh independently; server cursors expire five minutes after the snapshot is created.</p>
  ${paging.error ? html`<p class="connection-error" role="status">${paging.error} ${paging.rows?.length ? 'Previously loaded rows remain visible for reference.' : ''}</p>` : null}
</div>`;
const FilesystemValue = ({ measurement }) => {
  const field = filesystemMeasurement(measurement);
  return html`<td>${field.label}<div class="note">${field.state}<br/>Source: ${field.observedLabel}</div></td>`;
};
const Chart = ({ history = [] }) => {
  const scale = chartScale(history), latest = history.at(-1);
  const available = history.some(point => Number.isFinite(point.read) || Number.isFinite(point.write));
  return html`<div class="live-chart">
    ${!available ? html`<${Empty}>Waiting for available rates. Unknown values are never plotted as zero.<//>` : html`<div class="chart-plot"><div class="chart-y-axis note"><span>${scale.maxLabel}</span><span>0 B/s</span></div><div class="chart-body"><svg viewBox="0 0 600 160" role="img" aria-label=${`Read and write rates, zero to ${scale.maxLabel}; browser arrival times ${timeLabel(scale.start)} to ${timeLabel(scale.end)}`} preserveAspectRatio="none" overflow="visible"><path d=${chartPath(history,'read')} fill="none" stroke=${OK} stroke-width="2"/><path d=${chartPath(history,'write')} fill="none" stroke="#888b90" stroke-width="2"/>${chartPoints(history,'read').map(point => html`<circle cx=${point.x} cy=${point.y} r="2.5" fill=${OK}/>`)}${chartPoints(history,'write').map(point => html`<circle cx=${point.x} cy=${point.y} r="2.5" fill="#888b90"/>`)}</svg><div class="chart-x-axis note"><span>${timeLabel(scale.start)}</span><span>${timeLabel(scale.end)}</span></div></div></div>`}
    <div class="note">Blue: read. Gray: write. SI units (1 MB = 1,000,000 bytes); vertical scale adjusts to the visible rates. Last ${history.length} browser polls by arrival time. Polls can repeat retained measurements; this is not a count of distinct source samples. Gaps mark unavailable rates or observed continuity changes.</div>
    ${latest ? html`<div class="note chart-source-times">Latest poll's oldest contributing source dates: read ${timeLabel(latest.readObservedAt)}; write ${timeLabel(latest.writeObservedAt)}.</div>` : null}
  </div>`;
};
export const Sidebar = ({ view, nodeCount, go }) => html`<aside class="sidebar">
  <div style=${{padding:'0 18px'}}><div class="brand"><span class="mark"/><span class="word">CIDER</span></div><div class="kicker">Live storage console</div></div>
  <nav aria-label="Console views">${NAV.map(([number,label,id]) => html`<button key=${id} class=${'nav-btn'+(view===id?' active':'')} aria-current=${view===id?'page':undefined} onClick=${()=>go(id)}><span class="n">${number}</span><span class="label">${label}</span></button>`)}</nav>
  <div class="foot note" style=${{marginTop:'auto',padding:'0 18px'}}>Heartbeat 5s<br/>${nodeCount} hosts in the latest snapshot<br/>Viewer session</div>
</aside>`;
export const Topbar = ({ title, vm, clock, connected, disconnect }) => html`<header class="topbar">
  <div><h1>${title}</h1><div class="note">Collector observations, not simulated data</div></div>
  <div class="counts">${[['ONLINE',vm.healthyCount],['DEGRADED',vm.degradedCount],['OFFLINE',vm.offlineCount],['UNKNOWN',vm.unknownCount]].map(([key,value])=>html`<div key=${key}><div class="k">${key}</div><div class="v">${value}</div></div>`)}</div>
  <span class="note">${clock}</span>${connected ? html`<button class="btn" onClick=${disconnect}>Disconnect</button>` : null}
</header>`;
export const Connection = ({ token, onInput, onSubmit, error }) => html`<section class="panel panel-pad connect-panel">
  <div class="eyebrow-accent">Connect to your cluster</div><h2>Real observations. No fixtures.</h2>
  <p>Open the console from your running Cider Server. In the Mac app, select <strong>Copy viewer credential</strong>, then connect below. For headless operation, the owner-only credential is in the server data directory as <code>viewer-token</code>.</p>
  <form onSubmit=${onSubmit}><label for="viewer-token">Read-only viewer credential</label><div class="connect-fields"><input id="viewer-token" type="password" autocomplete="off" spellcheck="false" value=${token} onInput=${e=>onInput(e.target.value)} required placeholder="viewer_..."/><button class="btn btn-primary" type="submit">Connect</button></div></form>
  ${error ? html`<p class="connection-error" role="alert">${error}</p>` : null}
  <p class="note">After connecting, this browser remembers the read-only viewer credential for this server and reconnects after reloads or temporary server outages. Disconnect removes the saved credential and clears this page's observations. Use Disconnect on a shared browser. Expired or rejected credentials are removed automatically.</p>
</section>`;
const Hosts = ({ nodes, open }) => html`<${Panel} title="Enrolled hosts" note="Select a host for its inventory">
  ${nodes.length ? html`<div class="table-scroll"><table class="live-table"><thead><tr><th>Host</th><th>Availability</th><th>Local used / total</th><th>Read</th><th>Write</th><th>Last heartbeat</th></tr></thead><tbody>
    ${nodes.map(n=>html`<tr key=${n.id}><td><button class="host-link" onClick=${()=>open(n.id)}>${n.name}</button><div class="note">${n.model}</div></td><td><${Tag} state=${n.state}/><div class="note">${n.health?.unknown_dimensions?.length || 0} health dimensions unknown</div></td><td>${n.capacityLabel}<span class="bar"><i style=${n.capBarStyle}/></span><div class="note">${n.capacityCoverage}</div></td><td>${n.readLabel}<div class="note">${n.readCoverage}</div></td><td>${n.writeLabel}<div class="note">${n.writeCoverage}</div></td><td>${n.lastSeenLabel}</td></tr>`)}</tbody></table></div>`
    : html`<${Empty}>No nodes are enrolled. Create an enrollment token in the Mac app and connect a collector.<//>`}
<//>`;
const Events = ({ events, paging, empty = 'No collector events were reported in the last hour.' }) => html`<${Panel} title="Collector events" note="Last-hour window at this table snapshot">
  <${PageControls} paging=${paging}/>
  ${events.length ? html`<div class="table-scroll"><table class="live-table"><thead><tr><th>Observed</th><th>Severity</th><th>Source</th><th>Summary</th></tr></thead><tbody>${events.map(e=>html`<tr key=${e.event_id}><td>${timeLabel(e.occurred_at)}</td><td><${Tag} state=${e.severity}/></td><td>${e.source}<div class="note">${e.node_id}</div></td><td>${e.summary}</td></tr>`)}</tbody></table></div>` : html`<${Empty}>${paging?.busy ? 'Loading event observations…' : paging?.error ? 'No event snapshot is available.' : empty}<//>`}
<//>`;
export const Overview = ({ vm, a }) => html`<div class="stack">
  <div class="live-split"><div class="rack-panel"><div class="stage"><div class="stage-host" ref=${a.stageRef}/>
    ${a.noGl || !vm.nodes.length ? html`<div class="stage-empty"><h3>${vm.nodes.length ? 'Rack renderer unavailable or loading' : 'Waiting for enrolled hosts'}</h3><p>The host table remains available without 3D.</p></div>` : null}
    <div class="stage-chrome" style=${{top:0,justifyContent:'space-between'}}><div><div class="eyebrow">Enrolled hosts</div><div class="note">Hosts ${a.rack?.rangeStart || 0}–${a.rack?.rangeEnd || 0} of ${vm.nodes.length}</div></div><span class="note">${vm.stale?'Snapshot unavailable':'Current snapshot'}</span></div>
    <div class="stage-chrome" style=${{bottom:0}}><div class="note">${a.hover ? `${a.hover.name}: read ${a.hover.readLabel}, write ${a.hover.writeLabel}` : 'Drag to orbit. Select a host to inspect. Placement is illustrative; models may be unknown.'}</div></div>
  </div><div class="rack-controls"><button class="btn" disabled=${!a.rack || a.rack.pageIndex===0} onClick=${a.rack?.previous}>Previous hosts</button><span class="note" role="status">Hosts ${a.rack?.rangeStart || 0}–${a.rack?.rangeEnd || 0} of ${vm.nodes.length}</span><button class="btn" disabled=${!a.rack || a.rack.pageIndex+1>=a.rack.pageCount} onClick=${a.rack?.next}>Next hosts</button></div></div><div class="stack"><${Kpi} label="Local capacity" value=${vm.localFree} note=${`Free of ${vm.localTotal}. Free: ${vm.localFreeCoverage}; total: ${vm.localTotalCoverage}. ${vm.coverage}`}/><${Kpi} label="Sampled driver read / write" value=${vm.totalRead} note=${`Read: ${vm.readCoverage} Write ${vm.totalWrite}: ${vm.writeCoverage}`}/><${SharedCapacity} vm=${vm}/></div></div>
  <${Hosts} nodes=${vm.nodes} open=${a.open}/><${Events} events=${vm.events} paging=${a.paging}/>
</div>`;
export const Filesystem = ({ vm, paging }) => html`<div class="stack"><div class="live-kpis"><${Kpi} label="Local free" value=${vm.localFree} note=${`${vm.localFreeCoverage}. ${vm.coverage}`}/><${Kpi} label="Local total" value=${vm.localTotal} note=${`${vm.localTotalCoverage}. APFS containers and identified independent local filesystems.`}/><${Kpi} label="NFS mounts" value=${vm.sharedMounts.length} note="Space usage for each share is shown below."/></div>
  <${SharedCapacity} vm=${vm}/><${Panel} title="Filesystem observations" note="Each field retains its own source date and observation state"><${PageControls} paging=${paging}/>
    ${vm.filesystems.length ? html`<div class="table-scroll"><table class="live-table"><thead><tr><th>Mount / object</th><th>Host</th><th>Type</th><th>Used</th><th>Free</th><th>Available</th></tr></thead><tbody>${vm.filesystems.map(f=>html`<tr key=${f.object_id}><td>${f.mount_point || f.object_id}<div><a class="host-link" href=${`#ops/history/${encodeURIComponent(f.object_id)}`}>History and capacity outlook</a></div></td><td><a class="host-link" href=${`#node/${encodeURIComponent(f.node_id)}${f.physical_disk_ids?.length===1?`/disk/${encodeURIComponent(f.physical_disk_ids[0])}`:''}`}>${vm.nodes.find(n=>n.id===f.node_id)?.name || f.node_id}</a><div class="note">Host storage context</div></td><td>${f.filesystem_type}<div class="note">${f.classification}</div></td><${FilesystemValue} measurement=${f.capacity.used_bytes}/><${FilesystemValue} measurement=${f.capacity.free_bytes}/><${FilesystemValue} measurement=${f.capacity.available_bytes}/></tr>`)}</tbody></table></div>` : html`<${Empty}>${paging?.busy ? 'Loading filesystem observations…' : paging?.error ? 'No filesystem snapshot is available.' : 'No active filesystem inventory has been reported.'}<//>`}
  <//>
</div>`;
export const Throughput = ({ vm, history }) => html`<div class="stack"><div class="live-kpis"><${Kpi} label="Sampled driver read" value=${vm.totalRead} note=${vm.readCoverage}/><${Kpi} label="Sampled driver write" value=${vm.totalWrite} note=${vm.writeCoverage}/></div><${Panel} title="Sampled driver I/O" note="Browser session only"><${Chart} history=${history}/><//>
  <${Panel} title="Per-host rates"><div class="table-scroll"><table class="live-table"><thead><tr><th>Host</th><th>Read</th><th>Write</th><th>Coverage: read / write</th></tr></thead><tbody>${vm.nodes.map(n=>html`<tr key=${n.id}><td>${n.name}</td><td>${n.readLabel}<div class="note">Source: ${n.readObservedLabel}</div></td><td>${n.writeLabel}<div class="note">Source: ${n.writeObservedLabel}</div></td><td>${n.readCoverage}<br/>${n.writeCoverage}</td></tr>`)}</tbody></table></div><//>
  <p class="note">Rates are byte-counter differences averaged over the collector's sampling interval. A short dd run can occupy only part of that interval. Cached reads, buffered writes, and other host traffic can make driver I/O differ from dd's application throughput. Compare the same device and interval; these values do not measure disk capability.</p>
</div>`;
export const HostDetail = ({ vm, node, storage, paging, history, open }) => !node ? html`<${Panel} title="Select an enrolled host"><${Hosts} nodes=${vm.nodes} open=${open}/><//>` : html`<div class="stack">
  <${Panel} title=${node.name} note=${`Inventory generation ${node.inventory_generation}`}><div class="host-summary"><${Tag} state=${node.state}/><span>${node.model}</span><span class="note">Hardware: ${node.hardware?.model_identifier || 'not reported'} · ${node.hardware?.state || 'unknown'} · ${node.kind}</span><span>OS ${node.os_version || 'not reported'}</span><span>Agent ${node.agent_version || 'unknown'}</span><span>Last heartbeat ${node.lastSeenLabel}</span><span>Inventory updated ${timeLabel(node.inventory_updated_at)}</span></div><div class="note">${node.id}</div><p class="note">Unknown health dimensions: ${node.health?.unknown_dimensions?.join(', ') || 'none'}. Availability is not proof of healthy storage.</p><//>
  <div class="live-kpis"><${Kpi} label="Local used / total" value=${node.capacityLabel} note=${node.capacityCoverage}/><${Kpi} label="Sampled driver read" value=${node.readLabel} note=${node.readCoverage}/><${Kpi} label="Sampled driver write" value=${node.writeLabel} note=${node.writeCoverage}/><${Kpi} label="Highest reported device temperature" value=${node.temperatureLabel}/></div>
  <${Panel} title="Observed throughput"><${Chart} history=${history}/><//>
  <${NodeStorage} ...${storage}/>
  <${Events} events=${vm.events.filter(e=>e.node_id===node.id)} paging=${paging}/>
</div>`;
