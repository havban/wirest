//! Privileged helper. Like Tailscale's system extension it is installed once,
//! behind one admin prompt, as a root LaunchDaemon; after that the app
//! connects and disconnects by talking to it over a Unix socket, with no
//! password prompts.
//!
//! Any local user in `staff` can reach the socket, so the helper only runs
//! its own root-owned copy of the tools and refuses configs with script
//! hooks (see `config::parse`).

use crate::config;
use crate::wg::{self, IfaceStats, Tools};
use serde::{Deserialize, Serialize};
use std::fs::Permissions;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::process::Command;
use std::time::Duration;

const SOCKET: &str = "/var/run/wirest.sock";
const HOME: &str = "/Library/Application Support/Wirest";
const LABEL: &str = "com.github.havban.wirest.helper";
const PLIST: &str = "/Library/LaunchDaemons/com.github.havban.wirest.helper.plist";
/// wg-quick records `<name>.name` -> utunN here.
const RUN_DIR: &str = "/var/run/wireguard";
const STAFF_GID: u32 = 20;

#[derive(Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "lowercase")]
pub enum Request {
    Version,
    Up { name: String, config: String },
    Down { name: String },
}

#[derive(Serialize, Deserialize, Default)]
pub struct Response {
    pub ok: bool,
    pub output: String,
    pub build: String,
    pub active: Vec<Active>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Active {
    pub name: String,
    pub stats: IfaceStats,
}

/// Identifies this exact build, so the app can tell when the installed
/// helper is out of date and needs reinstalling.
pub fn build_id() -> String {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        let bytes = std::env::current_exe().and_then(std::fs::read).unwrap_or_default();
        let mut h = std::hash::DefaultHasher::new();
        bytes.hash(&mut h);
        format!("{}-{:016x}", env!("CARGO_PKG_VERSION"), h.finish())
    })
    .clone()
}

// ---- client side (the app) ----

pub fn request(req: &Request) -> Result<Response, String> {
    let stream = UnixStream::connect(SOCKET).map_err(|e| format!("Wirest helper not running: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(90))).ok();
    let mut line = serde_json::to_string(req).expect("request serializes");
    line.push('\n');
    (&stream).write_all(line.as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = String::new();
    BufReader::new(&stream).read_line(&mut buf).map_err(|e| e.to_string())?;
    let r: Response = serde_json::from_str(&buf).map_err(|e| format!("Bad reply from helper: {e}"))?;
    if r.ok {
        Ok(r)
    } else {
        Err(r.output)
    }
}

fn plist() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array><string>{HOME}/bin/wirest-helper</string><string>--helper</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>/var/log/wirest-helper.log</string>
</dict>
</plist>
"#
    )
}

/// Copies the tools and this binary to a root-owned location and starts the
/// helper. Shows the macOS admin password prompt.
pub fn install(tools: &Tools) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut sh = format!(
        "set -e; H={}; B=\"$H/bin\"; /bin/mkdir -p \"$B\" \"$H/tunnels\"; ",
        wg::shq(HOME)
    );
    for f in wg::TOOL_FILES {
        let src = tools.dir.join(f);
        if src.exists() {
            sh += &format!("/bin/cp -fL {} \"$B/{f}\"; ", wg::shq(&src.to_string_lossy()));
        }
    }
    sh += &format!("/bin/cp -f {} \"$B/wirest-helper\"; ", wg::shq(&exe.to_string_lossy()));
    sh += "/usr/sbin/chown -R root:wheel \"$H\"; /bin/chmod 755 \"$H\" \"$B\" \"$B\"/*; /bin/chmod 700 \"$H/tunnels\"; ";
    sh += &format!(
        "/usr/bin/printf '%s' {} > {PLIST}; /bin/chmod 644 {PLIST}; ",
        wg::shq(&plist())
    );
    sh += &format!(
        "/bin/launchctl bootout system/{LABEL} 2>/dev/null || true; /bin/rm -f {SOCKET}; \
         /bin/launchctl bootstrap system {PLIST}; "
    );
    // Clean up the sudoers rule earlier versions used.
    sh += "/bin/rm -f /etc/sudoers.d/wirest; ";
    sh += &format!(
        "for i in $(/usr/bin/seq 1 50); do [ -S {SOCKET} ] && exit 0; /bin/sleep 0.1; done; \
         echo 'The helper did not start; see /var/log/wirest-helper.log' >&2; exit 1"
    );
    wg::as_admin(
        &sh,
        "Wirest needs to install its network helper so it can connect without asking again.",
    )
    .map(|_| ())
}

