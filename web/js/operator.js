// Pure operator presentation helpers. Exact evidence never passes through Number.
export const human = value => typeof value === 'string' && value ? value.replaceAll('_',' ') : 'unknown';
export function measurementText(metric) {
  if (!metric || metric.value == null) return metric?.state ? `Unknown · ${human(metric.state)}` : 'Unknown';
  const value = typeof metric.value === 'string' || typeof metric.value === 'number' || typeof metric.value === 'boolean' ? String(metric.value) : 'Unknown';
  return `${value}${metric.unit ? ` ${metric.unit}` : ''} · ${metric.state || 'unknown'}`;
}
export function settingsPayload(current,draft) {
  if (!current?.revision) throw new Error('Refresh notification settings before editing.');
  const result={expected_revision:current.revision,enabled:draft.enabled===true};
  for(const key of ['sender','recipient']) {const phone=(draft[key] || '').trim();if(phone){if(!/^\+[1-9]\d{1,14}$/.test(phone))throw new Error(`Enter ${key} in international format, such as +15555550123.`);result[key]=phone;}}
  return result;
}
export function forecastModel(raw) {
  const date=raw?.state==='estimated' && Number.isFinite(Date.parse(raw.estimated_exhaustion_at)) && Number.isFinite(Date.parse(raw.scenario_range?.earliest_at)) && Number.isFinite(Date.parse(raw.scenario_range?.latest_at)) ? raw.estimated_exhaustion_at : null;
  return {state:date?'estimated':'unknown',date,range:date?raw.scenario_range:null,reasons:Array.isArray(raw?.reasons)&&raw.reasons.length?raw.reasons:date?[]:['forecast_unavailable'],basis:raw?.basis || {},assumptions:Array.isArray(raw?.assumptions)?raw.assumptions:[]};
}
export function historySeries(data) {
  const raw=data?.resolution==='raw';
  return (Array.isArray(data?.series)?data.series:[]).map((s,index)=>{
    const identity=s.identity || {}, unit=!raw&&identity.kind==='counter'?`${identity.unit || 'units'}/second`:identity.unit || 'units';
    const rows=(s.points || []).map(p=>({...p,at:p.observed_at || p.bucket_start,value:raw?p.value:p.mean,coverage:p.coverage?`${p.coverage.observed ?? '?'} / ${p.coverage.expected ?? '?'} observed samples`:'Not aggregated'}));
    const valid=rows.filter(p=>p.state==='ok' && p.value!=null && Number.isFinite(Date.parse(p.at)) && (typeof p.value==='number'&&Number.isFinite(p.value)||typeof p.value==='string'&&/^-?\d+(\.\d+)?$/.test(p.value)));
    const integer=valid.length>0&&valid.every(p=>/^-?\d+$/.test(String(p.value)));
    const values=valid.map(p=>integer?BigInt(p.value):Number(p.value));
    const min=values.reduce((a,b)=>a<b?a:b,values[0]),max=values.reduce((a,b)=>a>b?a:b,values[0]);
    const dates=rows.map(p=>Date.parse(p.at)).filter(Number.isFinite),start=Math.min(...dates),end=Math.max(...dates);
    const segments=[];let segment=[];const dots=[];
    for(const row of rows){if(!valid.includes(row)){if(segment.length)segments.push(segment);segment=[];continue;}
      if(row.gap_before&&segment.length){segments.push(segment);segment=[];}
      const value=integer?BigInt(row.value):Number(row.value);const range=integer?Number(max-min):max-min;const offset=integer?Number(value-min):value-min;
      const point={x:10+(Date.parse(row.at)-start)/Math.max(1,end-start)*580,y:140-(range?offset/range:.5)*125,at:row.at,value:String(row.value)};dots.push(point);segment.push(point);
    }
    if(segment.length)segments.push(segment);
    return {key:JSON.stringify(identity)+index,identity,unit,rows,dots,segments,minLabel:min==null?'Unknown':String(min),maxLabel:max==null?'Unknown':String(max),start:Number.isFinite(start)?new Date(start).toISOString():null,end:Number.isFinite(end)?new Date(end).toISOString():null};
  });
}
export function quotaRows(rows) {return (Array.isArray(rows)?rows:[]).map(row=>{
  const m=row.metrics || {}, current=row.state==='available';
  const field=(key)=>{const v=m[key] || m[`quota_${key}`];return v&&row.state&&!current?{...v,state:row.state}:v;};
  const limit=(key)=>{const v=row.limits?.[key];if(!current)return `Unknown · ${human(row.state)}`;if(v?.state==='unlimited')return 'No limit · available';return v?.state==='limited'?`${v.value} ${v.unit || ''} · available`:'Unknown';};
  return {...row,used:measurementText(field('used_bytes')),soft:limit('block_soft'),hard:limit('block_hard'),inodeUsed:measurementText(field('used_inodes')),inodeSoft:limit('inode_soft'),inodeHard:limit('inode_hard')};});}
