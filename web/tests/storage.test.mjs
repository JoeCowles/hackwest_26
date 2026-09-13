import test from 'node:test';
import assert from 'node:assert/strict';
import * as storage from '../js/storage.js';
import * as session from '../js/session.js';
import { chartPath, rateLabel, metricLabel } from '../js/model.js';

const m = (value, observed_at = '2026-09-12T12:00:00Z', state = 'ok') => ({value, observed_at, state, unit:'bytes_per_second'});
const meta = (extra = {}) => ({node_id:'N',boot_id:'B',inventory_generation:'1',topology_revision:'T',disk_inventory:{state:'ok',reason_codes:[]},next_cursor:null,...extra});
const disk = (id = 'P', extra = {}) => ({object_id:id,node_id:'N',boot_id:'B',inventory_generation:'1',active:true,availability:'online',topology:{state:'resolved',shared_pool_ids:[]},io:{linkage_state:'resolved',continuity_key:'K',source_object_ids:['D'],read_bytes_per_second:m(.001),write_bytes_per_second:m(4)},...extra});
const edge = (kind, from, to) => ({relationship_id:`${kind}-${from}-${to}`,kind,from_object_id:from,to_object_id:to});
function fixture(shared = false) {
  const ids = shared ? ['P','Q'] : ['P'];
  const rows = [['P','device'], ...(shared ? [['Q','device']] : []), ['C','pool'],['V1','filesystem'],['V2','filesystem'],['M','mount'],['D','device']].map(([id,kind]) => ({object_id:id,kind,properties:{name:id},physical_disk_ids:ids,relationships:[],topology_state:'resolved'}));
  rows.find(r=>r.object_id==='P').physical_disk_ids=['P'];
  if(shared) rows.find(r=>r.object_id==='Q').physical_disk_ids=['Q'];
  rows.find(r=>r.object_id==='D').physical_disk_ids=['P'];
  rows[0].relationships=[edge('contains','P','C'),edge('contains','C','V1'),edge('contains','C','V2'),edge('mounts','M','V1'),edge('attached_to','D','P')];
  return {rows,disks:ids.map(id=>disk(id,{topology:{state:'resolved',shared_pool_ids:shared?['C']:[]}}))};
}
const entries = vm => {const all=[]; const visit=e=>{all.push(e); e.children.forEach(visit);}; ['disks','sharedPools','virtual','network','unresolved'].forEach(k=>vm[k].forEach(visit)); return all;};

