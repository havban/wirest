#!/bin/bash
# Builds the runtime tools Wirest bundles into .deps/out/bin, without Homebrew or Go:
#   wg            - wireguard-tools (C, built with Xcode command line tools)
#   wg-quick      - wireguard-tools' darwin script
#   bash          - GNU bash 5 (wg-quick needs bash >= 4; macOS ships 3.2)
#   boringtun-cli - Cloudflare's userspace WireGuard in Rust (replaces wireguard-go)
#   wireguard-go  - tiny shim so wg-quick launches boringtun-cli
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEPS="$ROOT/.deps"
SRC="$DEPS/src"
OUT="$DEPS/out/bin"
JOBS="$(sysctl -n hw.ncpu)"

WG_TOOLS_VER=1.0.20250521
WG_TOOLS_SHA=61f520e7c1664ae9301fa36a2b8e90cf4680887a71f456c290d5d8b879f1e2e6
BASH_VER=5.3
BASH_SHA=0d5cd86965f869a26cf64f4b71be7b96f90a3ba8b3d74e27e8e9d9d5550f31ba
BORINGTUN_VER=0.6.0

# Oldest macOS the bundled tools run on (clang defaults to the build machine's).
export MACOSX_DEPLOYMENT_TARGET=11.0

# Bump BUILD_REV when the build settings or patches below change; a new
# stamp forces every tool to be rebuilt.
BUILD_REV=2
STAMP="$OUT/.build-$BUILD_REV-macos-$MACOSX_DEPLOYMENT_TARGET"
if [[ ! -f "$STAMP" ]]; then
    rm -rf "$OUT" "$DEPS/boringtun"
fi
mkdir -p "$SRC" "$OUT"

fetch() { # url sha256 dest
    if [[ ! -f "$3" ]] || ! echo "$2  $3" | shasum -a 256 -c - >/dev/null 2>&1; then
        echo "==> Downloading $1"
        curl -fsSL "$1" -o "$3.part"
        echo "$2  $3.part" | shasum -a 256 -c - >/dev/null || { echo "Checksum mismatch for $1" >&2; rm -f "$3.part"; exit 1; }
        mv "$3.part" "$3"
    fi
}

if [[ ! -x "$OUT/wg" || ! -f "$OUT/wg-quick" ]]; then
    tarball="$SRC/wireguard-tools-$WG_TOOLS_VER.tar.xz"
    fetch "https://git.zx2c4.com/wireguard-tools/snapshot/wireguard-tools-$WG_TOOLS_VER.tar.xz" "$WG_TOOLS_SHA" "$tarball"
    rm -rf "$SRC/wireguard-tools-$WG_TOOLS_VER" && tar -xf "$tarball" -C "$SRC"
    # `wg setconf` always sends fwmark=0, which BoringTun rejects with EINVAL
    # on macOS (fwmark is Linux-only). Don't send it.
    conf_c="$SRC/wireguard-tools-$WG_TOOLS_VER/src/config.c"
    sed -i '' 's/WGDEVICE_HAS_PRIVATE_KEY | WGDEVICE_HAS_FWMARK | WGDEVICE_HAS_LISTEN_PORT/WGDEVICE_HAS_PRIVATE_KEY | WGDEVICE_HAS_LISTEN_PORT/' "$conf_c"
    grep -q 'WGDEVICE_HAS_PRIVATE_KEY | WGDEVICE_HAS_LISTEN_PORT;' "$conf_c" || { echo "wg fwmark patch did not apply" >&2; exit 1; }
    echo "==> Building wg"
    make -s -C "$SRC/wireguard-tools-$WG_TOOLS_VER/src" -j"$JOBS" wg 2>"$SRC/wg-build.log" \
        || { cat "$SRC/wg-build.log" >&2; exit 1; }
    install -m 0755 "$SRC/wireguard-tools-$WG_TOOLS_VER/src/wg" "$OUT/wg"
    install -m 0755 "$SRC/wireguard-tools-$WG_TOOLS_VER/src/wg-quick/darwin.bash" "$OUT/wg-quick"
fi

if [[ ! -x "$OUT/bash" ]]; then
    tarball="$SRC/bash-$BASH_VER.tar.gz"
    fetch "https://ftp.gnu.org/gnu/bash/bash-$BASH_VER.tar.gz" "$BASH_SHA" "$tarball"
    rm -rf "$SRC/bash-$BASH_VER" && tar -xf "$tarball" -C "$SRC"
    echo "==> Building bash $BASH_VER (takes a minute)"
    (cd "$SRC/bash-$BASH_VER" && ./configure --without-bash-malloc --disable-nls && make -j"$JOBS") \
        >"$SRC/bash-build.log" 2>&1 || { tail -30 "$SRC/bash-build.log" >&2; exit 1; }
    install -m 0755 "$SRC/bash-$BASH_VER/bash" "$OUT/bash"
fi

if [[ ! -x "$OUT/boringtun-cli" ]]; then
    echo "==> Building boringtun-cli $BORINGTUN_VER"
    cargo install --quiet --locked boringtun-cli --version "$BORINGTUN_VER" --root "$DEPS/boringtun"
    install -m 0755 "$DEPS/boringtun/bin/boringtun-cli" "$OUT/boringtun-cli"
fi

# wg-quick runs `wireguard-go utun` from its own directory; hand that to
# boringtun-cli. It must keep root privileges since wg-quick configures the
# interface after the daemon starts.
cat > "$OUT/wireguard-go" <<'SHIM'
#!/bin/sh
exec "$(dirname "$0")/boringtun-cli" --disable-drop-privileges "$@"
SHIM
chmod 0755 "$OUT/wireguard-go"

touch "$STAMP"
echo "==> Tools ready in $OUT"
