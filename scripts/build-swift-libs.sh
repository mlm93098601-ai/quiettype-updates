#!/bin/bash
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SWIFT_DIR="$PROJECT_ROOT/lib/swift"
BUILD_DIR="$SWIFT_DIR/build"
mkdir -p "$BUILD_DIR"

swiftc -module-cache-path "$BUILD_DIR/module-cache" -emit-library -Osize \
  -target arm64-apple-macosx13.0 -module-name UtilHelper \
  "$SWIFT_DIR/UtilHelper/UtilHelper.swift" "$SWIFT_DIR/UtilHelper/Installation.swift" -framework ApplicationServices -framework AVFoundation \
  -o "$BUILD_DIR/libUtilHelper.dylib"
swiftc -module-cache-path "$BUILD_DIR/module-cache" -emit-library -Osize \
  -target arm64-apple-macosx13.0 -module-name InputHelper \
  "$SWIFT_DIR/InputHelper/InputHelper.swift" -framework ApplicationServices -framework AppKit \
  -o "$BUILD_DIR/libInputHelper.dylib"
swiftc -module-cache-path "$BUILD_DIR/module-cache" -emit-library -Osize \
  -target arm64-apple-macosx13.0 -module-name NativeUI \
  "$SWIFT_DIR/NativeUI/NativeUI.swift" "$SWIFT_DIR/NativeUI/Updater.swift" \
  -F "$PROJECT_ROOT/vendor/Sparkle" -framework Sparkle \
  -Xlinker -rpath -Xlinker @loader_path/../Frameworks \
  -framework AppKit -framework SwiftUI \
  -o "$BUILD_DIR/libNativeUI.dylib"
