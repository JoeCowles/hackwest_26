#!/bin/sh
set -eu
# All writes/mounts are confined to this disposable container and its private loop image.
mkdir -p /fixture /exports/quota /run/rpcbind /var/lib/nfs/rpc_pipefs
truncate -s 128M /fixture/quota.img
mkfs.ext4 -q -O quota /fixture/quota.img
loop=$(losetup --find --show /fixture/quota.img)
rquotad_pid=""
nfs_started=0
cleanup() {
    if [ -n "$rquotad_pid" ]; then kill "$rquotad_pid" 2>/dev/null || true; fi
    if mountpoint -q /mnt/client; then umount /mnt/client || true; fi
    exportfs -au || true
    if [ "$nfs_started" = 1 ]; then rpc.nfsd 0 || true; fi
    if mountpoint -q /exports/quota; then umount /exports/quota || true; fi
    losetup -d "$loop" || true
}
trap cleanup EXIT
trap 'exit 0' INT TERM
mount -t ext4 -o usrquota "$loop" /exports/quota

printf '/exports/quota 127.0.0.1(rw,sync,no_subtree_check,no_root_squash)\n' >/etc/exports
printf '%s /exports/quota ext4 defaults,usrquota 0 0\n' "$loop" >/etc/fstab
rpcbind -w
setquota -u 501 1024 2048 10 20 /exports/quota
setquota -u 502 4096 8192 30 40 /exports/quota
setquota -u 503 0 0 0 0 /exports/quota
for uid in 501 502 503; do mkdir /exports/quota/user-$uid; chown "$uid:$uid" /exports/quota/user-$uid; done
# Distinct allocated usage owned by configured test subjects, independent of quotas client.
dd if=/dev/zero of=/exports/quota/user-501/payload bs=1024 count=64 status=none
chown 501:501 /exports/quota/user-501/payload
dd if=/dev/zero of=/exports/quota/user-502/payload bs=1024 count=128 status=none
chown 502:502 /exports/quota/user-502/payload
sync
mount -t nfsd nfsd /proc/fs/nfsd
rpc.nfsd --no-nfs-version 4 2
nfs_started=1
rpc.mountd --no-nfs-version 4 --port 20048
exportfs -ra
rpc.rquotad -F -p 875 &
rquotad_pid=$!
repquota -uv /exports/quota
printf 'CIDER_NFS_QUOTA_READY\n'
wait "$rquotad_pid"
