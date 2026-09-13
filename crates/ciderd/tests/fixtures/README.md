These fixtures exercise the pure source adapters without requiring devices or NFS mounts.

* `nfsstat-client.json` is the structure captured from `/usr/bin/nfsstat -f JSON -c` on macOS on 2026-09-12. The host had zero client counters. It contains no identities, addresses, or paths. The v4.1/NLM/paging and callback RPC tables are present to verify that supplementary tables do not silently mix with the baseline v3/v4 families.
* Diskutil physical/APFS/snapshot XML plists reproduce observed macOS output structure with synthetic device sizes, UUIDs, names, and snapshot data. Only `diskutil list -plist physical` is accepted as physical-discovery input; the ordinary list output does not prove physical provenance.
* `iokit.plist` represents normalized worker output, including a u64 maximum counter and deliberately absent unsupported counters.
* `mounts.json` contains synthetic native worker records with an exact capacity product above the IEEE-754 integer range. No actual user mount paths are retained.
* `smart-nvme.json` is synthetic smartctl 7.5 output, with controller NSID -1, a valid health exit bit, Celsius -2, endurance above 100, a u128 maximum numeric counter, and a wide integer string companion.

Native NSTATUS declarations and field offsets come from installed SDK headers at build time. Apple source references used to confirm query dispatch and semantics: https://github.com/apple-oss-distributions/xnu/blob/main/bsd/vfs/vfs_subr.c and https://github.com/apple-oss-distributions/NFS/blob/main/kext/nfs_vfsops.c. SMART NSID and Celsius handling follows https://github.com/smartmontools/smartmontools/blob/master/smartmontools/nvmeprint.cpp.
