#!/bin/bash
# Mount the cluster export on a client node.
#
# Reports the options the client actually negotiated, which can differ from what
# was requested - macOS will silently fall back on version or transport.

set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/../lib/common.sh"

require_macos
require_root "$@"
load_config

[[ -n "${NFS_SERVER:-}" ]] || die "NFS_SERVER is not set in $NFSLAB_CONFIG_PATH"
[[ -n "${EXPORT_PATH:-}" ]] || die "EXPORT_PATH is not set in $NFSLAB_CONFIG_PATH"
[[ -n "${MOUNT_POINT:-}" ]] || die "MOUNT_POINT is not set in $NFSLAB_CONFIG_PATH"

if mount -t nfs | grep -qF " on ${MOUNT_POINT} "; then
  info "already mounted at $MOUNT_POINT:"
  mount -t nfs | grep -F " on ${MOUNT_POINT} "
  exit 0
fi

mkdir -p "$MOUNT_POINT"

opts="vers=${NFS_VERS:-3}"
[[ -n "${MOUNT_OPTS:-}" ]] && opts="${opts},${MOUNT_OPTS}"

info "mounting ${NFS_SERVER}:${EXPORT_PATH} -> ${MOUNT_POINT} (-o ${opts})"
mount -t nfs -o "$opts" "${NFS_SERVER}:${EXPORT_PATH}" "$MOUNT_POINT"

info ""
info "negotiated mount:"
mount -t nfs | grep -F " on ${MOUNT_POINT} " || true
info ""
info "capacity:"
df -h "$MOUNT_POINT"
info ""
info "to unmount: sudo umount ${MOUNT_POINT}"
