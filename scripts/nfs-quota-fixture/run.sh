#!/bin/bash
# Dedicated disposable fixture only. Never mount host data or alter host exports.
set -euo pipefail
fixture_dir="$(cd "$(dirname "$0")" && pwd)"
project_dir="$(cd "$fixture_dir/../.." && pwd)"
fixture_name="${QUOTA_FIXTURE_NAME:-cider-nfs-quota-test}"
fixture_image="cider-nfs-quota-fixture:local"
fixture_port="${QUOTA_FIXTURE_PORT:-18751}"
fixture_output="${QUOTA_FIXTURE_OUTPUT:-$project_dir/.codex-staging/operator-workflows/nfs-quota-verification}"
fixture_engine=("${CONTAINER_ENGINE:-docker}")
if [ -n "${PODMAN_CONNECTION:-}" ]; then fixture_engine+=(--connection "$PODMAN_CONNECTION"); fi
own_container() {
  # Existing explicitly selected fixtures retain their original ownership label.
  [ "$("${fixture_engine[@]}" inspect -f '{{index .Config.Labels "cider.fixture"}}' "$fixture_name")" = nfs-quota ] ||
    [ "$("${fixture_engine[@]}" inspect -f '{{index .Config.Labels "orchard.fixture"}}' "$fixture_name")" = nfs-quota ] || { echo 'Refusing an unrelated container' >&2; exit 1; }
}
case "${1:-}" in
up)
  if "${fixture_engine[@]}" inspect "$fixture_name" >/dev/null 2>&1; then echo 'Named container already exists; inspect it or run down first.' >&2; exit 1; fi
  "${fixture_engine[@]}" build -t "$fixture_image" "$fixture_dir"
  "${fixture_engine[@]}" run -d --name "$fixture_name" --label cider.fixture=nfs-quota \
    --privileged --memory=256m --cpus=1 --pids-limit=128 \
    -p "127.0.0.1:$fixture_port:875/udp" "$fixture_image"
  for ((attempt=0;attempt<30;attempt++)); do
    if "${fixture_engine[@]}" logs "$fixture_name" 2>&1 | grep -q '^CIDER_NFS_QUOTA_READY$'; then exit 0; fi
    if [ "$("${fixture_engine[@]}" inspect -f '{{.State.Running}}' "$fixture_name")" != true ]; then "${fixture_engine[@]}" logs "$fixture_name"; exit 1; fi
    sleep 1
  done
  echo 'Fixture did not become ready within30 seconds; run down to clean up.' >&2; exit 1
  ;;
verify)
  own_container
  mkdir -p "$fixture_output"
  "${fixture_engine[@]}" exec "$fixture_name" repquota -uv /exports/quota > "$fixture_output/repquota.txt"
  for fixture_uid in 501 502 503; do
    "${fixture_engine[@]}" exec "$fixture_name" setpriv "--reuid=$fixture_uid" "--regid=$fixture_uid" --clear-groups python3 /verify.py "$fixture_uid" > "$fixture_output/user-$fixture_uid.json"
  done
  "${fixture_engine[@]}" exec "$fixture_name" setpriv --reuid=501 --regid=501 --clear-groups python3 /verify.py 502 > "$fixture_output/denied.json"
  "${fixture_engine[@]}" exec "$fixture_name" setpriv --reuid=501 --regid=501 --clear-groups python3 /verify.py 501 /no-such-export > "$fixture_output/no-quota.json"
  "${fixture_engine[@]}" exec "$fixture_name" mkdir -p /mnt/client
  if ! "${fixture_engine[@]}" exec "$fixture_name" mountpoint -q /mnt/client; then
    "${fixture_engine[@]}" exec "$fixture_name" mount -t nfs -o vers=3,proto=tcp,port=2049,mountport=20048,nolock 127.0.0.1:/exports/quota /mnt/client
  fi
  "${fixture_engine[@]}" exec "$fixture_name" findmnt -n -o SOURCE,FSTYPE,OPTIONS /mnt/client > "$fixture_output/nfs-source.txt"
  "${fixture_engine[@]}" exec "$fixture_name" setpriv --reuid=501 --regid=501 --clear-groups sha256sum /mnt/client/user-501/payload > "$fixture_output/nfs-user-501-sha256.txt"
  "${fixture_engine[@]}" exec "$fixture_name" setpriv --reuid=502 --regid=502 --clear-groups sha256sum /mnt/client/user-502/payload > "$fixture_output/nfs-user-502-sha256.txt"
  python3 "$fixture_dir/verify_results.py" "$fixture_output"
  if [ -n "${CIDERD_BIN:-}" ]; then python3 "$fixture_dir/verify_worker.py" "$CIDERD_BIN" "$fixture_port" "$fixture_output"; fi
  ;;
down)
  own_container
  mkdir -p "$fixture_output"
  "${fixture_engine[@]}" stop --time 5 "$fixture_name"
  "${fixture_engine[@]}" logs "$fixture_name" > "$fixture_output/container.log" 2>&1
  "${fixture_engine[@]}" rm "$fixture_name"
  ;;
*) echo 'Usage: run.sh up|verify|down. Set CONTAINER_ENGINE, optionally PODMAN_CONNECTION, CIDERD_BIN, and QUOTA_FIXTURE_OUTPUT.' >&2; exit 2 ;;
esac
