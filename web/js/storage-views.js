import { html } from './lib.js';
import { storageVM, diskRateMeasurement } from './storage.js';
import { topologyStamp } from './session.js';
import { measured, bytes, metricLabel, rateLabel, timeLabel, inventoryLabel, inventoryProperties, coverageLabel, chartPath, chartPoints, chartScale } from './model.js';

const diskLink = (nodeId,id) => `#node/${encodeURIComponent(nodeId)}/disk/${encodeURIComponent(id)}`;
const objectAnchor = id => `storage-object-${id}`;
const Panel = ({title,children}) => html`<section class="panel"><div class="panel-head"><h3>${title}</h3></div><div class="panel-pad">${children}</div></section>`;
const measurementNote = m => `State at snapshot: ${m?.state || 'unknown'} · ${coverageLabel(m)} · Source: ${timeLabel(m?.observed_at)}`;
/** Dated source observations stay inspectable independently of current-rate eligibility. */
export const RawStorageObject = ({object}) => html`<details class="storage-raw"><summary>Source details · ${object.object_id}</summary>
  <p class="note">Object ID: ${object.object_id}<br/>Collector local ID: ${object.local_id || 'Not reported'}<br/>Raw parents: ${(object.parent_ids || []).join(', ') || 'none'}<br/>Backing: ${(object.physical_disk_ids || []).join(', ') || 'Unknown'}<br/>Topology: ${object.topology_state || 'unknown'} ${(object.topology_reason_codes || []).join(', ')}</p>
  <dl class="inventory-properties">${inventoryProperties(object).map(([key,value])=>html`<div key=${key}><dt>${key.replaceAll('_',' ')}</dt><dd>${value}</dd></div>`)}</dl>
  ${(object.latest_metric_series || []).length ? html`<div class="table-scroll"><table class="live-table"><thead><tr><th>Metric</th><th>Value</th><th>State at snapshot</th><th>Source / scope</th><th>Observed</th></tr></thead><tbody>${object.latest_metric_series.map((entry,i)=>html`<tr key=${entry.name+i}><td>${entry.name}</td><td title=${String(entry.measurement?.value ?? '')}>${metricLabel(entry.measurement || {})}</td><td>${entry.measurement?.state || 'unknown'}</td><td>${entry.measurement?.source || 'Not reported'} / ${entry.measurement?.scope || 'Unknown'}</td><td>${timeLabel(entry.measurement?.observed_at)}</td></tr>`)}</tbody></table></div>` : html`<p class="note">No metrics reported for this object.</p>`}
  <details><summary>Full reported object</summary><pre class="storage-json">${JSON.stringify(object,null,2)}</pre></details>
</details>`;
function objectRole(entry) {
  const kind=entry.object.properties?.ciderd_resource_type || entry.object.kind || 'object', source=entry.object.properties?.source || '';
  if(entry.disk)return 'Physical hardware size';
  if(['pool','storage_pool','container','apfs_container'].includes(kind) || source==='diskutil.apfs.container')return 'Pool capacity';
  if(kind==='mount')return 'Mount observations';
  if(['filesystem','volume','apfs_volume'].includes(kind))return 'Volume usage / quota';
  return 'Source observations';
}
const ObjectCapacity = ({entry}) => {
  const role=objectRole(entry), metrics=entry.object.latest_metrics || {};
  const fields=role==='Pool capacity' ? [['Pool total','capacity_bytes'],['Pool used','used_bytes'],['Pool free','free_bytes']]
    : role==='Volume usage / quota' ? [['Volume used','used_bytes'],['Volume quota','apfs_quota_bytes'],['Volume reserve','apfs_reserve_bytes']]
    : role==='Mount observations' ? [['Mount total','capacity_bytes'],['Mount used','used_bytes'],['Mount free','free_bytes'],['Mount available','available_bytes']] : [];
  return fields.length ? html`<dl class="inventory-properties storage-capacity">${fields.map(([label,key])=>html`<div key=${key}><dt>${label}</dt><dd title=${String(metrics[key]?.value ?? '')}>${bytes(measured(metrics[key]))}</dd><div class="note">${measurementNote(metrics[key])}</div></div>`)}</dl>` : null;
};
const StorageEntry = ({entry,nodeId,selectedDiskId,depth=0}) => html`<div class=${'storage-entry'+(entry.disk?' storage-disk':'')} id=${objectAnchor(entry.objectId)}>
  <div class="storage-entry-title"><strong>${entry.disk ? html`<a href=${diskLink(nodeId,entry.objectId)} aria-current=${entry.objectId===selectedDiskId?'page':undefined}>${entry.disk.label || entry.disk.bsd_name || entry.objectId}</a>` : inventoryLabel(entry.object)}</strong><span class="note">${entry.disk ? `Physical disk · ${entry.disk.topology?.state || 'unknown'}` : `${entry.object.kind || 'object'} · ${entry.object.topology_state || 'unknown'}`}</span></div>
  <p class="note">${objectRole(entry)}${entry.disk ? `: ${bytes(measured(entry.disk.hardware_size_bytes))}. Source: ${timeLabel(entry.disk.hardware_size_bytes?.observed_at)}.` : '. Values below belong to this object; nested observations are not additive.'}</p>
  ${entry.disk?.topology?.reason_codes?.length ? html`<p class="note">${entry.disk.topology.reason_codes.join(', ')}</p>` : null}
  ${entry.references.length ? html`<div class="storage-references note">Related: ${entry.references.map(ref=>html`<span key=${ref.kind+ref.objectId}>${ref.unavailable ? `${ref.objectId} (endpoint unavailable)` : html`<a href=${`#${objectAnchor(ref.objectId)}`} onClick=${event=>{event.preventDefault();document.getElementById(objectAnchor(ref.objectId))?.scrollIntoView({block:'center'});}}>${ref.objectId}</a>`} · ${ref.kind.replaceAll('_',' ')}</span>`)}</div>` : null}
  <${ObjectCapacity} entry=${entry}/><${RawStorageObject} object=${entry.object}/>
  ${entry.children.length ? html`<div class=${depth<5?'storage-children':'storage-children storage-flat'}>${entry.children.map(child=>html`<${StorageEntry} key=${child.objectId} entry=${child} nodeId=${nodeId} selectedDiskId=${selectedDiskId} depth=${depth+1}/>` )}</div>` : null}
</div>`;
/** Selected physical DiskSummary and browser history; current=false hides rates
 * without deleting dated topology, exact raw observations or the selected key. */
