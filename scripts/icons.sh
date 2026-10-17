#!/bin/bash
# Regenerates the app icon and logo PNGs from the SVGs in assets/.
set -euo pipefail
cd "$(dirname "$0")/../assets"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
swiftc -O -o "$TMP/render" ../scripts/render-svg.swift
SET="$TMP/AppIcon.iconset"; mkdir "$SET"
for s in 16 32 128 256 512; do
    "$TMP/render" icon.svg "$SET/icon_${s}x${s}.png" $s
    "$TMP/render" icon.svg "$SET/icon_${s}x${s}@2x.png" $((s * 2))
done
iconutil -c icns "$SET" -o AppIcon.icns
"$TMP/render" icon.svg icon-1024.png 1024
"$TMP/render" icon.svg icon-256.png 256
# Menu-bar template images (black + alpha); "off" is the same mark dimmed.
sed 's/<g opacity="1">/<g opacity="0.4">/' tray.svg > "$TMP/tray-off.svg"
"$TMP/render" tray.svg tray-on.png 44 36
"$TMP/render" "$TMP/tray-off.svg" tray-off.png 44 36
echo "==> Wrote assets/AppIcon.icns, icon-1024.png, icon-256.png, tray-*.png"
