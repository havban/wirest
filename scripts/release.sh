#!/bin/bash
# Builds dist/Wirest.app and packages it for a GitHub release:
#   dist/Wirest-macOS-arm64.dmg  (drag-to-Applications disk image)
#   dist/Wirest-macOS-arm64.zip
#   dist/SHA256SUMS
# The asset names carry no version so releases/latest/download/<name> links stay stable.
#   scripts/release.sh            build the files
#   scripts/release.sh --publish  also create the GitHub release v<Cargo version> with gh
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
NAME=Wirest-macOS-arm64
APP=dist/Wirest.app

./bundle.sh

# No Developer ID here, so sign ad hoc: keeps the bundle's signatures consistent,
# but Gatekeeper will still ask users to allow the app once (see the docs).
codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP"

rm -f "dist/$NAME.zip" "dist/$NAME.dmg"
ditto -c -k --keepParent "$APP" "dist/$NAME.zip"

STAGE="$(mktemp -d)"; trap 'rm -rf "$STAGE"' EXIT
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -quiet -volname "Wirest" -srcfolder "$STAGE" -ov -format UDZO "dist/$NAME.dmg"

(cd dist && shasum -a 256 "$NAME.dmg" "$NAME.zip" > SHA256SUMS)
echo "==> Wirest $VERSION packaged:"; cat dist/SHA256SUMS

if [[ "${1:-}" == "--publish" ]]; then
    gh release create "v$VERSION" "dist/$NAME.dmg" "dist/$NAME.zip" dist/SHA256SUMS \
        --title "Wirest $VERSION" --notes-file docs/release-notes.md
fi
