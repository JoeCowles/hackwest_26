// Passive native acquisition + explicitly synthetic USB transitions over real TLS.
// No drive is unplugged, no workload runs, and real notification credentials are
// removed from this child process environment. The disposable server keeps SMS off.
import assert from 'node:assert/strict';
import path from 'node:path';
import {randomUUID} from 'node:crypto';
import {setTimeout as delay} from 'node:timers/promises';
import {smokeCiderd} from './smoke-ciderd.mjs';

for (const key of Object.keys(process.env)) if (key.startsWith('TWILIO_')) delete process.env[key];

await smokeCiderd({cleanupOnFailure:true, verify:async ({temp,node,viewer,request,run})=>{
  const database=path.join(temp,'server','orchard.sqlite3');
  assert.match(node,/^[0-9a-f-]{36}$/);
  let native;
  for (let attempt=0;attempt<6;attempt++) {
    const rows=await run('/usr/bin/sqlite3',[database,
      `SELECT json_extract(c.value,'$.extensions.usb_device_snapshot') FROM batches b,json_each(b.raw_json,'$.collections') c WHERE b.node_id='${node}' AND json_extract(c.value,'$.collector')='iokit.block' AND json_extract(c.value,'$.extensions.usb_device_snapshot') IS NOT NULL ORDER BY b.received_at DESC LIMIT 1;`]);
    if (rows.trim()) {native=JSON.parse(rows);break;}
    await delay(5000);
  }
  assert.ok(native,'Native host IOKit collection publishes a USB snapshot');
  assert.equal(native.version,1);
  assert.equal(typeof native.complete,'boolean');
  assert.ok(Array.isArray(native.devices));
  for (const device of native.devices) {
    assert.match(device.identity,/^[0-9a-f-]{36}$/);
    assert.ok(['reported_usb_serial','boot_registry'].includes(device.identity_basis));
    assert.equal(Object.hasOwn(device,'serial_number'),false);
    if (device.negotiated_bps!==null) assert.match(device.negotiated_bps,/^[1-9][0-9]*$/);
  }

  const issued=await request('POST','/api/v1/enrollment-tokens',{});
  assert.equal(issued.status,201);
  const enrolled=await request('POST','/api/v1/nodes/enroll',{
    enrollment_token:issued.json().data.enrollment_token,
    name:'SYNTHETIC USB showcase verification',agent:{version:'synthetic-1'}
  });
  assert.equal(enrolled.status,201);
  const synthetic=enrolled.json().data;
  const host=`${synthetic.node_id}/host`, physicalDisk='synthetic-physical-usb', identity=randomUUID();
  const relationship='synthetic-driver-to-disk';
  let sequence=0, activeDriver=null;
  function heartbeat({speed='5000000000',present=true,complete=true,driver=firstDriver}={}) {
    sequence++;
    const at=new Date().toISOString(),mono=BigInt(sequence)*5_000_000_000n;
    const previousDriver=activeDriver;
    // Model an independent successful physical inventory alongside complete USB
    // scans. Partial USB scans retain the last graph without claiming removal.
    if(complete) activeDriver=present?driver:null;
    const resource=(resource_id,resource_type,attributes)=>({resource_id,resource_type,
      revision:String(sequence),observed_at:at,identity_confidence:'host_local',attributes});
    const resources=[resource(host,'host',{source:'synthetic-usb-smoke'})];
    const relationships=[],tombstones=[];
    const retire=(entity_type,entity_id)=>tombstones.push({entity_type,entity_id,
      revision:String(sequence),observed_at:at,reason:'removed'});
    if(activeDriver) {
      resources.push(resource(physicalDisk,'physical_device',{source:'diskutil.list.physical',
        bsd_name:'disk99',model:'SYNTHETIC USB storage',size_bytes:'1000000000'}));
      resources.push(resource(activeDriver,'controller',{source:'IOBlockStorageDriver',scope:'driver'}));
      // Reuse the logical edge with a newer revision so a reconnect cannot leave
      // two active drivers associated with the same physical disk.
      relationships.push({relationship_id:relationship,revision:String(sequence),observed_at:at,
        from_resource_id:activeDriver,to_resource_id:physicalDisk,relation:'attached_to',
        attributes:{source:'derived.storage',state:'resolved',mapping_method:'iokit.direct_whole_media',
          boot_id:'synthetic-usb-boot',agent_session_id:'synthetic-usb-session'}});
    }
    if(previousDriver&&previousDriver!==activeDriver) {
      retire('resource',previousDriver);
      if(!activeDriver) {retire('resource',physicalDisk);retire('relationship',relationship);}
    }
    const collection={collection_id:`synthetic-usb-${sequence}`,resource_id:host,collector:'iokit.block',
      adapter_version:'synthetic-1',source_version:'synthetic-usb-smoke',started_at:at,finished_at:at,
      clock_id:'synthetic-usb-clock',started_monotonic_ns:String(mono),finished_monotonic_ns:String(mono),
      status:'ok',metrics:[],extensions:{usb_device_snapshot:{version:1,complete,devices:present?[{
        identity,identity_basis:'reported_usb_serial',identity_scope:'usb_enclosure',driver_resource_id:driver,
        bsd_name:'disk99',negotiated_bps:speed,speed_state:speed===null?'unknown':'available',reason:null
      }]:[]}}};
    return {schema_version:'2.0',message_type:'heartbeat',node_id:synthetic.node_id,
      boot_id:'synthetic-usb-boot',agent_session_id:'synthetic-usb-session',agent_generation:'1',
      sequence:String(sequence),created_at:at,clock_id:'synthetic-usb-clock',monotonic_ns:String(mono+100_000_000n),
      agent:{version:'synthetic-1',target:'aarch64-apple-darwin',os_version:'synthetic',os_build:'synthetic',
        delivery_mode:'latest',heartbeat_interval_seconds:5,quarantined_workers:0,
        discarded_samples_total:'0',dropped_events_total:'0',payload_limited:false},
      inventory:{revision:String(sequence),included:true},resources,relationships,
      collections:[collection],collector_states:[{collector:'iokit.block',resource_id:host,phase:'idle',
        poll_interval_seconds:5,stale_after_seconds:15,last_attempt_id:collection.collection_id}],events:[],tombstones};
  }
  const firstDriver=randomUUID(), reconnectedDriver=randomUUID();
  async function send(body) {
    const r=await request('POST','/api/v2/ciderd/heartbeat',body,synthetic.credential);
    assert.equal(r.status,200,`Synthetic heartbeat rejected: ${r.text}`);
  }
  async function attention() {
    await delay(600);
    const r=await request('GET',`/api/v1/attention?node_id=${synthetic.node_id}&kind=reliability&status=all`);
    assert.equal(r.status,200);
    return r.json().data;
  }
  async function disks() {
    await delay(600);
    const r=await request('GET',`/api/v1/nodes/${synthetic.node_id}/disks`,undefined,viewer);
    assert.equal(r.status,200,'Viewer can read the production physical disk projection');
    const body=r.json();
    assert.equal(body.meta.node_id,synthetic.node_id);
    assert.ok(Array.isArray(body.data));
    return body.data;
  }
  async function diskWatch() {
    const rows=await disks();
    assert.equal(rows.length,1,'Exactly one synthetic physical disk has a current row');
    const disk=rows[0];
    assert.equal(disk.bsd_name,'disk99');
    assert.equal(disk.io.linkage_state,'resolved','The current graph has one attributable driver');
    assert.ok(disk.device_watch,'The production read API exposes the attributable USB watch');
    assert.equal(disk.device_watch.identity_basis,'reported_usb_serial');
    assert.equal(disk.device_watch.identity_scope,'usb_enclosure');
    assert.equal(disk.device_watch.evidence.object_id,disk.io.driver_object_id,
      'USB evidence belongs to the selected current driver');
    for(const key of ['read_bytes_per_second','write_bytes_per_second']) {
      assert.equal(disk.io[key].state,'unknown','No synthetic driver I/O was provided');
      assert.equal(disk.io[key].value,null,'Missing throughput stays unknown rather than zero');
    }
    return disk.device_watch;
  }
  const open=(rows,prefix)=>rows.filter(r=>r.status==='open'&&r.source_key.startsWith(prefix));
  await send(heartbeat());
  let watch=await diskWatch();
  assert.equal(watch.presence,'present');
  assert.equal(watch.armed,false);
  assert.equal(watch.link_state,'warming_up');
  assert.equal(watch.baseline_bps,null,'One observation cannot establish the previous link baseline');
  await send(heartbeat());
  watch=await diskWatch();
  const watchId=watch.watch_id;
  assert.equal(watch.armed,true);
  assert.equal(watch.observation.state,'current');
  assert.equal(watch.negotiated_bps,'5000000000');
  assert.equal(watch.baseline_bps,'5000000000');
  assert.equal(watch.link_state,'no_current_warning');
  await send(heartbeat({present:false,complete:false}));
  assert.equal(open(await attention(),'device_presence:').length,0,'Partial enumeration cannot establish loss');
  watch=await diskWatch();
  assert.equal(watch.presence,'unknown','Partial USB evidence cannot retain a current presence claim');
  assert.equal(watch.link_state,'unknown');
  assert.equal(watch.negotiated_bps,null);
  assert.equal(watch.baseline_bps,'5000000000','Previous dated baseline survives unavailable evidence');
  await send(heartbeat()); await send(heartbeat());
  await send(heartbeat({present:false}));
  assert.equal(open(await attention(),'device_presence:').length,0,'One absence cannot open loss');
  const absent=heartbeat({present:false}); await send(absent);
  let rows=await attention();
  assert.equal(open(rows,'device_presence:').length,1,'Two complete absences open one loss concern');
  assert.equal((await disks()).length,0,'Removed physical disk disappears while its concern remains in Attention');
  const lossId=open(rows,'device_presence:')[0].id;
  await send(absent);
  assert.equal(open(await attention(),'device_presence:')[0].id,lossId,'Receipt replay preserves one episode');
  await send(heartbeat({speed:'480000000',driver:reconnectedDriver}));
  await send(heartbeat({speed:'480000000',driver:reconnectedDriver}));
  rows=await attention();
  assert.equal(open(rows,'device_presence:').length,0,'The same identified connection recovers after replug');
  assert.equal(open(rows,'usb_link:').length,1,'Slower link retains the proven baseline across registry change');
  watch=await diskWatch();
  assert.equal(watch.watch_id,watchId,'The same USB identity keeps its watch through registry replacement');
  assert.equal(watch.evidence.driver_resource_id,reconnectedDriver);
  assert.equal(watch.presence,'present');
  assert.equal(watch.link_state,'warning');
  assert.equal(watch.negotiated_bps,'480000000');
  assert.equal(watch.baseline_bps,'5000000000');
  const linkId=open(rows,'usb_link:')[0].id;
  await send(heartbeat({speed:null,driver:reconnectedDriver}));
  assert.equal(open(await attention(),'usb_link:')[0].id,linkId,'Missing link speed does not recover concern');
  watch=await diskWatch();
  assert.equal(watch.presence,'present');
  assert.equal(watch.link_state,'unknown');
  assert.equal(watch.negotiated_bps,null,'Missing negotiated rate is not the previously observed rate');
  assert.equal(watch.baseline_bps,'5000000000');
  await send(heartbeat({driver:reconnectedDriver}));
  await send(heartbeat({driver:reconnectedDriver}));
  rows=await attention();
  assert.equal(open(rows,'usb_link:').length,0,'Two restored-speed observations recover the link');
  watch=await diskWatch();
  assert.equal(watch.watch_id,watchId);
  assert.equal(watch.link_state,'no_current_warning');
  assert.equal(watch.negotiated_bps,'5000000000');
  assert.equal(watch.baseline_bps,'5000000000');
  const concerns=rows.filter(r=>r.source_key.startsWith('device_presence:')||r.source_key.startsWith('usb_link:'));
  assert.equal(concerns.length,2,'Exactly two synthetic concern episodes were created');
  assert.ok(concerns.every(r=>r.notification?.state==='suppressed'),'Disposable server never enabled SMS');
  console.log(JSON.stringify({usb_watch_result:'passed',native_snapshot_complete:native.complete,
    native_usb_connections:native.devices.length,verified_tls:true,synthetic_loss_and_recovery:true,
    synthetic_link_downshift_and_recovery:true,receipt_replay_idempotent:true,
    viewer_disk_projection:true,synthetic_driver_replacement_mapping:true,unknown_values_preserved:true,
    removed_disk_attention_retained:true,real_sms_sent:false,
    scope:'One real Mac passive collection; all USB fault/link transitions used synthetic observations.'}));
}});
