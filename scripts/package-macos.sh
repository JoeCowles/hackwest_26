#!/bin/bash
set -euo pipefail
CIDER_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$CIDER_ROOT"
if [ "$(uname -s)" != Darwin ]; then
  printf '%s\n' 'Build the macOS bundle on a Mac.' >&2
  exit 1
fi
CIDER_PROFILE="${1:-release}"
if [ "$CIDER_PROFILE" = release ]; then
  bash scripts/cargo-local.sh build --release --package cider-server
elif [ "$CIDER_PROFILE" = debug ]; then
  bash scripts/cargo-local.sh build --package cider-server
else
  printf '%s\n' 'Usage: bash scripts/package-macos.sh [release|debug]' >&2
  exit 1
fi
CIDER_BUNDLE="$CIDER_ROOT/dist/Cider Server.app"
mkdir -p "$CIDER_BUNDLE/Contents/MacOS" "$CIDER_BUNDLE/Contents/Resources"
cp "target/$CIDER_PROFILE/cider-server" "$CIDER_BUNDLE/Contents/MacOS/cider-server"
cp macos/Info.plist "$CIDER_BUNDLE/Contents/Info.plist"
codesign --force --sign - "$CIDER_BUNDLE"
printf 'Built %s\n' "$CIDER_BUNDLE"
