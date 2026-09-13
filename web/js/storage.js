import { numeric } from './model.js';

/** Canonical graph of server DiskSummary[] and normalized inventory rows.
 * Entries are {objectId, object, disk?, children, references}. Only typed edges
 * choose display parents. physical_disk_ids is server evidence, never inferred.
 * References have no children/capacity and cannot create a second canonical row.
 */
export function storageVM(disks = [], inventory = []) {
  const vm = {disks:[], sharedPools:[], virtual:[], network:[], unresolved:[]};
  const objects = new Map(inventory.map(o => [o.object_id, o]));
  const summaries = new Map(disks.map(d => [d.object_id, d]));
  for (const disk of disks) if (!objects.has(disk.object_id)) objects.set(disk.object_id, {object_id:disk.object_id, kind:'physical_device', properties:{}, physical_disk_ids:[disk.object_id]});
  const shared = new Set(disks.flatMap(d => d.topology?.shared_pool_ids || []));
  const section = object => {
    if (summaries.has(object.object_id)) return `disk:${object.object_id}`;
    const owners = [...new Set(object.physical_disk_ids || [])].filter(id => summaries.has(id)).sort();
    if (shared.has(object.object_id) || owners.length > 1) return 'sharedPools';
    if (owners.length === 1) return `disk:${owners[0]}`;
    const p = object.properties || {};
    if (object.kind === 'nfs_mount' || p.classification === 'network' || p.is_network === true || ['nfs','nfs4','smbfs','cifs','afpfs','webdav'].includes(p.filesystem_type || p.fs_type)) return 'network';
    if (p.virtual === true || p.is_virtual === true || p.classification === 'virtual' || p.disk_image === true) return 'virtual';
    return 'unresolved';
  };
  const entries = new Map([...objects].sort(([a],[b])=>a.localeCompare(b)).map(([id,object]) => [id, {objectId:id, object, disk:summaries.get(id), children:[], references:[], section:section(object)}]));
  const edges = new Map();
  for (const o of objects.values()) for (const edge of o.relationships || []) edges.set(edge.relationship_id || JSON.stringify(edge), edge);
  const candidates = new Map();
  const addReference = (entry, target, kind, unavailable = false) => {
    if (entry && !entry.references.some(r => r.objectId === target && r.kind === kind)) entry.references.push({objectId:target,kind,unavailable});
  };
  for (const edge of edges.values()) {
    const from = entries.get(edge.from_object_id), to = entries.get(edge.to_object_id);
    addReference(from,edge.to_object_id,edge.kind,!to);
    addReference(to,edge.from_object_id,edge.kind,!from);
    if (!from || !to) continue;
    let parent, child;
    if (edge.kind === 'contains') { parent=from; child=to; }
    else if (['backed_by','mounts','attached_to','snapshot_of'].includes(edge.kind)) { parent=to; child=from; }
    else continue;
    if (parent === child || summaries.has(child.objectId) || child.section !== parent.section || shared.has(child.objectId)) continue;
    if (!candidates.has(child.objectId)) candidates.set(child.objectId,[]);
    // More specific containers/volumes beat direct disk display parents.
    candidates.get(child.objectId).push({parent:parent.objectId, priority:edge.kind === 'mounts' ? 0 : summaries.has(parent.objectId) ? 3 : 1});
  }
  const parents = new Map();
  for (const [child, options] of [...candidates].sort(([a],[b])=>a.localeCompare(b))) {
    options.sort((a,b)=>a.priority-b.priority || a.parent.localeCompare(b.parent));
    for (const option of options) {
      let cursor=option.parent; const seen=new Set([child]);
      while(cursor && !seen.has(cursor)) { seen.add(cursor); cursor=parents.get(cursor); }
      if (cursor) continue;
      parents.set(child,option.parent);break;
    }
  }
  for (const entry of entries.values()) {
    let parent = parents.get(entry.objectId);
    if (!parent && entry.section.startsWith('disk:') && !entry.disk) parent=entry.section.slice(5);
    if (parent) entries.get(parent).children.push(entry);
    else vm[entry.disk ? 'disks' : entry.section]?.push(entry);
  }
  for (const disk of disks) for (const pool of disk.topology?.shared_pool_ids || []) addReference(entries.get(disk.object_id),pool,'shared_pool',!entries.has(pool));
  return vm;
}

/** Age a frozen disk-rate measurement without altering its reported state.
 * Server age_seconds accounts for source clock skew. Elapsed browser clocks
 * then consume the remaining source freshness, independently for each direction.
 */
export function diskRateMeasurement(measurement,snapshotAgeMs=0) {
  if(measurement?.state!=='ok')return measurement;
  const sourceAge=measurement.age_seconds;
  const reportedLimit=measurement.ciderd?.stale_after_seconds;
  const sourceLimit=Number.isFinite(reportedLimit) && reportedLimit>0 ? reportedLimit : 15;
  const expired=snapshotAgeMs>=15000 || (Number.isFinite(sourceAge) && sourceAge+Math.max(0,snapshotAgeMs)/1000>=sourceLimit);
  return expired ? {...measurement,state:'stale'} : measurement;
}

/** Append a selected-disk browser-session sample using server rates only.
 * sample={disk,at,error?,timely?,snapshotAgeMs?}; at uses performance.timeOrigin+performance.now.
 * Continuity includes boot, inventory and server's opaque source/association key.
 * Source timestamps identify disk-direction observations, never x-axis positions.
 */
export function appendDiskHistory(history = [], sample, maxPoints = 180) {
  const disk = sample.disk, previous = history.at(-1), io = disk?.io;
  const identity = disk ? JSON.stringify([disk.node_id,disk.object_id,disk.boot_id,disk.inventory_generation,io?.continuity_key,io?.source_object_ids]) : previous?.identity;
  const valid = !sample.error && sample.timely !== false && disk?.active !== false && disk?.availability === 'online' && io?.linkage_state === 'resolved';
  const next = {at:sample.at,identity,breakBefore:!!previous && (identity !== previous.identity || sample.at-previous.at >= 15000)};
  let changed = false;
  for (const direction of ['read','write']) {
    const measurement = diskRateMeasurement(io?.[`${direction}_bytes_per_second`],sample.snapshotAgeMs);
    const signature = measurement ? JSON.stringify([identity,measurement.observed_at,measurement.value,measurement.state,measurement.source,measurement.source_epoch]) : previous?.[`${direction}Signature`];
    const retained = !!previous && signature === previous[`${direction}Signature`];
    next[`${direction}Signature`] = signature;
    next[`${direction}ObservedAt`] = measurement?.observed_at || null;
    next[direction] = valid && !retained ? numeric(measurement) : null;
    changed ||= !retained;
  }
  if (valid && !changed) return history;
  return [...history,next].slice(-maxPoints);
}
