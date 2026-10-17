# Wirest — developer guide

Everything needed to pick up work on Wirest in a new session: what it is, how it's built, why
things are the way they are, how to test, and what's still open. Keep this file current when
you change behaviour.

- Repo: https://github.com/havban/wirest (public, default branch `main`)
- Docs site: GitHub Pages from `docs/` on `main` → https://havban.github.io/wirest/
- Releases: https://github.com/havban/wirest/releases (assets built by `scripts/release.sh`)
- Former name: **Wirus** (renamed 2026-10-17; the GitHub repo was renamed too)

## Product

A one-click WireGuard VPN client for macOS, in the spirit of Tailscale's Mac app:

- **Simple view (default):** big status + one Connect/Disconnect button, VPN picker when there's
  more than one, "Add VPN from file…" / "Paste config…". Only *complete* configs appear here;
  unfinished ones (e.g. a `+ New` template) are visible only in Advanced, so the simple view never
  shows config errors. After connecting it shows "Waiting for server…" until the first handshake,
  then "Connected" with traffic counters.
- **Advanced view** (toggle top right): tunnel list, Overview (interface/peer details, live stats),
  Edit (text editor with validation, rename, delete), `+ New` (template with fresh keys), log
  panel, "Remove helper…".
- **Menu-bar icon** (tray): status line, Connect/Disconnect, Open Wirest, Quit. Closing the window
  minimizes instead of quitting while the tray exists.
- **One VPN at a time:** connecting another tunnel takes the current one down first.
- **Configs come from files.** Decided with the owner: no built-in server/provider integration;
  users import a `.conf` (drag & drop, file picker, or paste) once, then it's one click. Options
  considered and declined for now: own coordination server (Tailscale-style join links),
  Cloudflare WARP auto-registration (unofficial API), VPN-provider sign-in (e.g. Mullvad).

## Layout

```
src/main.rs      entry; `--helper` runs the root daemon, `--install-helper` / `--uninstall-helper`
                 are terminal flags for troubleshooting; otherwise starts the GUI (eframe 0.32)
src/app.rs       all UI: simple view, advanced view, dialogs, poller thread, connect jobs,
                 tray sync. Actions are collected during rendering and applied after (`Action`).
src/helper.rs    root helper: install/uninstall (via admin prompt), client `request()`,
                 daemon (`run_daemon`), up/down, active-tunnel listing, BoringTun reaping
src/tray.rs      menu-bar icon (tray-icon crate), template icons on/off
src/config.rs    .conf parsing + validation, key generation (x25519), name rules, file storage
src/wg.rs        locating tools, running wg-quick / `wg show all dump` (as root), admin prompt
scripts/deps.sh  builds bundled tools into .deps/out/bin (see below)
scripts/icons.sh regenerates icns/PNGs from assets/*.svg (uses scripts/render-svg.swift)
scripts/release.sh  build + ad-hoc sign + dmg/zip/SHA256SUMS; `--publish` creates the GH release
bundle.sh        deps + cargo build --release + assemble dist/Wirest.app
run.sh           one-shot for users building from source: installs Rust if needed, bundles, opens
assets/          icon.svg (app icon), logo-*.svg, tray.svg, generated PNG/ICNS
docs/            GitHub Pages site (index.html, img/), this guide, release notes
```

## Architecture

```
Wirest.app (user)                         wirest-helper (root LaunchDaemon)
  app.rs  ──JSON line over /var/run/wirest.sock──▶  helper.rs
   │  Request::Version / Up{name,config} / Down{name}     │
   │  ◀── Response{ok, output, build, active[]} ──        ├─ writes /Library/Application Support/Wirest/tunnels/<name>.conf (root, 0600)
   │                                                      ├─ bash wg-quick up|down <conf>   (tools in …/Wirest/bin, root-owned)
   └─ poller: Version every 2 s (status + stats)          ├─ wg show all dump → stats
                                                          └─ reap orphaned boringtun-cli
```

- **Helper install** (`helper::install`): one `osascript … with administrator privileges` shell
  that copies the tools and the running binary (as `wirest-helper`) to
  `/Library/Application Support/Wirest/bin`, strips xattrs (quarantine!), chowns root:wheel,
  writes `/Library/LaunchDaemons/com.github.havban.wirest.helper.plist`, `launchctl bootstrap`s
  it, removes any legacy `/etc/sudoers.d/wirest`, and waits for the socket.
- **Auto-update of the helper:** `helper::build_id()` = crate version + hash of the executable.
  The app compares it with the helper's; on mismatch the next Connect reinstalls (one prompt).
  Consequence: **every rebuild of the app means one password prompt on next connect.**
