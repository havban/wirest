# Wirest — developer guide

Everything needed to pick up work on Wirest in a new session: what it is, how it's built, why
things are the way they are, how to test, and what's still open. Keep this file current when
you change behaviour.

- Repo: https://github.com/havban/wirest (public, default branch `main`)
- Docs site: GitHub Pages from `docs/` on `main` → https://havban.github.io/wirest/
- Releases: https://github.com/havban/wirest/releases
- Former name: **Wirus** (renamed 2026-10-17; the GitHub repo was renamed too)

## Current state (update this when you stop working)

As of 2026-10-19:

- **v0.2.0 released** (latest): Mac app (dmg/zip) + `wirest-tui` packages for macOS aarch64 and
  Linux aarch64/x86_64 + SHA256SUMS. v0.1.0 (Mac app only) also carries the bash and
  wireguard-tools source tarballs that `scripts/deps.sh` uses as a download fallback. Docs site
  live from `main:/docs` (Pages "legacy" branch build, no Actions workflow) with a "Linux servers
  & the terminal" section.
- **Structure:** Cargo workspace — `crates/core` (shared), `crates/helper` (`wirest-helper`),
  `crates/gui` (Mac app), `crates/tui` (`wirest-tui`). See *Layout*.
- **Verified:** unit tests; `tests/linux-smoke.sh` on the release packages in Debian 12 (aarch64
  and x86_64): static binaries, root direct mode, helper as a normal user, SSH guard, hook refusal,
  full cleanup; the interactive TUI via tmux in Alpine (launch, connect, log, disconnect, paste,
  60 columns, quit); the website's `curl … | install.sh` flow from GitHub in a fresh container;
  on this Mac: new helper install, `wirest-tui` connect/disconnect through it, the Mac app
  sharing that helper without a reinstall prompt.