test('typed physical graph nests containers, volumes and mounts once, preserving driver raw details',()=>{
 const {rows,disks}=fixture(); const vm=storage.storageVM(disks,rows), all=entries(vm);
 assert.equal(vm.disks.length,1); assert.equal(new Set(all.map(e=>e.objectId)).size,rows.length);
 assert.equal(all.find(e=>e.objectId==='V1').children[0].objectId,'M');
 assert.equal(all.find(e=>e.objectId==='C').children.length,2);
 assert.equal(all.find(e=>e.objectId==='D').object,rows.at(-1));
});
test('a two-disk pool and its subtree occur only in shared pools with member disk references',()=>{
 const {rows,disks}=fixture(true); const vm=storage.storageVM(disks,rows), all=entries(vm);
 assert.equal(vm.disks.length,2); assert.equal(vm.sharedPools[0].objectId,'C');
 assert.equal(all.filter(e=>e.objectId==='C').length,1); assert.equal(all.length,rows.length);
 assert.ok(vm.disks.every(d=>d.references.some(r=>r.objectId==='C')));
});
test('multiple parents, cycles, dangling links and unassigned raw records stay canonical and inspectable',()=>{
 const {rows,disks}=fixture(); rows[0].relationships.push(edge('contains','V1','C'),edge('contains','P','V1'),edge('mounts','M','absent'));
 rows.push({object_id:'NET',kind:'mount',properties:{filesystem_type:'nfs'},physical_disk_ids:[]},{object_id:'RAW',kind:'device',physical_disk_ids:[]},{object_id:'IMG',kind:'device',properties:{virtual:true},physical_disk_ids:[]});
 const vm=storage.storageVM(disks,rows), all=entries(vm);
 assert.equal(all.length,rows.length); assert.equal(new Set(all.map(e=>e.objectId)).size,rows.length);
 assert.equal(vm.network[0].objectId,'NET'); assert.equal(vm.virtual[0].objectId,'IMG');
 assert.ok(all.some(e=>e.references.some(r=>r.objectId==='absent' && r.unavailable)));
});
test('server assignment, not flattened parents or BSD name, establishes disk ownership',()=>{
 const vm=storage.storageVM([disk()],[{object_id:'V',kind:'filesystem',parent_ids:['P'],properties:{bsd_name:'disk0s1'},physical_disk_ids:[]}]);
 assert.equal(vm.disks[0].children.length,0); assert.equal(vm.unresolved[0].objectId,'V');
});
test('disk history preserves tiny rates, skips retained samples, splits reassociation/reset/failure and caps points',()=>{
 let h=storage.appendDiskHistory([],{disk:disk(),at:1000}); assert.equal(h[0].read,.001);
 assert.equal(rateLabel(disk().io.read_bytes_per_second),'0.001 B/s');
 assert.equal(metricLabel({state:'ok',kind:'counter',unit:'bytes',value:'340282366920938463463374607431768211455'}),'340282366920938463463374607431768211455 bytes');
 h=storage.appendDiskHistory(h,{disk:disk(),at:6000}); assert.equal(h.length,1);
 const changed=disk(); changed.io={...changed.io,continuity_key:'K2',read_bytes_per_second:m(.002,'2026-09-12T12:00:05Z')};
 h=storage.appendDiskHistory(h,{disk:changed,at:11000}); assert.equal(h.at(-1).breakBefore,true);
 h=storage.appendDiskHistory(h,{disk:changed,at:16000,error:'unavailable'}); assert.equal(h.at(-1).read,null);
 h=storage.appendDiskHistory(h,{disk:changed,at:21000}); assert.equal(h.at(-1).read,null,'retained measurement after failure is not a fresh success');
 changed.io.read_bytes_per_second=m(null,'2026-09-12T12:00:10Z','counter_reset');
 h=storage.appendDiskHistory(h,{disk:changed,at:26000}); assert.equal(h.at(-1).read,null);
 assert.ok(!chartPath(h,'read').includes('L'));
 for(let i=0;i<190;i++) h=storage.appendDiskHistory(h,{disk:disk(),at:30000+i,error:'gap'});
 assert.equal(h.length,180);
});
test('disk deep links decode separately and malformed disk ids remain unselected',()=>{
 assert.equal(session.parseRoute('#node/%4e/disk/%50').diskId,'P');
 assert.equal(session.parseRoute('#node/N/disk/%ZZ').diskId,null);
});
test('metadata-preserving disk summary traversals require consistent frozen stamps and timely arrival',async()=>{
 let calls=0; const client={async get(){calls++;return {data:[disk()],meta:meta({next_cursor:calls===1?'next':null,topology_revision:calls===1?'T':'other'})};}};
 await assert.rejects(session.readDisks(client,{node_id:'N'}),/metadata|snapshot|stamp/i);
 let now=0; await assert.rejects(session.readDisks({async get(){now=16000;return {data:[],meta:meta()};}},{node_id:'N'},()=>now),/15 seconds/i);
});
test('2048 objects use five 500-row pages and publish only a complete frozen snapshot',async()=>{
 const calls=[],changes=[]; const client={async get(path,params){calls.push([path,params]);const i=Number(params.cursor||0);return {data:Array.from({length:i===4?48:500},(_,n)=>({object_id:`${i}-${n}`})),meta:meta({next_cursor:i===4?null:String(i+1)})};}};
 const loader=new session.StorageInventory(client,'N',s=>changes.push(s));
 loader.expect(meta()); await loader.refresh('1');
 assert.equal(loader.snapshot().complete,true); assert.equal(loader.snapshot().rows.length,2048);
 assert.equal(calls.length,5); assert.ok(calls.every(([,p])=>p.limit===500 && !p.disk_id));
 assert.ok(changes.some(s=>!s.complete && s.loadedCount===500 && s.rows.length===0));
});
test('topology inventory budgets eight pages per 30 seconds and continues the same cursor',async()=>{
 let now=0,calls=0; const loader=new session.StorageInventory({async get(_p,q){calls++;return {data:[{object_id:String(calls)}],meta:meta({next_cursor:calls<10?String(calls):null})};}},'N',()=>{},{now:()=>now,wallNow:()=>now});
 loader.expect(meta()); await loader.refresh('1'); assert.equal(calls,8); assert.equal(loader.snapshot().complete,false); assert.equal(loader.snapshot().loadedCount,8);
 await loader.continue(); assert.equal(calls,8); now=30001; await loader.continue(); assert.equal(calls,10); assert.equal(loader.snapshot().rows.length,10);
});
test('unchanged structure reuses cache with newer generation while a changed stamp suppresses live matching',async()=>{
 let calls=0;const loader=new session.StorageInventory({async get(){calls++;return {data:[{object_id:'P'}],meta:meta()};}},'N');
 loader.expect(meta());await loader.refresh('1');loader.expect(meta({inventory_generation:'2'})); await loader.refresh('2');
 assert.equal(calls,1);assert.equal(loader.snapshot().matching,true);
 loader.expect(meta({topology_revision:'new'}));assert.equal(loader.snapshot().matching,false);assert.equal(loader.snapshot().rows.length,1);
});
test('frozen-page mismatch cannot replace previously complete topology',async()=>{
 let phase=0; const loader=new session.StorageInventory({async get(_p,q){return phase===0?{data:[{object_id:'old'}],meta:meta()}:{data:[{object_id:'new'}],meta:meta({topology_revision:q.cursor?'bad':'new',next_cursor:q.cursor?null:'next'})};}},'N');
 loader.expect(meta());await loader.refresh('1');phase=1;loader.expect(meta({topology_revision:'new'}));await loader.refresh('1');
 assert.deepEqual(loader.snapshot().rows.map(r=>r.object_id),['old']);assert.match(loader.snapshot().error,/metadata|snapshot|stamp/i);assert.equal(loader.snapshot().matching,false);
});
test('429 honors Retry-After without losing pages; 410 restarts explicitly without showing false emptiness',async()=>{
 let now=0,calls=0; const loader=new session.StorageInventory({async get(_p,q){calls++; if(calls===2)throw Object.assign(new Error('budget'),{status:429,retryAfter:4});if(calls===3)throw Object.assign(new Error('expired'),{status:410});return {data:[{object_id:'P'}],meta:meta({next_cursor:calls===1?'next':null})};}},'N',()=>{},{now:()=>now,wallNow:()=>now});
 loader.expect(meta());await loader.refresh('1');assert.equal(loader.snapshot().loadedCount,1);await loader.continue();assert.equal(calls,2);
 now=4001;await loader.continue();assert.match(loader.snapshot().error,/expired/i);assert.equal(loader.snapshot().complete,false);
 await loader.continue();assert.equal(loader.snapshot().complete,true);
});
test('closed or obsolete boot traversals never publish late data and core reads remain independent',async()=>{
 let finish;const client={async get(path){if(path.endsWith('/inventory'))return new Promise(r=>finish=r);return {data:{fresh:true},meta:meta()};},async all(){return [];}};
 const loader=new session.StorageInventory(client,'N');loader.expect(meta());const loading=loader.refresh('1');
 assert.equal((await session.readSnapshot(client)).cluster.fresh,true);
 loader.expect(meta({boot_id:'new'}));finish({data:[{object_id:'old'}],meta:meta()});await loading;assert.equal(loader.snapshot().rows.length,0);
 const l2=new session.StorageInventory(client,'N');l2.expect(meta());const late=l2.refresh('1');l2.close();finish({data:[{object_id:'late'}],meta:meta()});await late;assert.equal(l2.snapshot().rows.length,0);
});
test('frozen traversal TTL expires even if wall clock advances during sleep',async()=>{
 let wall=0,calls=0;const loader=new session.StorageInventory({async get(){calls++;return {data:[{object_id:String(calls)}],meta:meta({next_cursor:String(calls)})};}},'N',()=>{},{now:()=>0,wallNow:()=>wall});
 loader.expect(meta());await loader.refresh('1');wall=300001;await loader.continue();assert.match(loader.snapshot().error,/expired/i);assert.equal(loader.snapshot().loadedCount,0);
});

