# Wirest

One-click WireGuard VPN app for macOS in Rust (egui + a root LaunchDaemon helper + bundled
wireguard-tools/BoringTun). Read `docs/DEVELOPMENT.md` first: architecture, decisions, how to test
the helper, release steps and known limitations.

- Use the rustup toolchain: `export PATH="$HOME/.cargo/bin:$PATH"` (Homebrew's cargo is too old,
  and Homebrew itself is broken on this macOS).
- `cargo build && cargo test`; `./run.sh` builds and opens `dist/Wirest.app`.
- Rebuilding the app makes the next Connect reinstall the helper (password prompt).
- Keep `docs/DEVELOPMENT.md` up to date when behaviour changes.