pub fn uninstall() -> Result<(), String> {
    let sh = format!(
        "/bin/launchctl bootout system/{LABEL} 2>/dev/null || true; \
         /bin/rm -f {PLIST} {SOCKET}; /bin/rm -rf {}",
        wg::shq(HOME)
    );
    wg::as_admin(&sh, "Wirest wants to remove its network helper.").map(|_| ())
}

// ---- server side (root daemon) ----

pub fn run_daemon() -> ! {
    let tools = Arc::new(Tools::in_dir(Path::new(HOME).join("bin")));
    std::fs::remove_file(SOCKET).ok();
    let listener = match UnixListener::bind(SOCKET) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("wirest-helper: cannot bind {SOCKET}: {e}");
            std::process::exit(1);
        }
    };
    std::os::unix::fs::chown(SOCKET, Some(0), Some(STAFF_GID)).ok();
    std::fs::set_permissions(SOCKET, Permissions::from_mode(0o660)).ok();
    reap_orphans(&tools);
    let lock = Arc::new(Mutex::new(()));
    for stream in listener.incoming().flatten() {
        let (tools, lock) = (tools.clone(), lock.clone());
        std::thread::spawn(move || handle(stream, &tools, &lock));
    }
    std::process::exit(1);
}

fn handle(stream: UnixStream, tools: &Tools, lock: &Mutex<()>) {
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() {
        return;
    }
    let result = match serde_json::from_str::<Request>(&line) {
        Ok(Request::Version) => Ok(String::new()),
        Ok(Request::Up { name, config }) => {
            let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
            up(tools, &name, &config)
        }
        Ok(Request::Down { name }) => {
            let _g = lock.lock().unwrap_or_else(|e| e.into_inner());
            down(tools, &name)
        }
        Err(e) => Err(format!("bad request: {e}")),
    };
    let (ok, output) = match result {
        Ok(o) => (true, o),
        Err(e) => (false, e),
    };
    let resp = Response { ok, output, build: build_id(), active: active(tools) };
    let mut out = serde_json::to_string(&resp).expect("response serializes");
    out.push('\n');
    (&stream).write_all(out.as_bytes()).ok();
}

fn conf_path(name: &str) -> PathBuf {
    Path::new(HOME).join("tunnels").join(format!("{name}.conf"))
}

fn up(tools: &Tools, name: &str, text: &str) -> Result<String, String> {
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
    config::ensure_dir(path.parent().expect("has parent")).map_err(|e| e.to_string())?;
    config::write_secure(&path, text).map_err(|e| e.to_string())?;
    match wg::quick(tools, "up", &path) {
        Ok(o) => Ok(out + &o),
        Err(e) => {
            reap_orphans(tools);
            Err(e)
        }
    }
}

fn down(tools: &Tools, name: &str) -> Result<String, String> {
    if !config::valid_name(name) {
        return Err(format!("Invalid tunnel name {name:?}"));
    }
    let path = conf_path(name);
    if !path.exists() {
        return Err(format!("{name} wasn't started by Wirest"));
    }
    let result = wg::quick(tools, "down", &path);
    reap_orphans(tools);
    result
}

/// `wg-quick down` stops wireguard-go by deleting its UAPI socket, but
/// BoringTun doesn't notice and keeps the utun interface up. Stop any of our
/// BoringTun processes whose socket under RUN_DIR is gone.
fn reap_orphans(tools: &Tools) {
    let output = |cmd: &mut Command| {
        cmd.output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
    };
    let pids = output(Command::new("/usr/bin/pgrep").arg("-f").arg(tools.dir.join("boringtun-cli")));
    for pid in pids.split_whitespace() {
        // -F n prints each open file's name on a line starting with 'n'.
        let files = output(Command::new("/usr/sbin/lsof").args(["-a", "-p", pid, "-U", "-F", "n"]));
        let sockets: Vec<&str> = files
            .lines()
            .filter_map(|l| l.strip_prefix('n'))
            .filter(|n| n.starts_with(RUN_DIR))
            .collect();
        if !sockets.is_empty() && sockets.iter().all(|s| !Path::new(s).exists()) {
            Command::new("/bin/kill").arg(pid).status().ok();
        }
    }
}

/// Running tunnels: wg-quick's name files, kept only if the interface is
/// really up.
fn active(tools: &Tools) -> Vec<Active> {
    let dump = wg::show_dump(tools).unwrap_or_default();
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
}
