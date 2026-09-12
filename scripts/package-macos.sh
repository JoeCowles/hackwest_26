#!/bin/bash
set -euo pipefail
ORCHARD_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ORCHARD_ROOT"
if [ "$(uname -s)" != Darwin ]; then
  printf '%s\n' 'Build the macOS bundle on a Mac.' >&2
  exit 1
fi
ORCHARD_PROFILE="${1:-release}"
if [ "$ORCHARD_PROFILE" = release ]; then
  bash scripts/cargo-local.sh build --release --package orchard-server
elif [ "$ORCHARD_PROFILE" = debug ]; then
  bash scripts/cargo-local.sh build --package orchard-server
else
  printf '%s\n' 'Usage: bash scripts/package-macos.sh [release|debug]' >&2
  exit 1
fi
ORCHARD_BUNDLE="$ORCHARD_ROOT/dist/Orchard Server.app"
mkdir -p "$ORCHARD_BUNDLE/Contents/MacOS" "$ORCHARD_BUNDLE/Contents/Resources"
cp "target/$ORCHARD_PROFILE/orchard-server" "$ORCHARD_BUNDLE/Contents/MacOS/orchard-server"
cp macos/Info.plist "$ORCHARD_BUNDLE/Contents/Info.plist"
codesign --force --sign - "$ORCHARD_BUNDLE"
printf 'Built %s\n' "$ORCHARD_BUNDLE"
