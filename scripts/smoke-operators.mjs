#!/usr/bin/env node
// Disposable real-server HTTP checks with explicitly SYNTHETIC SQLite observations.
// No collector, host exports, user data, .env, or real notification credentials.
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {mkdtemp,writeFile,readFile,mkdir,rm,access} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {createServer} from 'node:net';
import {randomUUID,createHash} from 'node:crypto';
import {setTimeout as delay} from 'node:timers/promises';
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..');
const output=path.join(root,'.codex-staging/operator-workflows');
const serve=process.argv.includes('--serve');
const q=v=>v==null?'NULL':`'${String(v).replaceAll("'","''")}'`;
const iso=ms=>new Date(ms).toISOString();
const admin='SYNTHETIC_QA_ADMIN_ONLY_2026_09_13_DO_NOT_REUSE';
const ids={node:randomUUID(),quotaNode:randomUUID(),lostNode:randomUUID(),pool:randomUUID(),flat:randomUUID(),mount:randomUUID()};
let child,refreshTimer,closed=false,temp,viewer,url,infoPath;
const helpers=new Set();
for(const signal of ['SIGINT','SIGTERM'])process.once(signal,()=>{cleanup().finally(()=>process.exit(0));});
const hash=v=>createHash('sha256').update(v).digest('hex');
async function sql(statement){return new Promise((resolve,reject)=>{const p=spawn('/usr/bin/sqlite3',[path.join(temp,'cider.sqlite3')],{stdio:['pipe','pipe','pipe']});helpers.add(p);p.once('close',()=>helpers.delete(p));let stderr='';p.stderr.on('data',b=>stderr+=b);p.on('error',reject);p.on('close',code=>code?reject(new Error(`Synthetic SQL fixture failed: ${stderr}`)):resolve());p.stdin.end(`.timeout 5000\nPRAGMA foreign_keys=ON;\n${statement}\n`);});}
async function request(route,{method='GET',body,token=viewer,expected=200}={}){const headers={Authorization:`Bearer ${token}`,Accept:'application/json'};if(method!=='GET'){headers['Content-Type']='application/json';headers['X-Request-ID']=randomUUID();headers['X-Request-Timestamp']=iso(Date.now());}const r=await fetch(new URL(route,url),{method,headers,body:body?JSON.stringify(body):undefined,signal:AbortSignal.timeout(10000),redirect:'error'});const text=await r.text();let json;try{json=JSON.parse(text);}catch{json={};}assert.equal(r.status,expected,`${method} ${route}: ${r.status} ${text.slice(0,1000)}`);return json;}
let quotaCapture=null;try{quotaCapture=JSON.parse(await readFile(path.join(output,'quota-api-capture.json'),'utf8'));}catch{}
function latest(object,name,value,at,{state='ok',ciderd,unit='bytes',kind='gauge'}={}){const raw={sample:{object_id:object,name,kind,value,unit,state,source:'synthetic-qa',scope:'container',labels:{},observed_at:iso(at)},scope:'container',boot_id:'synthetic-boot',inventory_generation:'1',received_at:iso(at),derivation_state:'unavailable',...(ciderd?{ciderd}: {})};return `INSERT OR REPLACE INTO latest_samples VALUES(${[object,name,'synthetic-qa','container','{}',JSON.stringify(raw),at,at].map(q).join(',')});`;}
async function seed(now=Date.now()){
 let s='BEGIN;';s+=`UPDATE nodes SET last_seen_at=${now} WHERE node_id IN (${q(ids.node)},${q(ids.quotaNode)});`;
 for(const [object,flat] of [[ids.pool,false],[ids.flat,true]]){
  // Fixed eight dated pairs slide with the synthetic scenario to stay fresh for QA.
  s+=`DELETE FROM metric_samples WHERE object_id=${q(object)};DELETE FROM metric_rollups WHERE object_id=${q(object)};`;
  for(let i=0;i<8;i++){const at=now-(7-i)*600000;for(const [name,value] of [['used_bytes',flat?500:880+i*10],['capacity_bytes',1000]]){s+=`INSERT INTO metric_samples(node_id,object_id,boot_id,generation,name,kind,unit,state,source,scope,labels_json,value_json,numeric_value,observed_at,received_at,derivation_state) VALUES(${[ids.node,object,'synthetic-boot',1,name,'gauge','bytes','ok','synthetic-qa','container','{}',JSON.stringify(String(value)),value,at,at,'unavailable'].map(q).join(',')});`;if(i===7)s+=latest(object,name,String(value),at);}}
 }
 s+=latest(ids.mount,'capacity_bytes','1000',now)+latest(ids.mount,'used_bytes','950',now);
 // Quota capture replay feeds the real read model, with explicit synthetic provenance.
 const captured=Array.isArray(quotaCapture?.data)?quotaCapture.data:Array.isArray(quotaCapture?.response?.data)?quotaCapture.response.data:[];
 for(const [i,row] of captured.entries()){
  const object=row.quota_id || `synthetic-quota-${i}`;const properties={ciderd_resource_type:'nfs_user_quota',server:row.server,export_path:row.export_path,uid:row.uid,display_label:`SYNTHETIC REPLAY ${row.display_label || `quota ${i+1}`}`,nfs_source:row.nfs_source,query_identity_uid:row.query_identity?.uid,query_identity_gid:row.query_identity?.gid,fixture_provenance:'Recorded isolated rquotad fixture response replayed into synthetic QA state; no live query'};
  s+=`INSERT OR REPLACE INTO objects VALUES(${[object,ids.quotaNode,object,'quota','[]',JSON.stringify(properties),1,1].map(q).join(',')});`;
  const ciderd={collection_id:`synthetic-quota-${i}`,clock_id:'synthetic-clock',age_at_receipt_seconds:0,stale_after_seconds:120,source_metric:{availability:'available',freshness:'live'},original_inventory_generation:'1'};
  s+=latest(object,'nfs_quota_status',row.state,now,{ciderd,unit:'enum',kind:'enum'});
  for(const [name,m] of Object.entries(row.metrics || {}))s+=latest(object,`nfs_quota_${name}`,m.value,now,{state:m.state,ciderd,unit:m.unit,kind:m.kind || 'gauge'});
 }
 await sql(s+'COMMIT;');return captured.length;
}
async function cleanup(){if(closed)return;closed=true;clearInterval(refreshTimer);for(const helper of helpers)helper.kill('SIGTERM');if(child&&child.exitCode===null){child.kill('SIGINT');await Promise.race([new Promise(r=>child.once('exit',r)),delay(6000)]);if(child.exitCode===null)child.kill('SIGKILL');}if(temp)await rm(temp,{recursive:true,force:true});if(infoPath)await rm(infoPath,{force:true});}
try{
 await mkdir(output,{recursive:true});temp=await mkdtemp(path.join(tmpdir(),'cider-synthetic-operators-'));await writeFile(path.join(temp,'admin-token'),admin,{mode:0o600});
 const reservation=createServer();await new Promise(r=>reservation.listen(0,'127.0.0.1',r));const port=reservation.address().port;await new Promise(r=>reservation.close(r));url=`http://127.0.0.1:${port}`;
 const env={...process.env};for(const key of Object.keys(env))if(/^(TWILIO_|CIDER_)/.test(key))delete env[key];Object.assign(env,{TWILIO_ACCOUNT_SID:'',TWILIO_SECRET:'',TWILIO_SID:'',TWILIO_AUTH_TOKEN:'',TWILIO_FROM:'',RUST_LOG:'cider_server=warn'});
 await assert.rejects(access(path.join(temp,'.env')),'The disposable working directory must not contain .env');
 child=spawn(path.join(root,'target/debug/cider-server'),['--headless','--bind',`127.0.0.1:${port}`,'--data-dir',temp],{cwd:temp,env,stdio:['ignore','pipe','pipe']});let logs='';child.stdout.on('data',b=>logs+=b);child.stderr.on('data',b=>logs+=b);let spawnError;child.on('error',e=>spawnError=e);
 for(let i=0;i<100;i++){if(spawnError)throw spawnError;if(child.exitCode!==null)throw new Error(`Server exited: ${logs.slice(-2000)}`);try{viewer=(await readFile(path.join(temp,'viewer-token'),'utf8')).trim();await request('/api/v1/notifications/settings');break;}catch{await delay(100);}}
 assert(viewer,'Server did not create its private synthetic viewer credential');
 const now=Date.now();let initial='BEGIN;';for(const [node,name,seen] of [[ids.node,'SYNTHETIC QA Mac',now],[ids.lostNode,'SYNTHETIC QA lost Mac',now-120000],[ids.quotaNode,'SYNTHETIC QA captured quota Mac',now]])initial+=`INSERT INTO nodes(node_id,name,agent_json,credential_hash,enrolled_at,last_seen_at,boot_id,inventory_generation) VALUES(${[node,name,'{"version":"synthetic-qa"}',hash(node),now,seen,'synthetic-boot',1].map(q).join(',')});`;
 for(const [object,kind,props] of [[ids.pool,'apfs_container',{name:'SYNTHETIC QA capacity pool'}],[ids.flat,'apfs_container',{name:'SYNTHETIC QA flat pool'}],[ids.mount,'mount',{mount_point:'/SYNTHETIC-QA',filesystem_type:'apfs',local:true}]])initial+=`INSERT INTO objects VALUES(${[object,ids.node,object,kind,'[]',JSON.stringify(props),1,1].map(q).join(',')});`;
 await sql(initial+'COMMIT;');const quotaRows=await seed();
 let seeding=false;refreshTimer=setInterval(()=>{if(!seeding){seeding=true;seed().catch(e=>{process.stderr.write(`Synthetic refresh failed: ${e.message}\n`);}).finally(()=>seeding=false);}},5000);
 for(const route of ['/api/v1/attention','/api/v1/attention/summary','/api/v1/notifications/settings','/api/v1/diagnostics','/api/v1/quotas']){await request(route);await request(route,{token:'wrong',expected:401});}
 let summary,episodes;for(let i=0;i<12;i++){summary=await request('/api/v1/attention/summary');episodes=await request('/api/v1/attention');if(summary.data.reconciliation?.state==='current'&&episodes.data.some(r=>r.kind==='capacity')&&episodes.data.some(r=>r.kind==='node_loss'))break;await delay(1000);}
 assert.equal(summary.data.reconciliation.state,'current');assert(episodes.data.some(r=>r.kind==='capacity'),'Worker must observe synthetic capacity pressure');assert(episodes.data.some(r=>r.kind==='node_loss'),'Worker must observe synthetic lost node');
 const concern=episodes.data.find(r=>r.kind==='node_loss');const ackBody={expected_revision:concern.revision,note:'SYNTHETIC QA acknowledgement; no real operator action'};
 await request(`/api/v1/attention/${concern.id}/acknowledgement`,{method:'POST',body:ackBody,expected:403});await request(`/api/v1/attention/${concern.id}/acknowledgement`,{method:'POST',token:admin,body:ackBody});await request(`/api/v1/attention/${concern.id}/acknowledgement`,{method:'POST',token:admin,body:ackBody,expected:409});
 const settings=await request('/api/v1/notifications/settings');const config={expected_revision:settings.data.revision,enabled:false,sender:'+15555550100',recipient:'+15555550101'};
 await request('/api/v1/notifications/settings',{method:'PUT',body:config,expected:403});const saved=await request('/api/v1/notifications/settings',{method:'PUT',token:admin,body:config});assert.equal(saved.data.sender,'***0100');assert.equal(saved.data.recipient,'***0101');assert.equal(saved.data.enabled,false);await request('/api/v1/notifications/settings',{method:'PUT',token:admin,body:config,expected:409});
 const end=Date.now(),params=new URLSearchParams({metric:'used_bytes',from:iso(end-3*3600000),to:iso(end),resolution:'raw'});const history=await request(`/api/v1/objects/${ids.pool}/history?${params}`);assert.equal(history.meta.selected_points,8);assert(history.data.series[0].points.every(p=>typeof p.value==='string'));
 const forecast=await request(`/api/v1/objects/${ids.pool}/capacity-forecast`);assert.equal(forecast.data.state,'estimated',JSON.stringify(forecast.data));const flat=await request(`/api/v1/objects/${ids.flat}/capacity-forecast`);assert.equal(flat.data.state,'unknown');assert(flat.data.reasons.includes('no_reliable_growth'));await request(`/api/v1/objects/${ids.pool}/capacity-forecast?unexpected=true`,{expected:400});
 const quotas=await request('/api/v1/quotas');assert.equal(quotas.data.length,quotaRows);for(const source of quotaCapture?.data || []){const row=quotas.data.find(r=>r.quota_id===source.quota_id);assert(row,'Captured quota row is present');assert.equal(row.state,source.state,`Quota replay state ${source.uid}`);for(const field of ['used_bytes','block_soft_limit_bytes','block_hard_limit_bytes'])assert.equal(row.metrics[field]?.value,source.metrics[field]?.value,`Exact replay ${field}`);if(source.state==='available')assert.deepEqual(row.limits,source.limits,'Quota replay normalized limits');}
 const artifact={status:'passed',fixture:'SYNTHETIC SQLite observations; actual built server, workers and authenticated HTTP routes',read_auth:true,mutation_auth_revision_masking:true,attention_reconciliation:true,capacity_forecast:'linear estimate and flat rejection',quota_capture_replay_rows:quotaRows,twilio:'disabled; all credential environment values cleared; private cwd contains no .env; no live send',checked_at:iso(Date.now())};await writeFile(path.join(output,'operators-http-verification.json'),JSON.stringify(artifact,null,2));
 if(serve){infoPath=path.join(output,'browser-info.json');await writeFile(infoPath,JSON.stringify({url,admin,viewer,objectIds:ids,from:iso(Date.now()-3*3600000),to:iso(Date.now()-5000),fixture:'SYNTHETIC QA only; quota values are captured fixture replays',pid:process.pid},null,2),{mode:0o600});console.log(JSON.stringify({status:'SYNTHETIC QA server ready',url,credentials_file:infoPath,expires_in_minutes:45}));await new Promise(resolve=>{const timer=setTimeout(resolve,45*60000);process.once('SIGINT',()=>{clearTimeout(timer);resolve();});process.once('SIGTERM',()=>{clearTimeout(timer);resolve();});});}
 else console.log(JSON.stringify(artifact));
}catch(error){console.error(`Operator smoke failed: ${error.message}`);process.exitCode=1;}finally{await cleanup();}
