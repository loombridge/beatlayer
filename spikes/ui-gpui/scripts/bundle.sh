#!/bin/sh
# Builds the spike in release mode and wraps it in a minimal .app bundle, so
# macOS registers it with LaunchServices when it's started with `open`.
# Accessibility tools (Accessibility Inspector and others) and
# VoiceOver expect a registered app.
#
# Usage: scripts/bundle.sh
# Then:  open -n target/release/BeatlayerGpuiSpike.app --args [spike options]
set -eu
cd "$(dirname "$0")/.."
cargo build --release
app=target/release/BeatlayerGpuiSpike.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp target/release/bl-spike-ui-gpui "$app/Contents/MacOS/bl-spike-ui-gpui"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>io.loombridge.beatlayer.spike.gpui</string>
  <key>CFBundleName</key><string>Beatlayer GPUI spike</string>
  <key>CFBundleExecutable</key><string>bl-spike-ui-gpui</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.0.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
</dict>
</plist>
PLIST
echo "$app"
