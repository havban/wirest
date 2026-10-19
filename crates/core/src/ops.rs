//! Tunnel control as root: used by the helper daemon, and directly by a
//! frontend that already runs as root (e.g. `sudo wirest-tui` on a server).
//!
//! Configs are copied into a root-only state directory and brought up with
//! the bundled wg-quick. Configs with errors — including script hooks, which
//! would run as root — are refused (see `config::parse`).

use crate::config;
use crate::protocol::Active;
use crate::wg::{self, Tools};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Root-owned copies of the configs that wg-quick reads.
pub const STATE_DIR: &str = if cfg!(target_os = "macos") {
    "/Library/Application Support/Wirest/tunnels"
} else {
    "/var/lib/wirest/tunnels"
};
/// wg-quick's runtime dir: UAPI sockets, and on macOS `<name>.name` -> utunN.
const RUN_DIR: &str = "/var/run/wireguard";

fn conf_path(name: &str) -> PathBuf {
    Path::new(STATE_DIR).join(format!("{name}.conf"))
}

pub fn up(tools: &Tools, name: &str, text: &str) -> Result<String, String> {
    if !config::valid_name(name) {
        return Err(format!("Invalid tunnel name {name:?}"));
    }
    if let Some(e) = config::parse(text).errors.into_iter().next() {
        return Err(e);
    }
    // One VPN at a time: connecting replaces whatever Wirest had up.
    let mut out = String::new();
    for a in active(tools) {
        if conf_path(&a.name).exists() {
            out += &down(tools, &a.name).unwrap_or_else(|e| e);
        }
    }
    let path = conf_path(name);
    config::ensure_dir(Path::new(STATE_DIR)).map_err(|e| e.to_string())?;
    config::write_secure(&path, text).map_err(|e| e.to_string())?;
    match wg::quick(tools, "up", &path) {
        Ok(o) => Ok(out + &explain_fallback(&o)),
        Err(e) => {
            stop_userspace(tools, name);
            Err(e)
        }
    }
}

/// Without kernel WireGuard, Linux wg-quick prints `ip`'s alarming "Unknown
/// device type" before its own clear note about falling back; drop the former.
fn explain_fallback(output: &str) -> String {
    output.lines().filter(|l| l.trim() != "Error: Unknown device type.").collect::<Vec<_>>().join("\n")
}

pub fn down(tools: &Tools, name: &str) -> Result<String, String> {
    if !config::valid_name(name) {
        return Err(format!("Invalid tunnel name {name:?}"));
    }
    let path = conf_path(name);
    if !path.exists() {
        return Err(format!("{name} wasn't started by Wirest"));
    }
    let result = wg::quick(tools, "down", &path);
    stop_userspace(tools, name);
    result
}

/// After taking `name` down (or failing to bring it up), make sure no
/// BoringTun process is left serving it. On Linux the interface is named
/// after the tunnel, so its process is known exactly; macOS uses utunN names,
/// so fall back to the general orphan check.
fn stop_userspace(tools: &Tools, name: &str) {
    if cfg!(target_os = "macos") {
        reap_orphans(tools);
        return;
    }
    for pid in boringtun_pids(&tools.dir.join("boringtun-cli")) {
        if last_arg(&pid).as_deref() == Some(name) {
            terminate(&pid);
        }
    }
    std::fs::remove_file(Path::new(RUN_DIR).join(format!("{name}.sock"))).ok();
}

fn last_arg(pid: &str) -> Option<String> {
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let args: Vec<&[u8]> = cmdline.split(|b| *b == 0).filter(|a| !a.is_empty()).collect();
    (args.len() > 1).then(|| String::from_utf8_lossy(args[args.len() - 1]).into_owned())
}

/// Running WireGuard interfaces, by tunnel name.
pub fn active(tools: &Tools) -> Vec<Active> {
    let dump = wg::show_dump(tools).unwrap_or_default();
    if cfg!(target_os = "macos") {
        // Interfaces are utunN; wg-quick records which tunnel is which.
        std::fs::read_dir(RUN_DIR)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                if path.extension()? != "name" {
                    return None;
                }
                let name = path.file_stem()?.to_str()?.to_owned();
                let iface = std::fs::read_to_string(&path).ok()?.trim().to_owned();
                let stats = dump.iter().find(|d| d.iface == iface)?.clone();
                Some(Active { name, stats })
            })
            .collect()
    } else {
        // On Linux the interface is named after the tunnel.
        dump.into_iter()
            .map(|stats| Active { name: stats.iface.clone(), stats })
            .collect()
    }
}

/// When wg-quick takes down a tunnel served by a userspace implementation,
/// wireguard-go exits on its own but BoringTun doesn't, keeping the interface
/// and routes (macOS) or a dead process and socket (Linux) around. Stop any of
/// our BoringTun processes whose tunnel is gone. Runs after down, after a
/// failed up, and at helper start.
pub fn reap_orphans(tools: &Tools) {
    let boringtun = tools.dir.join("boringtun-cli");
    for pid in boringtun_pids(&boringtun) {
        if let Some(leftovers) = orphaned(&pid) {
            terminate(&pid);
            for f in leftovers {
                std::fs::remove_file(f).ok();
            }
        }
    }
}

/// SIGTERM, then SIGKILL if still running after 3 s. BoringTun handles
/// SIGTERM in its event loop, so it can take a moment to exit; callers expect
/// it gone when this returns.
fn terminate(pid: &str) {
    let Ok(pid) = pid.parse::<libc::pid_t>() else { return };
    // SAFETY: kill only sends a signal to the given pid.
    let alive = || unsafe { libc::kill(pid, 0) } == 0;
    unsafe { libc::kill(pid, libc::SIGTERM) };
    for _ in 0..30 {
        if !alive() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    unsafe { libc::kill(pid, libc::SIGKILL) };
}

fn output(cmd: &mut Command) -> String {
    cmd.output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

fn boringtun_pids(exe: &Path) -> Vec<String> {
    output(Command::new("pgrep").arg("-f").arg(exe))
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

/// macOS: wg-quick down deletes the UAPI socket (under RUN_DIR); the process
/// is orphaned once all its sockets are gone. Returns files to clean up.
#[cfg(target_os = "macos")]
fn orphaned(pid: &str) -> Option<Vec<PathBuf>> {
    // -F n prints each open file's name on a line starting with 'n'.
    let sockets: Vec<String> = output(Command::new("/usr/sbin/lsof").args(["-a", "-p", pid, "-U", "-F", "n"]))
        .lines()
        .filter_map(|l| l.strip_prefix('n'))
        .filter(|n| n.starts_with(RUN_DIR))
        .map(str::to_owned)
        .collect();
    (!sockets.is_empty() && sockets.iter().all(|s| !Path::new(s).exists())).then(Vec::new)
}

/// Linux: wg-quick names the interface after the tunnel and passes it as
/// BoringTun's last argument; down deletes the interface but leaves the
/// process and its socket (RUN_DIR/<iface>.sock).
#[cfg(not(target_os = "macos"))]
fn orphaned(pid: &str) -> Option<Vec<PathBuf>> {
    let iface = last_arg(pid)?;
    if !config::valid_name(&iface) || Path::new("/sys/class/net").join(&iface).exists() {
        return None;
    }
    Some(vec![Path::new(RUN_DIR).join(format!("{iface}.sock"))])
}
