#!/bin/bash
# Generic NFS monitoring daemon.
#
# Samples every NFS mount on this host and emits one JSON object per sample as
# newline-delimited JSON. Optionally POSTs each sample to METRICS_ENDPOINT.
#
#   nfs-monitor.sh --once     take a single sample, print to stdout, exit
#   nfs-monitor.sh            loop forever at SAMPLE_INTERVAL
#
# Every probe is time-bounded. A hung NFS mount blocks df and stat indefinitely,
# so an unbounded probe would wedge the daemon exactly when it has something
# interesting to report.

set -uo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/../lib/common.sh"

PROBE_TIMEOUT="${PROBE_TIMEOUT:-5}"

once=0
[[ "${1:-}" == "--once" ]] && once=1

load_config

# Run a command with a wall-clock bound. Returns 124 on timeout.
run_bounded() {
  local secs=$1; shift
  local tmp pid waited rc
  tmp=$(mktemp)
  "$@" >"$tmp" 2>/dev/null &
  pid=$!
  waited=0
  while kill -0 "$pid" 2>/dev/null; do
    if [[ $waited -ge $((secs * 10)) ]]; then
      kill -9 "$pid" 2>/dev/null
      wait "$pid" 2>/dev/null
      rm -f "$tmp"
      return 124
    fi
    sleep 0.1
    waited=$((waited + 1))
  done
  wait "$pid"; rc=$?
  cat "$tmp"
  rm -f "$tmp"
  return $rc
}

# Emit one mount object. Fields that could not be read are null with a state,
# never zero - an unreadable mount is not an empty one.
mount_json() {
  local source=$1 mountpoint=$2 opts=$3
  local df_out rc state cap_total cap_used cap_avail

  df_out=$(run_bounded "$PROBE_TIMEOUT" df -k "$mountpoint")
  rc=$?

  if [[ $rc -eq 124 ]]; then
    state="not_responding"
  elif [[ $rc -ne 0 ]]; then
    state="error"
  else
    state="ok"
    read -r _ cap_total cap_used cap_avail _ <<<"$(printf '%s\n' "$df_out" | awk 'NR==2')"
  fi

  printf '{"source":"%s","mount_point":"%s","options":"%s","state":"%s"' \
    "$(json_escape "$source")" "$(json_escape "$mountpoint")" \
    "$(json_escape "$opts")" "$state"

  if [[ "$state" == "ok" && -n "${cap_total:-}" ]]; then
    printf ',"capacity_kb":%s,"used_kb":%s,"available_kb":%s' \
      "$cap_total" "$cap_used" "$cap_avail"
  else
    printf ',"capacity_kb":null,"used_kb":null,"available_kb":null'
  fi
  printf '}'
}

# NFS RPC counters, embedded verbatim from `nfsstat -f JSON`.
#
# These are per-host, not per-mount, so they are reported once per sample rather
# than duplicated onto every mount. The full per-operation breakdown is kept
# rather than a summary: the ratio between Read, Write, Remove and Rename is the
# signal that separates bulk copy from mass deletion from encryption-in-place,
# and collapsing it here would throw that away before anything can use it.
#
# `-c` is the client view, `-s` the server view. A node that both exports and
# mounts reports both; each is null where nfsd or the client is not active.
nfsstat_json() {
  local flag=$1 out
  out=$(run_bounded "$PROBE_TIMEOUT" nfsstat "$flag" -f JSON)
  if [[ $? -ne 0 || -z "$out" ]]; then
    printf 'null'
    return
  fi
  # nfsstat already emits well-formed JSON; splice it in rather than reparse it.
  printf '%s' "$out"
}

sample() {
  local ts host first=1
  ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  host=$(hostname)

  printf '{"ts":"%s","host":"%s","mounts":[' "$ts" "$(json_escape "$host")"

  while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    local source mountpoint opts
    source=${line%% on *}
    mountpoint=${line#* on }
    mountpoint=${mountpoint%% (*}
    opts=${line#*(}
    opts=${opts%)}

    [[ $first -eq 1 ]] || printf ','
    first=0
    mount_json "$source" "$mountpoint" "$opts"
  done < <(mount -t nfs 2>/dev/null)

  printf '],"nfs_client":%s,"nfs_server":%s}\n' \
    "$(nfsstat_json -c)" "$(nfsstat_json -s)"
}

publish() {
  local json=$1
  if [[ -n "${METRICS_LOG:-}" ]]; then
    mkdir -p "$(dirname "$METRICS_LOG")"
    printf '%s\n' "$json" >>"$METRICS_LOG"
  fi
  if [[ -n "${METRICS_ENDPOINT:-}" ]]; then
    printf '%s' "$json" | curl -sf --max-time 10 \
      -H 'Content-Type: application/json' \
      --data-binary @- "$METRICS_ENDPOINT" >/dev/null \
      || printf 'warn: POST to %s failed\n' "$METRICS_ENDPOINT" >&2
  fi
}

if [[ $once -eq 1 ]]; then
  sample
  exit 0
fi

trap 'exit 0' TERM INT
while true; do
  json=$(sample)
  printf '%s\n' "$json"
  publish "$json"
  sleep "${SAMPLE_INTERVAL:-30}"
done
