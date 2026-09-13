# Disposable NFS quota fixture

Use only an isolated Linux VM/container with a quota-capable kernel. No host
exports or existing data are mounted. The container creates a 128 MiB ext4 loop
image, enables user quotas and runs real NFSv3/rquotad. It requires privilege
inside the VM to mount that image and nfsd, bounded to 1 CPU/256 MiB/128 PIDs.

Docker Desktop on the validation Mac lacked CONFIG_QFMT_V2. A separate Podman VM
worked. The Podman 5.7 base image is 10 GiB, so a smaller disk silently truncates the
image and fails to boot. Use a 12 GiB sparse disk (about 1.2 GiB initially allocated),
1 CPU and 1 GiB RAM. Existing VMs and unrelated services must remain unchanged.

Example, with a unique dedicated VM and an empty temporary share:

```sh
mkdir -p /tmp/cider-quota-empty
podman machine init --cpus 1 --memory 1024 --disk-size 12 --rootful \
  --volume /tmp/cider-quota-empty:/mnt/fixture-share:ro cider-nfs-quota-test
podman machine start cider-nfs-quota-test
podman machine ssh cider-nfs-quota-test sudo modprobe -a loop quota_v2
export CONTAINER_ENGINE=podman
export PODMAN_CONNECTION=cider-nfs-quota-test-root
export CIDERD_BIN="$PWD/target/debug/ciderd"
scripts/nfs-quota-fixture/run.sh up
scripts/nfs-quota-fixture/run.sh verify
scripts/nfs-quota-fixture/run.sh down
podman machine stop cider-nfs-quota-test
podman machine rm -f cider-nfs-quota-test
```

Do not substitute an existing personal VM for the dedicated fixture VM. Start
new containers after a VM reboot: stale privileged device mappings in a preserved
container may not include the recreated loop devices. `up` refuses an existing
container name; `down` checks the fixture label before stopping/removing it.

`verify` checks three real process identities and their distinct server quotas,
explicit zero/unlimited limits, permission denial, no-quota, NFS mount source,
and SHA256 of files read through the NFS mount. Optional CIDERD_BIN also checks
the production worker's real identity and loopback timeout/unavailable responses;
on the original UID 501 Mac it additionally asserts available/denied/no-quota.
No user identity is forged into RPC authentication. The fixture's root setup
sets only the three explicit test users' quotas. Both clients use GETQUOTA only.

Default output is ignored `.codex-staging/operator-workflows/nfs-quota-verification`.
Set QUOTA_FIXTURE_OUTPUT to preserve a new run separately, QUOTA_FIXTURE_PORT to
choose a different loopback UDP port, and QUOTA_FIXTURE_NAME to use another
unique container name. `down` saves logs, unmounts the private NFS client/export,
stops the namespace's nfsd and detaches its loop device. The image/cache can then
be removed only if it belongs to this fixture; never prune unrelated resources.
