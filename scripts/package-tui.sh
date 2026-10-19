#!/bin/bash
# Packages the terminal version for the platform this runs on:
#   dist/wirest-tui-<os>-<arch>.tar.gz  containing  wirest-tui-<os>-<arch>/
#     wirest-tui  wirest-helper  wg  wg-quick  bash  boringtun-cli  wireguard-go  install.sh  README.txt
# On Linux, run it in a musl environment (Alpine) for static binaries;
# scripts/linux.sh does that in an Apple `container`.
set -euo pipefail
cd "$(dirname "$0")/.."

TOOLS="$(./scripts/deps.sh | tee /dev/stderr | tail -1)"
PLAT="$(basename "$(dirname "$TOOLS")")"       # e.g. linux-x86_64
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
cargo build --release -p wirest-tui -p wirest-helper

NAME="wirest-tui-$PLAT"
STAGE="dist/$NAME"
rm -rf "$STAGE" && mkdir -p "$STAGE"
cp "$TARGET_DIR/release/wirest-tui" "$TARGET_DIR/release/wirest-helper" "$STAGE/"
cp -p "$TOOLS"/* "$STAGE/"

cat > "$STAGE/install.sh" <<'EOF'
#!/bin/sh
# Installs wirest-tui to /usr/local/lib/wirest-tui with a command in /usr/local/bin.
set -e
DIR="$(cd "$(dirname "$0")" && pwd)"
DEST=/usr/local/lib/wirest-tui
if [ "$(id -u)" -ne 0 ]; then exec sudo sh "$0" "$@"; fi
mkdir -p "$DEST" /usr/local/bin
for f in wirest-tui wirest-helper wg wg-quick bash boringtun-cli wireguard-go; do
    cp -f "$DIR/$f" "$DEST/$f"
done
chmod 755 "$DEST" "$DEST"/*
ln -sf "$DEST/wirest-tui" /usr/local/bin/wirest-tui
echo "Installed. Run: wirest-tui   (or: sudo wirest-tui to skip the helper)"
EOF
chmod 755 "$STAGE/install.sh"

cat > "$STAGE/README.txt" <<EOF
Wirest $(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1) — one-click WireGuard for terminals and servers

  ./install.sh        install to /usr/local (asks for sudo)
  wirest-tui          open the interactive UI (or run ./wirest-tui from here)
  wirest-tui --help   commands for scripts: list, status, connect, disconnect, add

Everything WireGuard needs is included (wg, wg-quick, bash, BoringTun). On Linux
the system needs iproute2 (the 'ip' command); for configs with DNS = ..., also
resolvconf. Linux kernels with WireGuard use it; others fall back to BoringTun.

Docs: https://havban.github.io/wirest/   Source: https://github.com/havban/wirest
EOF

tar -C dist -czf "dist/$NAME.tar.gz" "$NAME"
echo "==> dist/$NAME.tar.gz"
