#!/bin/bash
# Builds the runtime tools Wirest bundles, without Homebrew, Go or distro packages,
# into .deps/<os>-<arch>/bin (os: macos|linux, arch: aarch64|x86_64):
#   wg            - wireguard-tools (C)
#   wg-quick      - wireguard-tools' darwin or linux script
#   bash          - GNU bash 5 (wg-quick needs bash >= 4; macOS ships 3.2, Alpine none)
#   boringtun-cli - Cloudflare's userspace WireGuard in Rust (replaces wireguard-go);
#                   on Linux only used when the kernel has no WireGuard module
#   wireguard-go  - tiny shim so wg-quick launches boringtun-cli
# Linux binaries are linked statically (build on musl, e.g. Alpine) so one build
# runs on any distro. Prints the output directory on the last line.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEPS="$ROOT/.deps"
SRC="$DEPS/src"

case "$(uname -s)" in
    Darwin) OS=macos ;;
    Linux) OS=linux ;;
    *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac
ARCH="$(uname -m)"
[[ "$ARCH" == arm64 ]] && ARCH=aarch64
PLAT="$DEPS/$OS-$ARCH"
OUT="$PLAT/bin"
# Where sources are unpacked and compiled; WIREST_BUILD_DIR lets container builds
# use their own filesystem (shared host folders can't hold the tarballs' symlinks).
BUILD="${WIREST_BUILD_DIR:-$PLAT/build}"
if [[ $OS == macos ]]; then JOBS="$(sysctl -n hw.ncpu)"; else JOBS="$(nproc)"; fi

WG_TOOLS_VER=1.0.20250521
WG_TOOLS_SHA=61f520e7c1664ae9301fa36a2b8e90cf4680887a71f456c290d5d8b879f1e2e6
BASH_VER=5.3
BASH_SHA=0d5cd86965f869a26cf64f4b71be7b96f90a3ba8b3d74e27e8e9d9d5550f31ba
BORINGTUN_VER=0.6.0

if [[ $OS == macos ]]; then
    # Oldest macOS the bundled tools run on (clang defaults to the build machine's).
    export MACOSX_DEPLOYMENT_TARGET=11.0
    TARGET_TAG="macos-$MACOSX_DEPLOYMENT_TARGET"
else
    export LDFLAGS="-static"
    TARGET_TAG="static"
fi

# Bump BUILD_REV when the build settings or patches below change; a new
# stamp forces every tool to be rebuilt.
BUILD_REV=3
STAMP="$OUT/.build-$BUILD_REV-$TARGET_TAG"
if [[ ! -f "$STAMP" ]]; then
    rm -rf "$OUT" "$BUILD" "$PLAT/boringtun"
fi
mkdir -p "$SRC" "$OUT" "$BUILD"

# Exact copies of the upstream source tarballs, kept on our own release (also
# serves as the GPL source offer). Used when the upstream servers are down or slow;
# the sha256 check makes the source irrelevant.
SOURCE_MIRROR=https://github.com/havban/wirest/releases/download/v0.1.0

sha256_ok() { # sha256 file
    if command -v shasum >/dev/null; then
        echo "$1  $2" | shasum -a 256 -c - >/dev/null 2>&1
    else
        echo "$1  $2" | sha256sum -c - >/dev/null 2>&1
    fi
}

fetch() { # sha256 dest url...
    local sha="$1" dest="$2"; shift 2
    if [[ -f "$dest" ]] && sha256_ok "$sha" "$dest"; then
        return
    fi
    for url in "$@" "$SOURCE_MIRROR/$(basename "$dest")"; do
        echo "==> Downloading $url"
        # Give up on a server that's slower than 50 KB/s for 20 s and try the next.
        if curl -fsSL --connect-timeout 15 --speed-limit 50000 --speed-time 20 "$url" -o "$dest.part" \
            && sha256_ok "$sha" "$dest.part"; then
            mv "$dest.part" "$dest"
            return
        fi
        echo "    failed or too slow, trying next source" >&2
        rm -f "$dest.part"
    done
    echo "Could not download $(basename "$dest") from any source" >&2
    exit 1
}