- **Not verified yet:** a real WireGuard server (handshake, traffic, DNS); kernel WireGuard on
  Linux (Apple container's kernel lacks it — only the BoringTun fallback ran); the systemd
  install path (containers have no systemd); the macOS menu-bar icon visually; install on
  another Mac / older macOS; Gatekeeper first-run of the downloaded app or Mac TUI tarball.
- **This Mac:** the v0.2.0 helper is installed (LaunchDaemon, socket `root:staff`). User tunnels
  dir has `wg.conf`, an unfinished template. Apple `container` 1.5.0 installed and running.
- **Product decisions:** configs are imported from files (no server/provider integration);
  simple one-button UIs; no visible config errors in the GUI's simple view.

## Product

A one-click WireGuard VPN client, in the spirit of Tailscale. Two frontends share one core:

**macOS app** (`Wirest.app`, egui):
- **Simple view (default):** big status + one Connect/Disconnect button, VPN picker when there's
  more than one, "Add VPN from file…" / "Paste config…". Only *complete* configs appear; unfinished
  ones are visible only in Advanced. Shows "Waiting for server…" until the first handshake.
- **Advanced view:** tunnel list, Overview (details, live stats), Edit (validated editor, rename,
  delete), `+ New` (template with fresh keys), log panel, "Remove helper…".
- **Menu-bar icon:** status, Connect/Disconnect, Open Wirest, Quit. Closing the window minimizes.

**Terminal UI + CLI** (`wirest-tui`, ratatui; macOS and Linux incl. headless servers):
- Interactive: VPN list + details, Enter connects/disconnects, `a` add file, `p` paste (bracketed
  paste), `n` new (template in `$EDITOR`), `e` edit in `$EDITOR`, `d` delete, `l` log, `q` quit
  (the VPN stays up). Incomplete configs are listed with an "incomplete" tag.
- CLI: `list`, `status`, `connect <name> [--force]`, `disconnect [name]`, `add <file> [name]`,
  `helper install|uninstall|status`.
- **SSH guard:** inside an SSH session (`SSH_CONNECTION`), connecting a config that routes
  `0.0.0.0/0` or `::/0` asks for confirmation (TUI) or needs `--force` (CLI) — it would cut the
  session off.
- As root it controls WireGuard directly (`Backend::Direct`); otherwise via the helper, installed
  with `sudo` in the terminal on first use.

Both: **one VPN at a time** (connecting another takes the current one down first); configs come
from files. Options considered and declined for now: own coordination server (Tailscale-style
join links), Cloudflare WARP auto-registration, VPN-provider sign-in.

## Layout

```
Cargo.toml              workspace (version lives in [workspace.package])
crates/core/            wirest-core library
  src/lib.rs            Backend: Direct (root) vs Helper; up/down/active/ensure_helper
  src/config.rs         .conf parsing + validation, keys (x25519), names, per-OS tunnels_dir
  src/store.rs          user tunnel files: load, add, import, unique names
  src/protocol.rs       JSON-line protocol, socket path, client request(), build ids
  src/daemon.rs         the helper's server loop (used by crates/helper)
  src/ops.rs            root operations: up/down/active via wg-quick, BoringTun cleanup
  src/install.rs        helper install/uninstall/status (LaunchDaemon | systemd), elevation
  src/wg.rs             finding tools, running wg-quick / `wg show all dump`
  src/format.rs         bytes, "5s ago", traffic summary
crates/helper/          wirest-helper binary (`--socket-gid <gid>`), runs as root
crates/gui/             wirest (macOS app): main.rs, app.rs (all UI), tray.rs (menu bar)
crates/tui/             wirest-tui: main.rs (CLI), tui.rs (interactive UI)
scripts/deps.sh         builds bundled tools into .deps/<os>-<arch>/bin (see below)
scripts/package-tui.sh  dist/wirest-tui-<os>-<arch>.tar.gz for the current platform
scripts/linux.sh        Linux packages on a Mac via Apple `container` (static, Alpine/musl)
scripts/release.sh      macOS app: ad-hoc sign + dmg/zip/SHA256SUMS; `--publish` creates the release
scripts/icons.sh        regenerates icns/PNGs from assets/*.svg (scripts/render-svg.swift)
tests/linux-smoke.sh    end-to-end test of a Linux package inside a container
bundle.sh               deps + release build + assemble dist/Wirest.app (incl. wirest-helper)
run.sh                  one-shot for users building the Mac app from source
assets/                 icon.svg, logo-*.svg, tray.svg, generated PNG/ICNS
docs/                   GitHub Pages site (index.html, img/), this guide, release notes
```

The GUI crate only builds on macOS; on Linux build `-p wirest-tui -p wirest-helper`.

## Architecture

```
Wirest.app / wirest-tui (user)                 wirest-helper (root: LaunchDaemon | systemd)
  Backend::Helper ──JSON line over SOCKET──▶  daemon.rs → ops.rs
   Version / Up{name,config} / Down{name}        ├─ writes STATE_DIR/<name>.conf (root, 0600)
   ◀── Response{ok, output, build, active[]}     ├─ <bin>/bash <bin>/wg-quick up|down <conf>
  poller: Version every 2 s (status + stats)     ├─ wg show all dump → stats
                                                 └─ stops leftover BoringTun processes
sudo wirest-tui → Backend::Direct → ops.rs (same functions, tools next to the binary)
```

| | macOS | Linux |
| --- | --- | --- |
| socket | `/var/run/wirest.sock` | `/run/wirest.sock` |
| helper + tools | `/Library/Application Support/Wirest/bin` | `/usr/local/lib/wirest/bin` |
| state (root copies of configs) | `/Library/Application Support/Wirest/tunnels` | `/var/lib/wirest/tunnels` |
| service | LaunchDaemon `com.github.havban.wirest.helper` | `wirest-helper.service` (needs systemd) |
| user configs | `~/Library/Application Support/Wirest/tunnels` | `~/.config/wirest/tunnels` |
| interface | `utunN`, mapped via `/var/run/wireguard/<name>.name` | named after the tunnel |

- **Install** (`install::install`): one root shell script that copies the tools and the shipped
  `wirest-helper` (found next to the frontend binary: `Wirest.app/Contents/MacOS/`, or the TUI
  package dir) into the bin dir, makes everything root-owned, writes the service, (re)starts it
  and waits for the socket. macOS also strips xattrs (quarantine would stop launchd). Elevation:
  `AdminDialog` (osascript password dialog, GUI) or `Terminal` (`sudo`, or nothing when root).
- **Socket access:** `root:<gid>` `0660`. gid = the installing user's group; when the install
  runs as root, `SUDO_GID`, else `staff` (20) on macOS. So the helper can be reached by those
  users only — and it therefore (a) uses only its root-owned tools, (b) validates names, (c)
  refuses configs with errors, including `PreUp/PostUp/PreDown/PostDown` (they'd run as root).
- **Helper versioning:** `protocol::file_build_id` — release builds: the crate version only (the
  GUI and TUI of one release share a helper without reinstall prompts); debug builds: version +
  hash of the helper binary (every rebuild reinstalls, so fixes are picked up during dev).
- **Status:** Linux: interfaces from `wg show all dump` (named after tunnels). macOS: `.name`
  files whose utun is in the dump.

## Bundled tools (`scripts/deps.sh`)

No Homebrew, no Go, no distro packages. Pinned + sha256-verified downloads (upstream, then a GNU
mirror, then copies on our v0.1.0 release; slow sources are abandoned after 20 s < 50 KB/s),
built into `.deps/<os>-<arch>/bin`:

| Tool | Source | Notes |
| --- | --- | --- |
| `wg`, `wg-quick` | wireguard-tools 1.0.20250521 | macOS: **patched** so `wg setconf` doesn't send `fwmark` (BoringTun EINVAL). wg-quick: darwin.bash / linux.bash |
| `bash` | GNU bash 5.3 | wg-quick needs bash ≥ 4 (macOS has 3.2, Alpine none) |
| `boringtun-cli` | boringtun 0.6.0 via `cargo install` | replaces wireguard-go; on Linux only used without kernel WireGuard |
| `wireguard-go` | 2-line shim | wg-quick calls `wireguard-go <iface>`; shim execs `boringtun-cli --disable-drop-privileges` |

- macOS: `MACOSX_DEPLOYMENT_TARGET=11.0` (clang otherwise targets the build machine's macOS).
  Check with `otool -l <bin> | grep -A4 LC_BUILD_VERSION`.
- Linux: `LDFLAGS=-static` on musl (Alpine) → static binaries that run on any distro. Linux
  boringtun-cli is installed **without `--locked`**: its 2023 lockfile pins a getrandom that
  calls `open64`, which musl ≥ 1.2.4 removed.
- `BUILD_REV` + a stamp file force a full rebuild when settings/patches change — bump it.
- `WIREST_BUILD_DIR` moves unpacking/compiling elsewhere (container builds: the shared host
  folder can't hold the tarballs' symlinks).
- wg-quick finds `wg`/`wireguard-go` in its own directory; it's always run as
  `<bin>/bash <bin>/wg-quick`.
- Linux runtime needs `iproute2` (`ip`); `resolvconf` for configs with `DNS =`.

### BoringTun quirks (important)

1. **fwmark** on macOS — see the wg patch.
2. **Doesn't exit when its tunnel goes down.** wg-quick expects the userspace implementation to
   exit (macOS: it deletes the UAPI socket; Linux: it deletes the interface). BoringTun keeps
   running — on macOS with the utun and routes. `ops::stop_userspace` (after down / failed up)
   and `ops::reap_orphans` (helper start; macOS uses it after down too) stop it.
3. **SIGTERM is handled asynchronously** (BoringTun blocks it and processes it in its event loop,
   taking up to ~1 s). `ops::terminate` sends SIGTERM, waits up to 3 s, then SIGKILL.
4. Linux without kernel WireGuard: `ip link add … type wireguard` prints "Unknown device type"
   before wg-quick's own fallback note; `ops::explain_fallback` drops that line.
5. Writes the utun name to `WG_TUN_NAME_FILE` on macOS; logs to `/tmp/boringtun.out`.

## Building & running

```sh
export PATH="$HOME/.cargo/bin:$PATH"   # rustup toolchain; /opt/homebrew/bin/cargo is an old 1.67
cargo build && cargo test              # 6 unit tests (config parsing, keys, names, template)
./run.sh                               # Mac app: full build + open dist/Wirest.app
./target/debug/wirest-tui              # TUI dev run (tools from .deps/<os>-<arch>/bin)
scripts/package-tui.sh                 # dist/wirest-tui-macos-aarch64.tar.gz
scripts/linux.sh                       # dist/wirest-tui-linux-{aarch64,x86_64}.tar.gz
```

Tools are looked up in `<exe>/../Resources/bin`, `<exe dir>`, `.deps/<os>-<arch>/bin` (debug
builds only), then Homebrew / system dirs.

## Testing

### Linux (Apple `container`)

Apple's `container` 1.5.0 is installed (`/usr/local/bin/container`; `container system start`
if it isn't running). Each container is a small VM with its own Linux kernel; `--arch amd64`
runs x86_64 via Rosetta. The default kernel has **no WireGuard module**, so tests exercise the
BoringTun fallback; `--cap-add CAP_NET_ADMIN` is needed for interfaces.

```sh
scripts/linux.sh arm64      # build (first run ~5 min; caches in .deps/ and target/linux-*)
container run --rm --arch arm64 --cap-add CAP_NET_ADMIN -v "$PWD:/src" docker.io/library/debian:12-slim \
  bash -c "apt-get update -qq && apt-get install -y -qq iproute2 procps binutils >/dev/null && \
           bash /src/tests/linux-smoke.sh /src/dist/wirest-tui-linux-aarch64.tar.gz"
scripts/linux.sh shell arm64  # interactive shell in the build image
```

The interactive TUI can be driven with tmux inside a container (`apk add tmux`):
`tmux new-session -d -s t -x 100 -y 24 wirest-tui; tmux send-keys -t t Enter; tmux capture-pane -p -t t`.
There's no tmux on the Mac itself (Homebrew is broken).

### macOS helper

Drive the helper over its socket (users in `staff`), with a safe config: route only
`10.123.45.0/24`, no DNS, endpoint in TEST-NET (`203.0.113.10`):

```python
import base64, json, os, socket
def req(o):
    s = socket.socket(socket.AF_UNIX); s.connect("/var/run/wirest.sock")
    s.sendall((json.dumps(o) + "\n").encode()); d = b""
    while not d.endswith(b"\n"): d += s.recv(65536)
    return json.loads(d)
k = lambda: base64.b64encode(os.urandom(32)).decode()
conf = f"[Interface]\nPrivateKey = {k()}\nAddress = 10.123.45.2/32\n\n[Peer]\nPublicKey = {k()}\nAllowedIPs = 10.123.45.0/24\nEndpoint = 203.0.113.10:51820\n"
print(req({"cmd": "up", "name": "wirestest", "config": conf}))
print(req({"cmd": "down", "name": "wirestest"}))
```

Or the CLI: `wirest-tui add test.conf && wirest-tui connect test && wirest-tui disconnect`.
Check afterwards: `ifconfig -l`, `netstat -rn -f inet | grep 10.123.45`,
`pgrep -fl Wirest/bin/boringtun`. Installing the helper needs a password: from this
non-interactive shell use
`osascript -e 'do shell script "<path>/wirest-tui helper install" with administrator privileges'`
(the user enters it in a dialog; `sudo` can't prompt here).

### Mac app screenshots

No click automation here; capture windows with `screencapture -x -o -l <windowid>`. Get the id
with CoreGraphics (match `kCGWindowName == "Wirest"`; the app also owns blank and menu-bar-sized
windows):

```sh
PID=$(pgrep -f Wirest.app/Contents/MacOS)
swift -e "import CoreGraphics; let l = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as! [[String:Any]]; for w in l where (w[\"kCGWindowOwnerPID\"] as? Int) == $PID && (w[\"kCGWindowName\"] as? String) == \"Wirest\" { print(w[\"kCGWindowNumber\"]!) }"
```

The menu-bar icon can't be captured this way on macOS 27. egui's default fonts lack many symbols
(`●`, `＋`, arrows render as boxes) — paint shapes or use plain text; `⚠` works.

## Releasing

1. Bump `version` in the root `Cargo.toml` (`[workspace.package]`); update `docs/release-notes.md`.
2. `scripts/release.sh` → `dist/Wirest-macOS-arm64.{dmg,zip}` + `SHA256SUMS`.
3. `scripts/package-tui.sh` (Mac) and `scripts/linux.sh` → `dist/wirest-tui-*.tar.gz`.
4. `scripts/release.sh --publish` creates tag `v<version>` + the release with the app assets; add
   the TUI tarballs with `gh release upload v<version> dist/wirest-tui-*.tar.gz`.
   Asset names are unversioned so `releases/latest/download/…` links keep working.

Signing: ad-hoc only (`codesign -s -`), not notarized — no Apple Developer ID. Users allow the
app once (System Settings → Privacy & Security → Open Anyway); the docs page explains.

## Environment notes (this Mac)

- macOS 27 on Apple Silicon. Homebrew is broken here (doesn't recognise macOS 27).
- Rust via rustup in `~/.cargo/bin` (not on PATH by default).
- Apple `container` 1.5.0 installed for Linux builds/tests. Rancher Desktop and podman on this
  Mac are Intel-only binaries and don't run.
- GitHub: HTTPS with the gh credential helper (`gh auth setup-git` done); SSH key not registered.
- `git` is Apple's `/usr/bin/git` (an old Intel-only git in `/usr/local/git` was removed).

## Known limitations / ideas

- Not notarized; macOS arm64 only (no universal build).
- The helper on Linux needs systemd; without it, run `wirest-tui` as root.
- No start-at-login, no reconnect on wake / network change, no on-demand rules.
- GUI tray status says "Connected to X" even while waiting for the first handshake.
- GUI `app.rs` still has its own `Tunnel` loading; the TUI uses `core::store` — could share.
- Possible next steps: Tailscale-style join links with a small server, QR import, kill switch,
  auto-update, packages for apt/dnf/Homebrew, a one-line `curl | sh` installer for servers.