const vnodes = node => {
 const all=[];const visit=n=>{if(Array.isArray(n)){n.forEach(visit);return;}if(!n || typeof n!=='object')return;if(typeof n.type==='function'){visit(n.type(n.props));return;}all.push(n);visit(n.props?.children);};visit(node);return all;
};
const visibleText = node => {const text=[];const visit=n=>{if(Array.isArray(n)){n.forEach(visit);return;}if(n==null || typeof n==='boolean')return;if(typeof n!=='object'){text.push(String(n));return;}if(typeof n.type==='function'){visit(n.type(n.props));return;}if(n.type==='pre')return;visit(n.props?.children);};visit(node);return text.join(' ');};
test('render join rechecks metadata to reject a newer summary before its expected-stamp callback',async()=>{
 const {NodeStorage}=await import('../js/storage-views.js');const {rows,disks}=fixture();
 const props={nodeId:'N',diskId:'P',summary:{data:disks,meta:meta({topology_revision:'changed'})},inventory:{rows,disks,meta:meta(),complete:true,matching:true,loadedAt:0},current:true};
 const text=visibleText(NodeStorage(props));assert.match(text,/waiting for matching/i);assert.ok(!text.includes('0.001 B/s'));
});
test('rendered disk removal is explicit and source raw counters remain exact',async()=>{
 const {NodeStorage,RawStorageObject}=await import('../js/storage-views.js');
 const text=visibleText(NodeStorage({nodeId:'N',diskId:'P',summary:{data:[],meta:meta()},inventory:{rows:[],meta:meta(),complete:true,matching:true},current:true}));
 assert.match(text,/removed or is unavailable/i);assert.match(text,/No physical disks were reported/);
 const raw=visibleText(RawStorageObject({object:{object_id:'D',latest_metric_series:[{name:'bytes',measurement:{value:'184467440737095516160',kind:'counter',state:'ok',unit:'bytes'}}]}}));assert.ok(raw.includes('184467440737095516160 bytes'));
});
test('actual host panel passes grouped storage props and rack buttons page outside overlay',async()=>{
 const views=await import('../js/views.js');let page=0;
 const vm={nodes:Array.from({length:57},(_,i)=>({id:String(i),health:{}})),events:[]};
 const tree=views.Overview({vm,a:{rack:{pageIndex:0,pageCount:3,rangeStart:1,rangeEnd:24,next:()=>page++,previous:()=>page--}}});
 const nodes=vnodes(tree),buttons=nodes.filter(n=>n.type==='button');
 const next=buttons.find(b=>b.props.children==='Next hosts');assert.equal(next.props.disabled,false);next.props.onClick();assert.equal(page,1);
 const controls=nodes.find(n=>n.props?.class==='rack-controls');assert.ok(controls);assert.match(visibleText(controls),/Hosts\s+1\s*–\s*24\s+of\s+57/);
 const host=views.HostDetail({vm,node:{id:'N',health:{unknown_dimensions:[]}},storage:{nodeId:'N',summary:{data:[],meta:meta()},inventory:{rows:[],meta:meta(),complete:true,matching:true}},history:[]});
 assert.match(visibleText(host),/No physical disks were reported/);
});

