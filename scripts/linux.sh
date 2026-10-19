#!/bin/bash
# Builds the Linux TUI packages on a Mac using Apple's `container` tool
# (https://github.com/apple/container): static musl builds in Alpine.
#   scripts/linux.sh              arm64 and amd64
#   scripts/linux.sh arm64        one architecture
#   scripts/linux.sh shell arm64  interactive shell in the build container
# Output: dist/wirest-tui-linux-{aarch64,x86_64}.tar.gz
set -euo pipefail
cd "$(dirname "$0")/.."

IMAGE=docker.io/library/rust:1.95-alpine3.22
# Per-arch caches inside .deps so container builds never touch the host's.
run() { # arch command...
    local arch="$1"; shift
    container run --rm --arch "$arch" --cpus 4 --memory 6G \
        -v "$PWD:/src" -w /src \
        -e CARGO_HOME="/src/.deps/cargo-home-linux-$arch" \
        -e CARGO_TARGET_DIR="/src/target/linux-$arch" \
        -e WIREST_BUILD_DIR=/tmp/wirest-build \
        "$IMAGE" sh -c "apk add -q --no-cache build-base linux-headers bash curl xz tar perl >/dev/null && $*"
}

if [[ "${1:-}" == shell ]]; then
    arch="${2:-arm64}"
    container run --rm -it --arch "$arch" --cap-add CAP_NET_ADMIN \
        -v "$PWD:/src" -w /src \
        -e CARGO_HOME="/src/.deps/cargo-home-linux-$arch" -e CARGO_TARGET_DIR="/src/target/linux-$arch" \
        "$IMAGE" sh
    exit
fi

for arch in "${@:-arm64 amd64}"; do
    for a in $arch; do
        echo "==> Building wirest-tui for linux/$a"
        run "$a" "bash scripts/package-tui.sh"
    done
done
