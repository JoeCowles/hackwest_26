#!/bin/bash
# Remove the nfs-lab export from /etc/exports.
#
# Only the marked block is removed; any hand-written exports are preserved.
# nfsd is left running unless --stop-nfsd is passed, because other exports may
# still depend on it.

set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/../lib/common.sh"

BEGIN_MARK="# >>> nfs-lab managed block >>>"
END_MARK="# <<< nfs-lab managed block <<<"
EXPORTS=/etc/exports

stop_nfsd=0
[[ "${1:-}" == "--stop-nfsd" ]] && stop_nfsd=1

require_macos
require_root "$@"

[[ -f "$EXPORTS" ]] || { info "$EXPORTS does not exist; nothing to do"; exit 0; }

if ! grep -qF "$BEGIN_MARK" "$EXPORTS"; then
  info "no nfs-lab block found in $EXPORTS; nothing to remove"
else
  backup="${EXPORTS}.nfslab-backup.$(date +%Y%m%d%H%M%S)"
  cp -p "$EXPORTS" "$backup"
  info "backed up $EXPORTS -> $backup"

  tmp=$(mktemp)
  awk -v b="$BEGIN_MARK" -v e="$END_MARK" '
    $0 == b { skip = 1; next }
    $0 == e { skip = 0; next }
    !skip   { print }
  ' "$EXPORTS" > "$tmp"
  cat "$tmp" > "$EXPORTS"
  rm -f "$tmp"
  info "removed nfs-lab block from $EXPORTS"

  if nfsd status >/dev/null 2>&1; then
    nfsd update
    info "exports reloaded"
  fi
fi

remaining=$(grep -vcE '^\s*(#|$)' "$EXPORTS" || true)
info "remaining export lines in $EXPORTS: $remaining"

if [[ $stop_nfsd -eq 1 ]]; then
  if [[ "$remaining" -gt 0 ]]; then
    info "refusing --stop-nfsd: $remaining other export line(s) still present"
  else
    nfsd stop
    nfsd disable
    info "nfsd stopped and disabled"
  fi
fi
