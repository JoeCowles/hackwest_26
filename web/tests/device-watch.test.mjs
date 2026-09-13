import test from 'node:test';
import assert from 'node:assert/strict';
import { DiskMetricsPanel } from '../js/storage-views.js';

const observation = {state:'current',observed_at:'2026-09-13T12:00:00Z',received_at:'2026-09-13T12:00:01Z',age_seconds:1,stale_after_seconds:15};
const watch = overrides => ({watch_id:'watch-1',identity_basis:'reported_usb_serial',identity_scope:'usb_enclosure',presence:'present',armed:true,link_state:'warning',negotiated_bps:'480000000',baseline_bps:'5000000000',reason:'below_confirmed_baseline',observation:{...observation},evidence:{previous_bps:'5000000000',current_bps:'480000000',collection_id:'scan-2'},...overrides});
const disk = device_watch => ({object_id:'disk-1',node_id:'node-1',bsd_name:'disk4',active:true,availability:'online',io:{linkage_state:'unresolved'},device_watch});
const tree = (value, extra={}) => DiskMetricsPanel({disk:disk(value),current:true,nodeId:'node-1',diskId:'disk-1',...extra});
function elements(node) {
  if(node == null || typeof node !== 'object')return [];
  if(Array.isArray(node))return node.flatMap(elements);
  if(typeof node.type === 'function')return elements(node.type(node.props));
  return [node,...elements(node.props?.children)];
}
function text(node, raw=false) {
  if(node == null || typeof node === 'boolean')return '';
  if(Array.isArray(node))return node.map(value=>text(value,raw)).join(' ');
  if(typeof node !== 'object')return String(node);
  if(typeof node.type === 'function')return text(node.type(node.props),raw);
  if(node.type==='pre'&&!raw)return '';
  return text(node.props?.children,raw);
}
const connection = output => {
  const panel=elements(output).find(node=>node.props?.class==='device-watch');
  assert.ok(panel,'Selected disk must expose its USB connection observations');
  return panel;
};

test('fresh USB observations remain visible without a resolved I/O source',()=>{
  const value=watch(),output=tree(value),panel=connection(output),visible=text(panel);
  assert.match(visible,/Presence.*Present/);
  assert.match(visible,/Current/);
  assert.match(visible,/below.*previously confirmed/i);
  assert.match(visible,/480 Mb\/s.*480000000 bit\/s/);
  assert.match(visible,/5 Gb\/s.*5000000000 bit\/s/);
  assert.match(visible,/USB enclosure.*reported USB serial/i);
  assert.match(visible,/media health.*unknown/i);
  assert.match(text(panel,true),/scan-2/);
  assert.match(text(output),/Current rates Unknown/);
});

test('elapsed browser time expires presence and link claims while preserving dated exact evidence',()=>{
  const value=watch(),before=JSON.stringify(value),visible=text(connection(tree(value,{snapshotAgeMs:15000})));
  assert.match(visible,/Presence.*Unknown/);
  assert.match(visible,/Stale/);
  assert.match(visible,/At source observation.*present.*warning/i);
  assert.match(visible,/480000000 bit\/s/);
  assert.doesNotMatch(visible,/Presence\s+Present/);
  assert.equal(JSON.stringify(value),before,'Rendering must not mutate the server evidence');
});

test('refresh failure and offline owner cannot retain current presence labels',()=>{
  for(const extra of [{current:false},{disk:{...disk(watch()),availability:'offline'}}]) {
    const visible=text(connection(tree(watch(),extra)));
    assert.match(visible,/Presence.*Unknown/);
    assert.match(visible,/Stale/);
    assert.doesNotMatch(visible,/Presence\s+Present/);
  }
});

test('an unavailable watch never implies present, a zero speed, or healthy storage',()=>{
  const visible=text(connection(tree(null)));
  assert.match(visible,/unavailable/i);
  assert.match(visible,/Presence.*Unknown/);
  assert.match(visible,/Negotiated.*Unknown/);
  assert.doesNotMatch(visible,/0 bit\/s|Presence\s+Present|healthy/i);
});

test('new connections explain presence arming and unconfirmed link baselines',()=>{
  const visible=text(connection(tree(watch({armed:false,link_state:'warming_up',baseline_bps:null,negotiated_bps:'5000000000',reason:null}))));
  assert.match(visible,/Warming up/);
  assert.match(visible,/two.*presence observations/i);
  assert.match(visible,/Previously confirmed.*Unknown/);
  assert.match(visible,/two matching speed observations/i);
});

test('a pending downshift does not describe its proven baseline as unconfirmed',()=>{
  const visible=text(connection(tree(watch({link_state:'warming_up',reason:'link_degradation_pending'}))));
  assert.match(visible,/Previously confirmed.*5 Gb\/s/);
  assert.match(visible,/link degradation pending/);
  assert.doesNotMatch(visible,/waiting for two matching speed observations to confirm a baseline/i);
});

test('weak identity explains that reconnect comparisons are unavailable',()=>{
  const visible=text(connection(tree(watch({identity_basis:'boot_registry',identity_scope:'driver_incarnation',link_state:'unsupported',baseline_bps:null,reason:'weak_identity'}))));
  assert.match(visible,/Driver incarnation.*boot.*registry/i);
  assert.match(visible,/Unsupported/);
  assert.match(visible,/cannot compare.*reconnect/i);
  assert.doesNotMatch(visible,/below.*previously confirmed/i);
});

test('malformed or missing source freshness cannot claim current presence',()=>{
  for(const change of [{age_seconds:null},{age_seconds:-1},{stale_after_seconds:null},{state:'unknown'}]) {
    const visible=text(connection(tree(watch({observation:{...observation,...change}}))));
    assert.match(visible,/Presence.*Unknown/);
    assert.doesNotMatch(visible,/Presence\s+Present/);
  }
});

test('negotiated rates require canonical positive strings and retain wide exact integers',()=>{
  for(const invalid of [0,'0','-5','01','1e9',null]) {
    const visible=text(connection(tree(watch({negotiated_bps:invalid}))));
    assert.match(visible,/Negotiated.*Unknown/);
  }
  const exact='1208925819614629174706176';
  assert.ok(text(connection(tree(watch({baseline_bps:exact})))).includes(`${exact} bit/s`));
});

test('removed disk selection links to durable attention evidence without claiming a failure',()=>{
  const output=DiskMetricsPanel({disk:null,complete:true,nodeId:'node-1',diskId:'disk-1'});
  const links=elements(output).filter(node=>node.type==='a');
  assert.ok(links.some(link=>link.props.href==='#ops/attention'));
  assert.match(text(output),/recorded.*connection.*concerns/i);
  assert.doesNotMatch(text(output),/drive failed|hardware failure/i);
});