export function DiskMetricsPanel({disk,history=[],current=false,nodeId,diskId,complete=false,snapshotAgeMs=0}) {
  if(!disk)return html`<${Panel} title="Selected disk"><p role="status">${complete ? 'This disk was removed or is unavailable in the current physical inventory.' : 'This disk is not yet available. Waiting for a complete matching inventory.'}</p><p class="note">${diskId}</p><a href=${`#node/${encodeURIComponent(nodeId)}`}>Back to host storage</a><//>`;
  current=current && disk.active!==false && disk.availability==='online' && disk.io?.linkage_state==='resolved';
  const scale=chartScale(history),values=history.some(p=>Number.isFinite(p.read)||Number.isFinite(p.write));
  return html`<${Panel} title=${`Disk · ${disk.label || disk.bsd_name || disk.object_id}`}>
    <p class="note">${disk.object_id} · ${disk.active===false?'removed':disk.availability || 'unknown'} · I/O linkage ${disk.io?.linkage_state || 'unknown'}. ${current ? 'Confirmed driver source; each rate also honors its source age.' : 'Current rates Unknown while storage snapshots are unavailable, stale or unmatched.'}</p>
    <div class="live-kpis">${['read','write'].map(direction=>html`<div><div class="eyebrow-accent">Sampled disk ${direction}</div><div class="live-kpi">${current ? rateLabel(diskRateMeasurement(disk.io?.[`${direction}_bytes_per_second`],snapshotAgeMs)) : 'Unknown'}</div><p class="note">${measurementNote(disk.io?.[`${direction}_bytes_per_second`])}</p></div>`)}</div>
    <p class="note">Physical hardware size: ${bytes(measured(disk.hardware_size_bytes))}. Capacity attribution: ${disk.capacity?.attribution || 'unresolved'}${disk.capacity?.attribution==='exclusive'?`; exclusive used ${bytes(measured(disk.capacity.used_bytes))} / total ${bytes(measured(disk.capacity.capacity_bytes))}`:''}. Shared pool capacity appears once in the shared-pool section.</p>
    ${values ? html`<div class="chart-plot"><div class="chart-y-axis note"><span>${scale.maxLabel}</span><span>0 B/s</span></div><div class="chart-body"><svg class="disk-chart" viewBox="0 0 600 160" preserveAspectRatio="none" role="img" aria-label=${`Disk rates, 0 to ${scale.maxLabel}; ${timeLabel(scale.start)} to ${timeLabel(scale.end)}`}><path d=${chartPath(history,'read')} fill="none" stroke="#5980a6" stroke-width="2"/><path d=${chartPath(history,'write')} fill="none" stroke="#888b90" stroke-width="2"/>${['read','write'].map(direction=>chartPoints(history,direction).map(p=>html`<circle cx=${p.x} cy=${p.y} r="2.5" fill=${direction==='read'?'#5980a6':'#888b90'}/>`))}</svg><div class="chart-x-axis note"><span>${timeLabel(scale.start)}</span><span>${timeLabel(scale.end)}</span></div></div></div>` : html`<p class="empty-note">Waiting for a new available disk-rate observation. Unknown is never plotted as zero.</p>`}
    <p class="note">Blue: read. Gray: write. SI bytes per second; browser arrival times. Up to 180 session observations; retained source samples are not counted again. Gaps show unavailable data and identity, boot or source changes.</p>
    <details><summary>Disk summary and source provenance</summary><pre class="storage-json">${JSON.stringify(disk,null,2)}</pre></details>
  <//>`;
}
/** Props join a complete normalized inventory to independently polled summaries.
 * No tree expansion initiates a network request. Shared pools have one subtree. */
