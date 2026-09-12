#!/bin/bash
# Configure and start a generic NFS export on macOS.
#
# Idempotent: rewrites only its own marked block in /etc/exports, backs the file
# up first, validates with `nfsd checkexports`, and restores the backup if the
# new configuration does not parse.

set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/../lib/common.sh"

BEGIN_MARK="# >>> nfs-lab managed block >>>"
END_MARK="# <<< nfs-lab managed block <<<"
EXPORTS=/etc/exports

require_macos
require_root "$@"
load_config

[[ -n "${EXPORT_PATH:-}" ]] || die "EXPORT_PATH is not set in $NFSLAB_CONFIG_PATH"
[[ -d "$EXPORT_PATH" ]] || die "EXPORT_PATH does not exist or is not a directory: $EXPORT_PATH"

# --- Build the access-control clause -----------------------------------------
# Refuse to publish an export with no client restriction.

access=""
if [[ -n "${ALLOWED_NETWORK:-}" && -n "${ALLOWED_MASK:-}" ]]; then
  [[ -n "${ALLOWED_HOSTS:-}" ]] && die "set ALLOWED_NETWORK/ALLOWED_MASK or ALLOWED_HOSTS, not both"
  access="-network ${ALLOWED_NETWORK} -mask ${ALLOWED_MASK}"
elif [[ -n "${ALLOWED_HOSTS:-}" ]]; then
  access="${ALLOWED_HOSTS}"
else
  die "no client restriction configured. Set ALLOWED_NETWORK+ALLOWED_MASK or ALLOWED_HOSTS in $NFSLAB_CONFIG_PATH"
fi

mapping=""
case "${MAP_MODE:-}" in
  mapall)  [[ -n "${MAP_IDENTITY:-}" ]] || die "MAP_MODE=mapall requires MAP_IDENTITY"
           mapping="-mapall=${MAP_IDENTITY}" ;;
  maproot) [[ -n "${MAP_IDENTITY:-}" ]] || die "MAP_MODE=maproot requires MAP_IDENTITY"
           mapping="-maproot=${MAP_IDENTITY}" ;;
  "")      mapping="" ;;
  *)       die "MAP_MODE must be mapall, maproot, or empty (got: $MAP_MODE)" ;;
esac

export_line="${EXPORT_PATH} ${mapping} ${EXTRA_EXPORT_OPTS:-} ${access}"
export_line=$(printf '%s' "$export_line" | tr -s ' ')

# --- Rewrite /etc/exports ----------------------------------------------------

[[ -f "$EXPORTS" ]] || touch "$EXPORTS"
backup="${EXPORTS}.nfslab-backup.$(date +%Y%m%d%H%M%S)"
cp -p "$EXPORTS" "$backup"
info "backed up $EXPORTS -> $backup"

tmp=$(mktemp)
# Drop any previous managed block, keep everything else untouched.
awk -v b="$BEGIN_MARK" -v e="$END_MARK" '
  $0 == b { skip = 1; next }
  $0 == e { skip = 0; next }
  !skip   { print }
' "$EXPORTS" > "$tmp"

{
  printf '%s\n' "$BEGIN_MARK"
  printf '# generated %s by nfs-lab/bin/server-setup.sh\n' "$(date)"
  printf '%s\n' "$export_line"
  printf '%s\n' "$END_MARK"
} >> "$tmp"

cat "$tmp" > "$EXPORTS"
rm -f "$tmp"
chmod 644 "$EXPORTS"
info "wrote export: $export_line"

# --- Validate, rolling back on failure ---------------------------------------

if ! checkout=$(nfsd checkexports 2>&1); then
  cp -p "$backup" "$EXPORTS"
  printf '%s\n' "$checkout" >&2
  die "nfsd checkexports rejected the configuration; restored $backup"
fi
[[ -n "$checkout" ]] && printf '%s\n' "$checkout"

# --- Enable and (re)start ----------------------------------------------------

nfsd enable
if nfsd status >/dev/null 2>&1; then
  nfsd update
  info "nfsd running; exports reloaded"
else
  nfsd start
  info "nfsd started"
fi

sleep 1
info ""
info "exports visible to clients:"
showmount -e localhost || info "(showmount returned no exports yet; give nfsd a moment and retry)"