- **Socket:** `/var/run/wirest.sock`, `root:staff` `0660` — any local user in `staff` can ask it
  to bring tunnels up/down. That's why the helper (a) uses only its own root-owned tools,
  (b) validates names (`config::valid_name`) and (c) rejects configs with errors, including
  `PreUp/PostUp/PreDown/PostDown` (they'd run as root).
- **Status:** active tunnels = `/var/run/wireguard/<name>.name` files whose utun shows up in
  `wg show all dump`. Stats (handshake, rx/tx) come from the same dump.
- User configs live in `~/Library/Application Support/Wirest/tunnels` (0700/0600). The app sends
  the config text on each Up; the helper's copy is just what wg-quick reads.

## Bundled tools (`scripts/deps.sh`)

No Homebrew and no Go. Pinned + sha256-verified downloads, built into `.deps/out/bin`:

| Tool | Source | Notes |
| --- | --- | --- |
| `wg`, `wg-quick` | wireguard-tools 1.0.20250521 | **patched**: `wg setconf` no longer sends `fwmark` (BoringTun returns EINVAL for it on macOS) |
| `bash` | GNU bash 5.3 | wg-quick needs bash ≥ 4 |
| `boringtun-cli` | boringtun 0.6.0 via `cargo install` | replaces wireguard-go |
| `wireguard-go` | 2-line shim | wg-quick calls `wireguard-go utun`; shim execs `boringtun-cli --disable-drop-privileges` |

- `MACOSX_DEPLOYMENT_TARGET=11.0` is exported; without it clang targets the build machine's macOS
  (27 here) and the release wouldn't run on older Macs. Check with
  `otool -l <bin> | grep -A4 LC_BUILD_VERSION`.
- `BUILD_REV` + a stamp file force a full rebuild when build settings/patches change — bump it.
- wg-quick finds `wireguard-go`/`wg` in its own directory (it prepends `${SELF%/*}` to PATH);
  the helper always runs it as `<bin>/bash <bin>/wg-quick` so bash 3.2 is never used.

### BoringTun quirks (important)

1. **fwmark** — see the wg patch above.
2. **Doesn't exit on `wg-quick down`.** wg-quick stops wireguard-go by deleting its UAPI socket;
   BoringTun ignores that and keeps the utun + routes alive. `helper::reap_orphans` kills any of
   our `boringtun-cli` processes whose `/var/run/wireguard/*.sock` (found via `lsof -F n`) no
   longer exists. Runs after down, after a failed up, and at helper start.
3. Writes the utun name to `WG_TUN_NAME_FILE` (wg-quick relies on it) — verified in source.
4. Logs to `/tmp/boringtun.out`.

## Building & running

```sh
export PATH="$HOME/.cargo/bin:$PATH"   # rustup toolchain; /opt/homebrew/bin/cargo is an old 1.67
cargo build && cargo test              # 6 unit tests (config parsing, keys, names, template)
./run.sh                               # full build + open dist/Wirest.app
./target/debug/wirest                  # dev run; finds tools in .deps/out/bin (debug builds only)
```

Debug builds look for tools in `<exe>/../Resources/bin`, then `.deps/out/bin` (debug only), then
Homebrew.

## Testing the helper without clicking

The helper can be driven directly over its socket (any user in `staff`). Safe test config: route
only `10.123.45.0/24`, no DNS, endpoint in TEST-NET (`203.0.113.10`), so the Mac's internet is
untouched:

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

Verified (2026-10-17, macOS 27): up creates utunN with address + route; down removes utun,
routes and the boringtun process; a failed up (unresolvable endpoint) leaves nothing behind;
`PostUp` configs and path-like names are refused. Afterwards check
`ifconfig -l`, `netstat -rn -f inet | grep 10.123.45`, `pgrep -fl Wirest/bin/boringtun`.

`./target/debug/wirest --install-helper` installs/updates the helper (password prompt).

### Screenshots / UI checks

No accessibility automation is available in this environment; capture windows with
`screencapture -x -o -l <windowid>`. Get the id with CoreGraphics (match `kCGWindowName ==
"Wirest"` — the app also owns blank 500×500 and menu-bar-sized windows):

```sh
PID=$(pgrep -f Wirest.app/Contents/MacOS)
swift -e "import CoreGraphics; let l = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as! [[String:Any]]; for w in l where (w[\"kCGWindowOwnerPID\"] as? Int) == $PID && (w[\"kCGWindowName\"] as? String) == \"Wirest\" { print(w[\"kCGWindowNumber\"]!) }"
```

The menu-bar icon can't be captured this way on macOS 27 (status items are drawn by the
system). `Tray::new` logs `menu-bar icon unavailable: …` to stderr if creation fails.

egui's default fonts lack many symbols (`●`, `＋`, arrows rendered as boxes) — paint shapes or
use plain text; `⚠` works.

## Releasing

1. Bump `version` in `Cargo.toml`; update `docs/release-notes.md`.
2. `scripts/release.sh` → `dist/Wirest-macOS-arm64.{dmg,zip}` + `SHA256SUMS` (asset names are
   unversioned so `releases/latest/download/…` links on the docs page keep working).
3. `scripts/release.sh --publish` (creates tag `v<version>` and the release with `gh`).

Signing: ad-hoc only (`codesign -s -`), not notarized — there's no Apple Developer ID. Users must
allow the app once (System Settings → Privacy & Security → Open Anyway); the docs page explains.
`spctl -a -vv dist/Wirest.app` reports "rejected" — expected.

## Environment notes (this Mac)

- macOS 27 on Apple Silicon. Homebrew is broken here (doesn't recognise macOS 27) — don't rely on it.
- Rust via rustup in `~/.cargo/bin` (not on PATH by default).
- GitHub: push over HTTPS with the gh credential helper (`gh auth setup-git` done). SSH key isn't
  registered with GitHub.
- An old Intel-only git at `/usr/local/git` was removed; `git` is Apple's `/usr/bin/git`.

## Known limitations / ideas

- Not notarized (needs Apple Developer ID); arm64 only (no universal build yet).
- Rebuilding the app changes `build_id`, so the helper is reinstalled (one prompt) on next connect.
  A stable protocol version instead of an exe hash would avoid prompts across updates, at the cost
  of not shipping helper fixes automatically.
- Menu-bar actions when the window is minimized depend on egui still processing events; Quit and
  Connect/Disconnect run in the tray handler directly, but "Open Wirest" sends viewport commands.
- No start-at-login, no reconnect on wake / network change, no on-demand rules.
- The tray status says "Connected to X" even while waiting for the first handshake (the window
  distinguishes them).
- No universal/Intel build; `--deep` codesign is deprecated by Apple but fine for ad-hoc.
- Possible next steps: Tailscale-style join links with a small server, QR import, kill switch,
  per-app/on-demand rules, auto-update (Sparkle-like).
