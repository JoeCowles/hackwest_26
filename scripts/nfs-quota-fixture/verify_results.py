#!/usr/bin/env python3
"""Check independent server/UID accounting and actual NFS file reads."""
import hashlib,json,pathlib,sys
root=pathlib.Path(sys.argv[1])
for uid,used,soft,hard,isoft,ihard in [(501,66560,1048576,2097152,10,20),(502,132096,4194304,8388608,30,40),(503,1024,0,0,0,0)]:
    row=json.loads((root/f'user-{uid}.json').read_text())
    assert row['process_uid']==row['uid']==uid
    assert row['status']=='available' and row['active'] is True
    for key,value in [('used_bytes',used),('block_soft_limit_bytes',soft),('block_hard_limit_bytes',hard),('inode_soft_limit',isoft),('inode_hard_limit',ihard)]:assert row[key]==str(value),(uid,key,row)
assert json.loads((root/'denied.json').read_text())['status']=='permission_denied'
assert json.loads((root/'no-quota.json').read_text())['status']=='no_quota'
assert (root/'nfs-source.txt').read_text().startswith('127.0.0.1:/exports/quota nfs')
for uid,size in [(501,65536),(502,131072)]:
    expected=hashlib.sha256(bytes(size)).hexdigest()
    assert (root/f'nfs-user-{uid}-sha256.txt').read_text().split()[0]==expected
print('PASS: real user quotas, distinct UID limits, explicit unlimited, denied/no-quota and NFS-mounted payload reads')
