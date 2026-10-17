# Wirus

A small native macOS GUI for WireGuard, written in Rust ([egui/eframe](https://github.com/emilk/egui)).
It manages tunnel configs and drives `wg-quick` / `wg` from
[wireguard-tools](https://www.wireguard.com/install/).

## Features

- Tunnel list with live status
- Activate / deactivate tunnels (`wg-quick up|down`)
- Create tunnels with a freshly generated key pair (Curve25519, same as `wg genkey`)
- Import `.conf` files via file picker or drag & drop
- Built-in editor with validation (bad keys, unknown fields, missing sections)
- Overview of interface and peers: public key (copyable), addresses, DNS, endpoints, allowed IPs
- Live stats per peer — latest handshake and rx/tx — when passwordless control is enabled
- Log panel with `wg-quick` output

## Requirements

```sh
brew install wireguard-tools   # provides wg, wg-quick, wireguard-go and bash 4+
```

Rust 1.81+ to build.

## Build & run

```sh
cargo run --release
./bundle.sh                     # creates dist/Wirus.app
cp -R dist/Wirus.app /Applications/
```

## How it works

Configs live in `~/Library/Application Support/Wirus/tunnels/<name>.conf` (dir `0700`, files `0600`).
The file name is the tunnel name, so it must be a valid interface name (1–15 chars, `[A-Za-z0-9_=+.-]`).

Bringing tunnels up/down needs root. By default Wirus asks through the standard macOS admin
password prompt each time.

**Passwordless control** (button in the toolbar) installs `/etc/sudoers.d/wirus` allowing your
user to run `wg` and `wg-quick` via `sudo` without a password. This removes the prompt and lets Wirus
poll `wg show all dump` for live handshake/transfer stats. You can revoke it from the toolbar.

> ⚠️ `wg-quick` executes `PreUp`/`PostUp`/`PreDown`/`PostDown` commands from config files as root.
> With the sudoers rule in place, anything that can write your configs can run commands as root.
> Only enable it on a machine you trust.

## Troubleshooting

- **"wireguard-tools not found"** — install them with Homebrew, then restart Wirus.
- **`Version mismatch: bash 3 detected`** — `wg-quick` needs Homebrew's bash. Wirus puts
  `/opt/homebrew/bin` first in `PATH`, but a `secure_path` in your sudoers config can override it.
- Status shows inactive for a tunnel started outside Wirus — without passwordless control the app
  can only track tunnels it started itself.

## Tests

```sh
cargo test
```