test('generation changes during an active traversal discard the obsolete structure atomically',async()=>{
 let finish,calls=0;const loader=new session.StorageInventory({async get(_p,q){calls++;if(calls===1)return new Promise(r=>finish=r);return {data:[{object_id:'new'}],meta:meta({inventory_generation:q.generation,topology_revision:'new'})};}},'N');
 loader.expect(meta());const first=loader.refresh('1');loader.expect(meta({inventory_generation:'2',topology_revision:'new'}));await loader.refresh('2');
 finish({data:[{object_id:'old'}],meta:meta()});await first;assert.equal(loader.snapshot().rows.length,0);
 await loader.continue();assert.equal(loader.snapshot().rows[0].object_id,'new');assert.equal(loader.snapshot().meta.inventory_generation,'2');
});
test('source observations refresh after 30 seconds while the last complete structure remains published',async()=>{
 let now=0,calls=0;const loader=new session.StorageInventory({async get(){calls++;return {data:[{object_id:'P',value:calls}],meta:meta({inventory_generation:String(calls)})};}},'N',()=>{},{now:()=>now,wallNow:()=>now});
 loader.expect(meta());await loader.refresh('1');now=29999;await loader.refresh('2');assert.equal(calls,1);
 now=30000;await loader.refresh('2');assert.equal(calls,2);assert.equal(loader.snapshot().rows[0].value,2);
});
test('each topology page yields to pending core reads and denied auth reaches the session',async()=>{
 let coreBusy=true,calls=0,auth;const loader=new session.StorageInventory({async get(){calls++;throw Object.assign(new Error('denied'),{status:403});}},'N',()=>{},{canRead:()=>!coreBusy,onAuth:error=>auth=error});
 loader.expect(meta());await loader.refresh('1');assert.equal(calls,0);coreBusy=false;await loader.continue();assert.equal(calls,1);assert.equal(auth.status,403);
});
test('native pool and mount headings preserve their different accounting meanings',async()=>{
 const {NodeStorage}=await import('../js/storage-views.js');const {rows,disks}=fixture();
 rows.find(o=>o.object_id==='C').kind='provider';rows.find(o=>o.object_id==='C').properties.ciderd_resource_type='apfs_container';
 rows.find(o=>o.object_id==='M').kind='filesystem';rows.find(o=>o.object_id==='M').properties.ciderd_resource_type='mount';
 const text=visibleText(NodeStorage({nodeId:'N',summary:{data:disks,meta:meta()},inventory:{rows,meta:meta(),matching:true,complete:true}}));
 assert.match(text,/Pool capacity/);assert.match(text,/Mount observations/);assert.match(text,/Volume usage \/ quota/);
});
test('shared-pool capacity is rendered once on its canonical pool with state and source date',async()=>{
 const {NodeStorage}=await import('../js/storage-views.js');const {rows,disks}=fixture(true);
 rows.find(o=>o.object_id==='C').latest_metrics={capacity_bytes:{...m('987654321000'),unit:'bytes'},used_bytes:{...m(null,undefined,'unknown'),unit:'bytes'}};
 const text=visibleText(NodeStorage({nodeId:'N',summary:{data:disks,meta:meta()},inventory:{rows,meta:meta(),matching:true,complete:true}}));
 assert.equal((text.match(/987.65 GB/g) || []).length,1);assert.match(text,/Pool used.*Unknown/);assert.match(text,/Pool total/);
});
test('switching selected hosts cannot reset the shared topology request budget',async()=>{
 const budget={requests:[]};let calls=0;
 const client={async get(_path,q){calls++;return {data:[{object_id:String(calls)}],meta:meta({next_cursor:String(calls)})};}};
 const first=new session.StorageInventory(client,'N',()=>{},{budget,now:()=>0});first.expect(meta());await first.refresh('1');first.close();assert.equal(calls,8);
 const next=new session.StorageInventory(client,'N',()=>{},{budget,now:()=>0});next.expect(meta());await next.refresh('1');assert.equal(calls,8);
});