if [[ ! -x "$OUT/wg" || ! -f "$OUT/wg-quick" ]]; then
    tarball="$SRC/wireguard-tools-$WG_TOOLS_VER.tar.xz"
    fetch "$WG_TOOLS_SHA" "$tarball" \
        "https://git.zx2c4.com/wireguard-tools/snapshot/wireguard-tools-$WG_TOOLS_VER.tar.xz"
    dir="$BUILD/wireguard-tools-$WG_TOOLS_VER"
    rm -rf "$dir" && tar -xf "$tarball" -C "$BUILD"
    if [[ $OS == macos ]]; then
        # `wg setconf` always sends fwmark=0, which BoringTun rejects with EINVAL
        # on macOS (fwmark is Linux-only). Don't send it.
        sed -i.orig 's/WGDEVICE_HAS_PRIVATE_KEY | WGDEVICE_HAS_FWMARK | WGDEVICE_HAS_LISTEN_PORT/WGDEVICE_HAS_PRIVATE_KEY | WGDEVICE_HAS_LISTEN_PORT/' "$dir/src/config.c"
        grep -q 'WGDEVICE_HAS_PRIVATE_KEY | WGDEVICE_HAS_LISTEN_PORT;' "$dir/src/config.c" \
            || { echo "wg fwmark patch did not apply" >&2; exit 1; }
    fi
    echo "==> Building wg"
    make -s -C "$dir/src" -j"$JOBS" wg 2>"$BUILD/wg-build.log" || { cat "$BUILD/wg-build.log" >&2; exit 1; }
    install -m 0755 "$dir/src/wg" "$OUT/wg"
    if [[ $OS == macos ]]; then
        install -m 0755 "$dir/src/wg-quick/darwin.bash" "$OUT/wg-quick"
    else
        install -m 0755 "$dir/src/wg-quick/linux.bash" "$OUT/wg-quick"
    fi
fi

if [[ ! -x "$OUT/bash" ]]; then
    tarball="$SRC/bash-$BASH_VER.tar.gz"
    fetch "$BASH_SHA" "$tarball" \
        "https://ftp.gnu.org/gnu/bash/bash-$BASH_VER.tar.gz" \
        "https://mirrors.kernel.org/gnu/bash/bash-$BASH_VER.tar.gz"
    dir="$BUILD/bash-$BASH_VER"
    rm -rf "$dir" && tar -xf "$tarball" -C "$BUILD"
    echo "==> Building bash $BASH_VER (takes a minute)"
    (cd "$dir" && ./configure --without-bash-malloc --disable-nls && make -j"$JOBS") \
        >"$BUILD/bash-build.log" 2>&1 || { tail -30 "$BUILD/bash-build.log" >&2; exit 1; }
    install -m 0755 "$dir/bash" "$OUT/bash"
fi

if [[ ! -x "$OUT/boringtun-cli" ]]; then
    echo "==> Building boringtun-cli $BORINGTUN_VER"
    # boringtun-cli's 2023 Cargo.lock pins a getrandom that calls open64, which
    # musl >= 1.2.4 no longer provides; on Linux let cargo pick fixed patch versions.
    locked=--locked
    [[ $OS == linux ]] && locked=
    # LDFLAGS is for the C builds above; Rust on musl links statically by default.
    LDFLAGS= cargo install --quiet $locked boringtun-cli --version "$BORINGTUN_VER" --root "$PLAT/boringtun"
    install -m 0755 "$PLAT/boringtun/bin/boringtun-cli" "$OUT/boringtun-cli"
fi

# wg-quick runs `wireguard-go <iface>` from its own directory when it needs a
# userspace implementation; hand that to boringtun-cli. It must keep root
# privileges since wg-quick configures the interface after the daemon starts.
cat > "$OUT/wireguard-go" <<'SHIM'
#!/bin/sh
exec "$(dirname "$0")/boringtun-cli" --disable-drop-privileges "$@"
SHIM
chmod 0755 "$OUT/wireguard-go"

touch "$STAMP"
echo "==> Tools ready in $OUT"
echo "$OUT"
