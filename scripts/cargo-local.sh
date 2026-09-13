#!/bin/bash
set -euo pipefail
CIDER_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if command -v cargo >/dev/null 2>&1; then
  exec cargo "$@"
fi
export CARGO_HOME="$CIDER_ROOT/.codex-staging/cargo"
export RUSTUP_HOME="$CIDER_ROOT/.codex-staging/rustup"
if [ ! -x "$CARGO_HOME/bin/cargo" ]; then
  printf '%s\n' 'Rust is required. Install it using https://rustup.rs and retry.' >&2
  exit 1
fi
exec "$CARGO_HOME/bin/cargo" "$@"
