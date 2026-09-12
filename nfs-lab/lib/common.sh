#!/bin/bash
# Shared helpers. Sourced, not executed.

NFSLAB_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export NFSLAB_ROOT

die() { printf 'error: %s\n' "$*" >&2; exit 1; }
info() { printf '%s\n' "$*"; }

require_root() {
  [[ $EUID -eq 0 ]] || die "must run as root: sudo $0 $*"
}

require_macos() {
  [[ "$(uname -s)" == "Darwin" ]] || die "macOS only (found $(uname -s))"
}

load_config() {
  local cfg="${NFSLAB_CONFIG:-$NFSLAB_ROOT/config.env}"
  [[ -f "$cfg" ]] || die "no config at $cfg (copy config.env.example to config.env)"
  # shellcheck disable=SC1090
  source "$cfg"
  NFSLAB_CONFIG_PATH="$cfg"
}

# Minimal JSON string escaping: backslash, quote, and control characters.
json_escape() {
  local s=$1
  s=${s//\\/\\\\}
  s=${s//\"/\\\"}
  s=${s//$'\t'/\\t}
  s=${s//$'\r'/\\r}
  s=${s//$'\n'/\\n}
  printf '%s' "$s"
}
