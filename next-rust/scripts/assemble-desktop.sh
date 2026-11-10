#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/desktop/C-Paper Next.app"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp -X "$ROOT/desktop/target/debug/cpaper-desktop" "$APP/Contents/MacOS/cpaper-desktop"
cp -X "$ROOT/desktop/bundle/Info.plist" "$APP/Contents/Info.plist"
cp -X "$ROOT/../assets/icon.icns" "$APP/Contents/Resources/icon.icns"
plutil -lint "$APP/Contents/Info.plist"
echo "$APP"
