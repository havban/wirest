#!/bin/bash
# One-shot setup: installs anything missing, builds Wirest with its bundled
# WireGuard tools, and launches it. Safe to re-run; finished steps are cached.
#   ./run.sh            build and open dist/Wirest.app
#   ./run.sh --install  also copy it to /Applications
set -euo pipefail
cd "$(dirname "$0")"

if ! xcode-select -p >/dev/null 2>&1; then
    echo "==> Xcode Command Line Tools are required (clang, make). Opening the installer…"
    xcode-select --install || true
    echo "    Re-run ./run.sh once the installation finishes."
    exit 1
fi

# Prefer rustup's toolchain over any older system cargo.
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v rustup >/dev/null 2>&1; then
    echo "==> Installing Rust (rustup)"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    source "$HOME/.cargo/env"
fi
MIN_RUST=1.81
if [[ "$(printf '%s\n' "$MIN_RUST" "$(rustc --version | cut -d' ' -f2)" | sort -V | head -1)" != "$MIN_RUST" ]]; then
    echo "==> Updating Rust (need >= $MIN_RUST)"
    rustup update stable
fi

./bundle.sh

APP=dist/Wirest.app
if [[ "${1:-}" == "--install" ]]; then
    echo "==> Installing to /Applications"
    rm -rf /Applications/Wirest.app
    cp -R "$APP" /Applications/
    APP=/Applications/Wirest.app
fi
echo "==> Launching $APP"
open "$APP"
