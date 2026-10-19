**Wirest 0.2.0** adds a terminal version for Linux servers and the Mac terminal.

### New: `wirest-tui` for terminals and Linux servers
- The same one-click idea in a terminal UI: pick a VPN, press Enter to connect
- Works on headless servers over SSH, on Linux (x86_64 and arm64, any distro) and macOS
- Everything is included and statically built: WireGuard tools, bash and BoringTun. Uses the
  kernel's WireGuard when available, BoringTun otherwise (containers, LXC/OpenVZ VPSes)
- Commands for scripts: `list`, `status`, `connect <name>`, `disconnect`, `add <file>`
- Warns before connecting a VPN that would cut off your SSH session
- Run it as root, or let it install the helper once (`sudo` asks for your password; systemd on Linux)

### Mac app
- Now shares its helper with `wirest-tui`; updating installs the new helper once
- Shows "Waiting for server…" until the VPN server answers

### Downloads
| | |
| --- | --- |
| Mac app | **Wirest-macOS-arm64.dmg** (Apple Silicon, macOS 11+) |
| Linux server, Intel/AMD | **wirest-tui-linux-x86_64.tar.gz** |
| Linux server, ARM | **wirest-tui-linux-aarch64.tar.gz** |
| Mac terminal | **wirest-tui-macos-aarch64.tar.gz** |

On a server: `curl -fLO https://github.com/havban/wirest/releases/latest/download/wirest-tui-linux-x86_64.tar.gz && tar xzf wirest-tui-linux-x86_64.tar.gz && sudo ./wirest-tui-linux-x86_64/install.sh`

Full guide: https://havban.github.io/wirest/
