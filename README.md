# Wirus

A small native macOS GUI for WireGuard, written in Rust ([egui/eframe](https://github.com/emilk/egui)).
It manages tunnel configs and drives `wg-quick` / `wg`. Everything it needs is built from source and
bundled inside `Wirus.app` — no Homebrew, no Go.

## Quick start

```sh
./run.sh            # installs what's missing, builds dist/Wirus.app and opens it
./run.sh --install  # same, and copies it to /Applications
```

The only thing you may be asked for is Xcode Command Line Tools (`run.sh` opens Apple's installer if
they are missing). Rust is installed via [rustup](https://rustup.rs) if absent. The first build takes
a couple of minutes; later runs reuse the cache in `.deps/`.

## Features

- Tunnel list with live status
- Activate / deactivate tunnels (`wg-quick up|down`)
- Create tunnels with a freshly generated key pair (Curve25519, same as `wg genkey`)
- Import `.conf` files via file picker or drag & drop
- Built-in editor with validation (bad keys, unknown fields, missing sections)
- Overview of interface and peers: public key (copyable), addresses, DNS, endpoints, allowed IPs
- Live stats per peer — latest handshake and rx/tx — when passwordless control is enabled
- Log panel with `wg-quick` output

## Bundled tools

`scripts/deps.sh` downloads pinned, checksum-verified sources and builds into `.deps/out/bin`,
which `bundle.sh` copies to `Wirus.app/Contents/Resources/bin`:

| Tool | Source | Why |
| --- | --- | --- |
| `wg`, `wg-quick` | [wireguard-tools](https://git.zx2c4.com/wireguard-tools/) 1.0.20250521 (C) | configure interfaces |
| `boringtun-cli` | [BoringTun](https://github.com/cloudflare/boringtun) 0.6.0 (Rust, `cargo install`) | userspace WireGuard, replaces `wireguard-go` |
| `bash` | GNU bash 5.3 | `wg-quick` needs bash ≥ 4; macOS ships 3.2 |

`wg-quick` launches `wireguard-go` from its own directory; a two-line shim with that name starts
`boringtun-cli --disable-drop-privileges` instead. If no bundled tools are found, Wirus falls back to
a Homebrew `wireguard-tools` install.

Note: bash and wireguard-tools are GPL; if you redistribute `Wirus.app`, include their sources.

## How it works

Configs live in `~/Library/Application Support/Wirus/tunnels/<name>.conf` (dir `0700`, files `0600`).
The file name is the tunnel name, so it must be a valid interface name (1–15 chars, `[A-Za-z0-9_=+.-]`).

Bringing tunnels up/down needs root. By default Wirus asks through the standard macOS admin
password prompt each time.

**Passwordless control** (button in the toolbar) installs `/etc/sudoers.d/wirus` allowing your
user to run the bundled `wg-quick` (via the bundled bash) and `wg show` via `sudo` without a password. This removes the prompt and lets Wirus
poll `wg show all dump` for live handshake/transfer stats. You can revoke it from the toolbar.

> ⚠️ `wg-quick` executes `PreUp`/`PostUp`/`PreDown`/`PostDown` commands from config files as root.
> With the sudoers rule in place, anything that can write your configs can run commands as root.
> Only enable it on a machine you trust.

## Troubleshooting

- **"WireGuard tools not found"** — run `./run.sh` (or `./scripts/deps.sh`) and relaunch.
- **Moved the app after enabling passwordless control?** The sudoers rule names the tool paths; click
  *Revoke* and enable it again.
- BoringTun logs to `/tmp/boringtun.out`.
- Status shows inactive for a tunnel started outside Wirus — without passwordless control the app
  can only track tunnels it started itself.

## Tests

```sh
cargo test
```