/** Deliberate single attempt only. The caller clears the input before awaiting. */
export async function adminMutation(path,method,credential,payload,options={}) {
  const origin=options.origin ?? location.origin,url=new URL(path,origin);
  if(url.origin!==origin || !((method==='PUT'&&url.pathname==='/api/v1/notifications/settings')||(method==='POST'&&/^\/api\/v1\/attention\/[a-zA-Z0-9-]+\/acknowledgement$/.test(url.pathname))) || url.search || url.hash)throw new Error('Invalid administrator action destination.');
  if(!credential?.trim() || /[\r\n]/.test(credential))throw new Error('Enter an administrator credential for this action.');
  const encoded=JSON.stringify(payload);if(new TextEncoder().encode(encoded).length>8192)throw new Error('The action is too large.');
  const controller=new AbortController(),cancel=()=>controller.abort(),timer=setTimeout(cancel,10000);options.signal?.addEventListener('abort',cancel,{once:true});if(options.signal?.aborted)cancel();
  try {
    const response=await (options.fetchImpl || fetch)(url,{method,headers:{Authorization:`Bearer ${credential.trim()}`,'Content-Type':'application/json',Accept:'application/json','X-Request-ID':(options.randomUUID || (()=>crypto.randomUUID()))(),'X-Request-Timestamp':new Date((options.now || Date.now)()).toISOString()},body:encoded,cache:'no-store',credentials:'omit',redirect:'error',signal:controller.signal});
    const reader=response.body?.getReader();let size=0,parts=[];if(reader){for(;;){const {done,value}=await reader.read();if(done)break;size+=value.length;if(size>1_048_576){await reader.cancel();throw new Error('The server returned an oversized action response. Refresh to verify the result.');}parts.push(value);}}
    const bytes=new Uint8Array(size);let offset=0;for(const p of parts){bytes.set(p,offset);offset+=p.length;}let body;try{body=JSON.parse(new TextDecoder().decode(bytes));}catch{throw new Error('The action response was unreadable. Refresh to verify the result.');}
    if(!response.ok)throw new Error(response.status===409?'The observation or settings changed. Refresh and review before submitting again.':body?.error?.message || `Action returned HTTP ${response.status}.`);
    if(!body?.meta || !Object.hasOwn(body,'data'))throw new Error('The action response was incomplete. Refresh to verify the result.');return body;
  } catch(error) {if(error.name==='AbortError'||error instanceof TypeError)throw new Error('The action result is uncertain. Refresh and check its state before submitting again.');throw error;}
  finally {clearTimeout(timer);options.signal?.removeEventListener('abort',cancel);credential='';}
}
// Source age and read elapsed use their respective clock domains.
export function attentionFreshness(snapshot,wall=Date.now(),mono=globalThis.performance?.now?.() ?? 0) {
  const r=snapshot?.data?.reconciliation;
  if(!r)return 'unknown';
  if(r.state==='failed')return 'failed';
  if(snapshot.error)return 'stale';
  const sourceAge=Date.parse(snapshot.server_time)-Date.parse(r.last_successful_at);
  const elapsed=Math.max(0,wall-(snapshot.receivedWall ?? wall),mono-(snapshot.receivedMono ?? mono));
  if(r.state==='current'&&(!Number.isFinite(sourceAge)||sourceAge<0))return 'unknown';
  if(r.state==='current'&&(sourceAge+elapsed>(r.stale_after_seconds || 15)*1000))return 'stale';
  return r.state || 'unknown';
}
