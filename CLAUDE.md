# Wirest

One-click WireGuard VPN in Rust: a macOS app (egui) and a terminal UI/CLI for macOS and Linux
(ratatui), sharing `crates/core`, plus a root helper daemon and bundled wireguard-tools/BoringTun.
Read `docs/DEVELOPMENT.md` first — start with its "Current state" section: architecture,
decisions, how to test (Linux via Apple `container`), release steps and known limitations.

- Use the rustup toolchain: `export PATH="$HOME/.cargo/bin:$PATH"` (Homebrew's cargo is too old,
  and Homebrew itself is broken on this macOS).
- `cargo build && cargo test`; `./run.sh` builds and opens `dist/Wirest.app`;
  `scripts/linux.sh` builds the Linux packages; `tests/linux-smoke.sh` tests one in a container.
- Debug builds reinstall the helper on next connect after each rebuild (password prompt).
- Keep `docs/DEVELOPMENT.md` (especially "Current state") up to date when behaviour changes.
