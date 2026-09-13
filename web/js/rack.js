// The server owns the hardware catalog. Free text and identifiers cannot select a mesh.
export function rackFamily(hardware) {
  return ['macbook', 'mac_mini', 'imac'].includes(hardware?.machine_family) ? hardware.machine_family : 'unknown';
}
/** Stable enrolled-time/UUID ordering; one supplied page is the entire scene. */
export function rackPage(nodes, pageIndex, selectedNodeId = null, pageSize = 24) {
  const id = node => node.node_id || node.id;
  const sorted = [...nodes].sort((a,b) => (a.enrolled_at || '').localeCompare(b.enrolled_at || '') || id(a).localeCompare(id(b)));
  const total = sorted.length, size = Math.max(1, Math.min(24, Math.floor(pageSize) || 24)), pageCount = Math.ceil(total / size);
  let index = Math.max(0, Math.min(pageCount - 1, Math.floor(pageIndex) || 0));
  const selected = selectedNodeId ? sorted.findIndex(n => id(n) === selectedNodeId) : -1;
  if (selected >= 0) index = Math.floor(selected / size);
  const start = index * size, page = sorted.slice(start, start + size);
  return { nodes: page, pageIndex: index, pageCount, rangeStart: total ? start + 1 : 0, rangeEnd: start + page.length, total };
}

/** Place measured HTML rectangles for one rack page, keeping their projected
 * anchors instead of moving names onto neighboring meshes. Crowded labels are
 * suppressed; hovering always gives that host first choice of space. */
export function placeRackLabels(projected, {width, height}, hoveredId = null) {
  const edge=8,top=46,bottom=height-52,gap=4,availableWidth=width-edge*2;
  if(!Number.isFinite(width)||!Number.isFinite(height)||availableWidth<=0||bottom<=top)return [];
  const candidates=projected.filter(p=>[p.x,p.y,p.width,p.height,p.depth].every(Number.isFinite)
    && p.width>0 && p.height>0 && p.height<=bottom-top && p.depth>=-1 && p.depth<=1);
  candidates.sort((a,b)=>Number(b.id===hoveredId)-Number(a.id===hoveredId) || a.depth-b.depth || a.id.localeCompare(b.id));
  const placed=[];
  for(const p of candidates) {
    const w=Math.min(p.width,availableWidth);
    const rect={id:p.id,left:Math.max(edge,Math.min(width-edge-w,p.x-w/2)),top:Math.max(top,Math.min(bottom-p.height,p.y-p.height)),width:w,height:p.height};
    if(placed.some(other=>rect.left<other.left+other.width+gap && other.left<rect.left+rect.width+gap
      && rect.top<other.top+other.height+gap && other.top<rect.top+rect.height+gap))continue;
    placed.push(rect);
  }
  return placed;
}
