import test from 'node:test';
import assert from 'node:assert/strict';
import { rackFamily, rackPage } from '../js/rack.js';
import { nodeVM } from '../js/model.js';
import * as rack from '../js/rack.js';
const nodes=Array.from({length:57},(_,i)=>({node_id:`n${String(i).padStart(2,'0')}`,enrolled_at:new Date(i*1000).toISOString()})).reverse();
test('only an explicit catalog family selects a mesh',()=>{
 for(const family of ['macbook','mac_mini','imac','unknown'])assert.equal(rackFamily({machine_family:family}),family);
 for(const hardware of [null,{}, {model_identifier:'Mac14,12'}, {machine_family:'MacBook'}, {machine_family:'mac_studio'}])assert.equal(rackFamily(hardware),'unknown');
 assert.equal(nodeVM({model:'MacBook Pro'}).kind,'unknown');assert.equal(nodeVM({hardware:{machine_family:'mac_mini'}}).kind,'mac_mini');
});
test('57 enrolled nodes stay reachable in stable pages of 24/24/9',()=>{
 assert.deepEqual([0,1,2].map(i=>rackPage(nodes,i).nodes.length),[24,24,9]);
 const p=rackPage(nodes,1);assert.deepEqual([p.rangeStart,p.rangeEnd,p.total],[25,48,57]);assert.equal(p.nodes[0].node_id,'n24');
});
test('selection brings its page into view; joins preserve a valid page and removals clamp',()=>{
 assert.equal(rackPage(nodes,0,'n56').pageIndex,2);
 const joined=[...nodes,{node_id:'joined',enrolled_at:new Date(60000).toISOString()}];assert.equal(rackPage(joined,1).nodes[0].node_id,'n24');assert.equal(rackPage(joined,2).nodes.at(-1).node_id,'joined');
 assert.equal(rackPage(nodes.slice(0,10),2).pageIndex,0);assert.deepEqual(rackPage([],4),{nodes:[],pageIndex:0,pageCount:0,rangeStart:0,rangeEnd:0,total:0});
});

const placeLabels=(...args)=>{
 assert.equal(typeof rack.placeRackLabels,'function','projected labels need rectangle-aware placement');
 return rack.placeRackLabels(...args);
};
const label=(id,x,y,extra={})=>({id,x,y,width:120,height:22,depth:.5,...extra});
const overlaps=(a,b)=>a.left<b.left+b.width && b.left<a.left+a.width && a.top<b.top+b.height && b.top<a.top+a.height;
test('separated projected labels remain attached to their own anchors without changing input identities',()=>{
 const input=[label('one',100,120),label('two',350,120),label('three',100,270),label('four',350,270)],before=structuredClone(input);
 const visible=placeLabels(input,{width:630,height:460});
 assert.deepEqual(new Set(visible.map(l=>l.id)),new Set(['one','two','three','four']));
 for(const item of visible){const source=input.find(l=>l.id===item.id);assert.equal(item.left+item.width/2,source.x);assert.equal(item.top+item.height,source.y);}
 assert.deepEqual(input,before);
});
test('a dense 24-host projection retains useful labels without overlapping rectangles',()=>{
 const input=Array.from({length:24},(_,i)=>label(`host-${i}`,160+(i%4)*70,160+Math.floor(i/4)*17,{depth:.8-i*.01}));
 const visible=placeLabels(input,{width:630,height:460});
 assert.ok(visible.length>=2 && visible.length<24,'crowded labels are selectively suppressed, not all removed');
 assert.equal(new Set(visible.map(l=>l.id)).size,visible.length);
 for(let i=0;i<visible.length;i++)for(let j=i+1;j<visible.length;j++)assert.equal(overlaps(visible[i],visible[j]),false,`${visible[i].id} overlaps ${visible[j].id}`);
});
test('hovered full-size label wins a collision even when a nearer host was first',()=>{
 const input=[label('near',120,170,{depth:.1}),label('hovered',125,172,{width:300,height:64,depth:.9}),label('separate',450,340)];
 const visible=placeLabels(input,{width:630,height:460},'hovered');
 assert.ok(visible.some(l=>l.id==='hovered'));assert.ok(!visible.some(l=>l.id==='near'));assert.ok(visible.some(l=>l.id==='separate'));
 const hovered=visible.find(l=>l.id==='hovered');assert.equal(hovered.width,300);assert.equal(hovered.height,64);
 assert.deepEqual(visible,placeLabels([...input].reverse(),{width:630,height:460},'hovered'),'priority is stable across source iteration order');
});
test('long labels stay inside narrow viewport and clear the rack header and footer',()=>{
 for(const [x,y] of [[1,1],[309,329]]) {
  const [visible]=placeLabels([label('long',x,y,{width:800,height:64})],{width:310,height:330},'long');
  assert.ok(visible);assert.ok(visible.left>=8);assert.ok(visible.left+visible.width<=302);
  assert.ok(visible.top>=46);assert.ok(visible.top+visible.height<=278);
 }
});
test('invalid or clipped projections cannot leave ghost labels and an absent viewport is empty',()=>{
 const input=[label('behind',100,150,{depth:1.1}),label('nan',NaN,150),label('valid',100,150)];
 assert.deepEqual(placeLabels(input,{width:630,height:460}).map(l=>l.id),['valid']);
 assert.deepEqual(placeLabels(input,{width:0,height:0}),[]);
});
