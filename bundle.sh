#!/bin/bash
# Builds a release binary and wraps it, with the WireGuard tools from
# scripts/deps.sh, in dist/Wirus.app.
set -euo pipefail
cd "$(dirname "$0")"
./scripts/deps.sh
cargo build --release
APP=dist/Wirus.app
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/bin"
cp target/release/wirus "$APP/Contents/MacOS/wirus"
cp -p .deps/out/bin/* "$APP/Contents/Resources/bin/"
cp assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Wirus</string>
  <key>CFBundleDisplayName</key><string>Wirus</string>
  <key>CFBundleIdentifier</key><string>com.github.havban.wirus</string>
  <key>CFBundleExecutable</key><string>wirus</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
echo "==> Built $APP"