export function NodeStorage({nodeId,summary,inventory,diskId,history=[],refresh,current=false,snapshotAgeMs=0}) {
  const matching=!!inventory?.matching && !!topologyStamp(summary?.meta) && topologyStamp(summary.meta)===topologyStamp(inventory?.meta), complete=!!inventory?.complete;
  // Keep dated roots from the structure's last matching summaries during replacement.
  const disks=matching ? summary?.data || [] : inventory?.disks || [];
  const vm=storageVM(disks,inventory?.rows || []);
  const selected=summary?.data?.find(d=>d.object_id===diskId) || (!matching ? disks.find(d=>d.object_id===diskId) : null);
  const inventoryState=summary?.meta?.disk_inventory || inventory?.meta?.disk_inventory;
  const sections=[['disks','Physical disks'],['sharedPools','Pools backed by multiple disks'],['virtual','Virtual storage'],['network','Network filesystems'],['unresolved','Unresolved and other source observations']];
  return html`<div class="stack storage-view">
    ${diskId ? html`<${DiskMetricsPanel} disk=${selected} history=${history} snapshotAgeMs=${snapshotAgeMs} current=${current && matching && !summary?.error && !inventory?.error} nodeId=${nodeId} diskId=${diskId} complete=${matching && complete}/>` : null}
    <${Panel} title="Host storage"><div class="page-controls"><button class="btn" onClick=${refresh} disabled=${!!inventory?.busy || !summary?.meta}>Refresh source observations</button><p class="note" role="status">${complete ? `Complete structure: ${inventory.rows.length} objects; snapshot request started ${timeLabel(inventory.snapshotStartedAt)} (${Math.floor((inventory.ageMs || 0)/1000)} seconds old); loaded ${timeLabel(inventory.loadedAt)}.` : `Loading complete storage topology: ${inventory?.loadedCount || 0} objects received.`} ${inventory?.loading && complete ? `Replacement loading: ${inventory.loadedCount} objects.` : ''} ${complete && !matching?'Dated context retained; waiting for matching disk summaries.':''}</p>
      <p class="note">Physical inventory: ${inventoryState?.state || 'pending'}${inventoryState?.reason_codes?.length?` · ${inventoryState.reason_codes.join(', ')}`:''}. Complete structure refreshes independently. Raw observation states are frozen at that snapshot, retained as dated context; each field keeps its original source date.</p>
      ${summary?.error || inventory?.error ? html`<p class="connection-error" role="status">${summary?.error || inventory?.error}</p>` : null}
    </div>
    ${!complete ? html`<p class="empty-note">Grouped storage will appear after all pages of one matching snapshot are loaded. Unloaded records are not absent.</p>` : sections.map(([key,label])=>html`<section class="storage-section" key=${key}><h4>${label} <span class="note">${vm[key].length}</span></h4>${vm[key].length ? vm[key].map(entry=>html`<${StorageEntry} key=${entry.objectId} entry=${entry} nodeId=${nodeId} selectedDiskId=${diskId}/>`):key==='disks'?html`<p class="empty-note">${inventoryState?.state==='ok' && matching?'No physical disks were reported by the completed inventory.':'Physical disks Unknown; inventory has not established current physical devices.'}</p>`:html`<p class="note">No objects assigned to this section in the loaded snapshot.</p>`}</section>`)}
    <//>
  </div>`;
}
