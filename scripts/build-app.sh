#!/bin/bash
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$PROJECT_ROOT"
in_place=false
if [ "${1:-}" = "--install" ]; then in_place=true; shift; fi
if $in_place && pgrep -f '^/Applications/QuietType[.]app/Contents/MacOS/ai-quick-input$' >/dev/null; then
  printf '请先退出 QuietType，再原地更新。\n' >&2
  exit 1
fi
bash scripts/build-swift-libs.sh
if $in_place; then npm run tauri -- build --no-bundle "$@"
else npm run tauri -- build --bundles app "$@"; fi
profile=release
for arg in "$@"; do
  if [ "$arg" = "--debug" ]; then profile=debug; fi
done
APP="$PROJECT_ROOT/src-tauri/target/$profile/bundle/macos/QuietType.app"
if $in_place; then
  APP="/Applications/QuietType.app"
  test -d "$APP/Contents/Resources"
  install -m 755 "src-tauri/target/$profile/ai-quick-input" "$APP/Contents/MacOS/ai-quick-input"
  install -m 755 lib/swift/build/*.dylib "$APP/Contents/Resources/"
  install -m 644 src-tauri/icons/icon.icns "$APP/Contents/Resources/icon.icns"
  install -m 644 sidecar/local_speech/server.py sidecar/local_speech/vocabulary.py "$APP/Contents/Resources/local_speech/"
  install -m 644 release/runtime-download.json "$APP/Contents/Resources/runtime-download.json"
  mkdir -p "$APP/Contents/Resources/sounds"
  install -m 644 assets/sounds/*.wav "$APP/Contents/Resources/sounds/"
fi
mkdir -p "$APP/Contents/Frameworks"
ditto vendor/Sparkle/Sparkle.framework "$APP/Contents/Frameworks/Sparkle.framework"
if $in_place; then python3 scripts/sync-bundle-metadata.py "$APP/Contents/Info.plist"; fi
# A persistent private local identity keeps permissions compatible across updates.
# Public distribution requires a Developer ID certificate and notarization.
python3 scripts/sign-app.py "$APP"
printf 'Built and verified: %s\n' "$APP"
