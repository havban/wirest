//! `wirest-tui`: Wirest for terminals and headless servers (macOS and Linux).
//! Without arguments it opens the interactive UI; subcommands cover scripts
//! and quick SSH sessions.

mod tui;

use std::path::Path;
use std::process::ExitCode;
use wirest_core::install::{self, Elevation, HelperStatus};
use wirest_core::{config, format, store, Backend};

const HELP: &str = "\
wirest-tui — one-click WireGuard VPN for terminals and servers

Usage:
  wirest-tui                      open the interactive UI
  wirest-tui list                 list VPNs and which one is connected
  wirest-tui status               show the connection status
  wirest-tui connect <name>       connect (--force: even if it may cut off SSH)
  wirest-tui disconnect [name]    disconnect (default: the connected VPN)
  wirest-tui add <file> [name]    add a VPN from a .conf file
  wirest-tui helper install|uninstall|status
                                  manage the background helper (non-root use)

Run as root (e.g. `sudo wirest-tui`) to control WireGuard directly; otherwise
Wirest installs a small helper once (sudo asks for your password) and never
asks again. VPN configs live in ";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        [] => tui::run(),
        ["-h" | "--help" | "help"] => {
            println!("{HELP}{}", config::tunnels_dir().display());
            Ok(())
        }
        ["-V" | "--version"] => {
            println!("wirest-tui {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        ["list"] => list(),
        ["status"] => status(),
        ["connect", name] => connect(name, false),
        ["connect", name, "--force"] | ["connect", "--force", name] => connect(name, true),
        ["disconnect"] => disconnect(None),
        ["disconnect", name] => disconnect(Some(name)),
        ["add", file] => add(file, None),
        ["add", file, name] => add(file, Some(name)),
        ["helper", "install"] => helper_install(),
        ["helper", "uninstall"] => install::uninstall(Elevation::Terminal).map(|()| println!("Helper removed")),
        ["helper", "status"] => helper_status(),
        _ => Err(format!("unknown command; see `wirest-tui --help`")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("wirest-tui: {}", e.trim());
            ExitCode::FAILURE
        }
    }
}

fn backend() -> Backend {
    Backend::detect(Elevation::Terminal)
}

fn list() -> Result<(), String> {
    let active = backend().active();
    let tunnels = store::load_all(&config::tunnels_dir());
    if tunnels.is_empty() {
        println!("No VPNs yet. Add one with `wirest-tui add <file.conf>`.");
    }
    for t in tunnels {
        let state = if active.iter().any(|a| a.name == t.name) {
            "connected"
        } else if t.ready() {
            ""
        } else {
            "incomplete"
        };
        println!("{:<16} {state}", t.name);
    }
    Ok(())
}

fn status() -> Result<(), String> {
    let b = backend();
    let tunnels = store::load_all(&config::tunnels_dir());
    match b.active().into_iter().find(|a| tunnels.iter().any(|t| t.name == a.name)) {
        Some(a) if format::last_handshake(&a.stats) > 0 => {
            println!("Connected to {}\n{}", a.name, format::traffic_summary(&a.stats))
        }
        Some(a) => println!("Waiting for server ({}): no handshake yet", a.name),
        None if !install::is_root() && matches!(install::status(), HelperStatus::Missing) => {
            println!("Not connected (the helper isn't installed yet; `connect` installs it)")
        }
        None => println!("Not connected"),
    }
    Ok(())
}

fn connect(name: &str, force: bool) -> Result<(), String> {
    let t = store::Tunnel::load(store::path_for(&config::tunnels_dir(), name))
        .ok_or_else(|| format!("No VPN named {name}; see `wirest-tui list`"))?;
    if let Some(e) = t.parsed.errors.first() {
        return Err(format!("{name} is incomplete: {e}"));
    }
    if !force && over_ssh() && config::routes_everything(&t.parsed) {
        return Err(format!(
            "{name} sends all traffic through the VPN, which will probably cut off this SSH \
             session. Use `wirest-tui connect {name} --force` to connect anyway."
        ));
    }
    let (output, _) = backend().up(name, &t.text)?;
    print_output(&output);
    println!("Connected to {name}");
    Ok(())
}

fn disconnect(name: Option<&str>) -> Result<(), String> {
    let b = backend();
    let name = match name {
        Some(n) => n.to_owned(),
        None => b.active().into_iter().next().map(|a| a.name).ok_or("Not connected")?,
    };
    let (output, _) = b.down(&name)?;
    print_output(&output);
    println!("Disconnected {name}");
    Ok(())
}

fn add(file: &str, name: Option<&str>) -> Result<(), String> {
    let dir = config::tunnels_dir();
    let name = match name {
        Some(n) => {
            let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
            store::add(&dir, n, &text)?;
            n.to_owned()
        }
        None => store::import(&dir, Path::new(file))?,
    };
    println!("Added {name}. Connect with `wirest-tui connect {name}`.");
    Ok(())
}

fn helper_install() -> Result<(), String> {
    let b = backend();
    let tools = b.tools().ok_or("WireGuard tools not found next to wirest-tui")?;
    install::install(tools, Elevation::Terminal)?;
    println!("Helper installed");
    Ok(())
}

fn helper_status() -> Result<(), String> {
    match install::status() {
        HelperStatus::Missing => println!("Helper: not installed"),
        HelperStatus::Stale(r) => println!("Helper: installed, but a different version ({})", r.build),
        HelperStatus::Ready(r) => println!("Helper: installed and current ({})", r.build),
    }
    if install::is_root() {
        println!("(running as root: wirest-tui controls WireGuard directly and doesn't need it)");
    }
    Ok(())
}

fn print_output(output: &str) {
    for l in output.lines().filter(|l| !l.trim().is_empty()) {
        println!("  {l}");
    }
}

/// True inside an SSH session, where a full-tunnel VPN usually cuts the
/// connection off.
pub fn over_ssh() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_CLIENT").is_some()
}
