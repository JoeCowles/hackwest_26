#!/bin/bash
# Install, remove, or inspect the NFS monitor as a launchd LaunchDaemon.
#
#   daemon-ctl.sh install
#   daemon-ctl.sh uninstall
#   daemon-ctl.sh status

set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/../lib/common.sh"

LABEL="local.nfs-lab.monitor"
PLIST="/Library/LaunchDaemons/${LABEL}.plist"
MONITOR="${NFSLAB_ROOT}/daemon/nfs-monitor.sh"
LOG_DIR="/usr/local/var/log/nfs-monitor"

cmd="${1:-}"

case "$cmd" in
  install)
    require_macos
    require_root "$@"
    [[ -x "$MONITOR" ]] || die "monitor not executable: $MONITOR (run: chmod +x $MONITOR)"
    [[ -f "${NFSLAB_ROOT}/config.env" ]] || die "no config.env; copy config.env.example first"

    mkdir -p "$LOG_DIR"

    cat >"$PLIST" <<PLISTEOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/bash</string>
    <string>${MONITOR}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>${LOG_DIR}/stdout.log</string>
  <key>StandardErrorPath</key>
  <string>${LOG_DIR}/stderr.log</string>
</dict>
</plist>
PLISTEOF

    chown root:wheel "$PLIST"
    chmod 644 "$PLIST"
    info "wrote $PLIST"

    # Replace any previous instance before loading.
    launchctl bootout "system/${LABEL}" 2>/dev/null || true
    launchctl bootstrap system "$PLIST"
    info "loaded ${LABEL}"
    info "logs: ${LOG_DIR}/"
    ;;

  uninstall)
    require_macos
    require_root "$@"
    launchctl bootout "system/${LABEL}" 2>/dev/null || info "was not loaded"
    if [[ -f "$PLIST" ]]; then
      rm -f "$PLIST"
      info "removed $PLIST"
    fi
    info "logs left in place at ${LOG_DIR}/"
    ;;

  status)
    if launchctl print "system/${LABEL}" >/dev/null 2>&1; then
      info "loaded:"
      launchctl print "system/${LABEL}" | grep -E '^\s+(state|pid|last exit) ' || true
    else
      info "not loaded"
    fi
    [[ -f "$PLIST" ]] && info "plist present: $PLIST" || info "plist absent"
    ;;

  *)
    die "usage: $0 {install|uninstall|status}"
    ;;
esac
