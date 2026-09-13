#!/usr/bin/env python3
"""Exercise the production macOS worker against only the isolated loopback fixture."""
import datetime,json,os,pathlib,socket,subprocess,sys
binary,port,output=sys.argv[1],int(sys.argv[2]),pathlib.Path(sys.argv[3])
def query(uid,path,port,timeout=2):
    request={'operation':'nfs-quota','target':{'server':'127.0.0.1','export_path':path,'uid':uid,'port':port},'timeout_seconds':timeout}
    r=json.loads(subprocess.check_output([binary,'worker'],input=json.dumps(request),text=True,timeout=timeout+3))
    assert r['query_identity_uid']==str(os.getuid())
    return {'request':request,'observed_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'result':r}
results=[query(501,'/exports/quota',port),query(502,'/exports/quota',port),query(501,'/no-such-export',port)]
if os.getuid()==501:
    assert [r['result']['status'] for r in results]==['available','permission_denied','no_quota']
    assert results[0]['result']['used_bytes']=='66560'
with socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as sock:
    sock.bind(('127.0.0.1',0));dead_port=sock.getsockname()[1]
    r=query(501,'/exports/quota',dead_port,1);assert r['result']['status']=='timeout';results.append(r)
r=query(501,'/exports/quota',dead_port,1);assert r['result']['status']=='unavailable';results.append(r)
(output/'production-worker.json').write_text(json.dumps(results,indent=2)+'\n')
print('PASS: production worker preserves real process identity and isolated UDP timeout/unavailable outcomes')
