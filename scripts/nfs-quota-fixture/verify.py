#!/usr/bin/env python3
"""Independent test-only rquota reader: always uses the process's real identity."""
import json, os, socket, struct, sys, time

def words(*xs): return b"".join(struct.pack("!I", x) for x in xs)
def opaque(v): return words(len(v)) + v + bytes((-len(v)) % 4)
def query(uid, path="/exports/quota"):
    xid = int.from_bytes(os.urandom(4), "big")
    identity = words(int(time.time()),) + opaque(socket.gethostname().encode())
    groups = os.getgroups()[:16]
    identity += words(os.getuid(), os.getgid(), len(groups), *groups)
    request = words(xid, 0, 2, 100011, 1, 1, 1) + opaque(identity) + words(0, 0)
    request += opaque(path.encode()) + words(uid)
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
        s.settimeout(2)
        s.connect(("127.0.0.1", 875))
        s.send(request)
        raw = s.recv(4096)
    result = {"process_uid":os.getuid(),"process_gid":os.getgid(),"uid":uid,
              "server":"127.0.0.1","export_path":path,"reply_hex":raw.hex(),"xid":xid}
    data = struct.unpack("!" + "I" * (len(raw)//4), raw)
    assert data[:3] == (xid, 1, 0), data
    length = data[4]
    offset = 5 + (length+3)//4
    assert data[offset] == 0, data
    status = data[offset+1]
    result["status"] = {1:"available",2:"no_quota",3:"permission_denied"}[status]
    if status == 1:
        block, active, hard, soft, used, ihard, isoft, iused, bgrace, igrace = data[offset+2:]
        result.update(active=bool(active), block_size_bytes=str(block), used_bytes=str(used*block),
                      block_soft_limit_bytes=str(soft*block), block_hard_limit_bytes=str(hard*block),
                      used_inodes=str(iused), inode_soft_limit=str(isoft), inode_hard_limit=str(ihard),
                      block_grace_seconds_raw=str(bgrace), inode_grace_seconds_raw=str(igrace))
    return result
if __name__ == "__main__":
    uid=int(sys.argv[1])
    print(json.dumps(query(uid,sys.argv[2] if len(sys.argv)>2 else "/exports/quota"),sort_keys=True))