test('disk snapshot freshness starts before page one while chart time records final arrival',async()=>{
 let now=1000,wall=100000,calls=0;
 const summary=await session.readDisks({async get(){calls++;if(calls===2){now=15000;wall=114000;}return {data:[disk(String(calls))],meta:meta({server_time:'2099-01-01T00:00:00Z',next_cursor:calls===1?'second':null})};}},{node_id:'N'},()=>now,()=>wall);
 assert.equal(summary.startedAt,1000);assert.equal(summary.startedWallAt,100000);
 assert.equal(summary.arrivedAt,15000);assert.equal(summary.at,performance.timeOrigin+15000);
 assert.equal(session.storageSnapshotAge(summary,15000,114000),14000);assert.equal(session.storageSnapshotAge(summary,29000,128000),28000);
 assert.equal(session.storageSnapshotAge({},15000,114000),Infinity);
 assert.equal(session.snapshotExpired(summary.startedAt,summary.startedWallAt,16000,115000),true);
 assert.equal(session.snapshotExpired(summary.startedAt,summary.startedWallAt,15001,130000),true,'sleep expires the snapshot even if monotonic time pauses');
});
test('throttled ten-page inventory keeps first-request age rather than granting fresh age at completion',async()=>{
 let now=1000,wall=100000,calls=0;
 const loader=new session.StorageInventory({async get(){calls++;return {data:[{object_id:String(calls)}],meta:meta({server_time:'2099-01-01T00:00:00Z',next_cursor:calls<10?String(calls):null})};}},'N',()=>{},{now:()=>now,wallNow:()=>wall});
 loader.expect(meta());await loader.refresh('1');assert.equal(calls,8);now=31001;wall=130001;await loader.continue();
 assert.equal(loader.snapshot().complete,true);assert.equal(loader.snapshot().ageMs,30001);
 assert.equal(loader.snapshot().snapshotStartedAt,100000);assert.equal(loader.snapshot().loadedAt,130001);
 wall=150000;assert.equal(loader.snapshot().ageMs,50000,'source age includes sleep without comparing browser time to server_time');
});
test('disk rates and histories age server-reported measurement age without changing raw states',async()=>{
 const rate={...m(.001),age_seconds:12,ciderd:{stale_after_seconds:15}};
 assert.equal(storage.diskRateMeasurement(rate,2000).state,'ok');
 assert.equal(storage.diskRateMeasurement(rate,4000).state,'stale');assert.equal(rate.state,'ok');
 assert.equal(storage.diskRateMeasurement({...rate,state:'stale'},0).state,'stale');
 assert.equal(storage.diskRateMeasurement({...rate,age_seconds:20,ciderd:{stale_after_seconds:90}},4000).state,'ok');
 assert.equal(storage.diskRateMeasurement({...rate,ciderd:{stale_after_seconds:90}},15000).state,'stale');
 const d=disk();d.io.read_bytes_per_second=rate;d.io.write_bytes_per_second={...m(4),age_seconds:1};
 const h=storage.appendDiskHistory([],{disk:d,at:5000,snapshotAgeMs:4000});assert.equal(h[0].read,null);assert.equal(h[0].write,4);
 const {DiskMetricsPanel}=await import('../js/storage-views.js');
 const text=visibleText(DiskMetricsPanel({disk:d,current:true,snapshotAgeMs:4000}));assert.ok(!text.includes('0.001 B/s'));assert.ok(text.includes('4 B/s'));assert.match(text,/State at snapshot/);
});
