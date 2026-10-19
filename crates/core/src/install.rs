//! Installing, checking and removing the root helper: a LaunchDaemon on
//! macOS, a systemd service on Linux. The helper runs its own root-owned
//! copy of the tools, so local users can't swap what runs as root.

use crate::ops::STATE_DIR;
use crate::protocol::{self, file_build_id, Request, Response, SOCKET};
use crate::wg::{self, Tools};
use std::path::PathBuf;
use std::process::Command;

pub const INSTALL_DIR: &str = if cfg!(target_os = "macos") {
    "/Library/Application Support/Wirest"
} else {
    "/usr/local/lib/wirest"
};
const LABEL: &str = "com.github.havban.wirest.helper";
const PLIST: &str = "/Library/LaunchDaemons/com.github.havban.wirest.helper.plist";
const UNIT: &str = "/etc/systemd/system/wirest-helper.service";
const HELPER_NAME: &str = "wirest-helper";

pub fn bin_dir() -> PathBuf {
    PathBuf::from(INSTALL_DIR).join("bin")
}

/// How to get root for installing or removing the helper.
#[derive(Clone, Copy)]
pub enum Elevation {
    /// The macOS administrator password dialog (GUI).
    AdminDialog,
    /// `sudo` in the current terminal, or nothing if already root (TUI/CLI).
    Terminal,
}

pub enum HelperStatus {
    Missing,
    /// Running, but not the build that ships with this frontend.
    Stale(Response),
    Ready(Response),
}

/// The helper binary shipped next to the running frontend.
pub fn helper_binary() -> Option<PathBuf> {
    let p = wg::exe_dir()?.join(HELPER_NAME);
    p.exists().then_some(p)
}

pub fn status() -> HelperStatus {
    match protocol::request(&Request::Version) {
        Err(_) => HelperStatus::Missing,
        Ok(r) if helper_binary().is_some_and(|b| file_build_id(&b) == r.build) => HelperStatus::Ready(r),
        Ok(r) => HelperStatus::Stale(r),
    }
}

pub fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

/// The group allowed to use the helper's socket: the installing user's.
/// When the install itself runs as root (sudo, or the macOS dialog), use the
/// sudo caller's group, or on macOS `staff` (every regular user).
fn socket_gid() -> u32 {
    if !is_root() {
        // SAFETY: getgid has no preconditions and cannot fail.
        return unsafe { libc::getgid() };
    }
    match std::env::var("SUDO_GID").ok().and_then(|g| g.parse().ok()) {
        Some(gid) => gid,
        None if cfg!(target_os = "macos") => 20,
        None => 0,
    }
}

fn copy_files(tools: &Tools, helper: &std::path::Path) -> String {
    let mut sh = format!(
        "set -e; H={}; B=\"$H/bin\"; /bin/mkdir -p \"$B\" {}; ",
        wg::shq(INSTALL_DIR),
        wg::shq(STATE_DIR)
    );
    for f in wg::TOOL_FILES {
        let src = tools.dir.join(f);
        if src.exists() {
            sh += &format!("/bin/cp -fL {} \"$B/{f}\"; ", wg::shq(&src.to_string_lossy()));
        }
    }
    sh += &format!("/bin/cp -f {} \"$B/{HELPER_NAME}\"; ", wg::shq(&helper.to_string_lossy()));
    sh
}

#[cfg(target_os = "macos")]
fn install_script(tools: &Tools, helper: &std::path::Path, gid: u32) -> String {
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array><string>{INSTALL_DIR}/bin/{HELPER_NAME}</string><string>--socket-gid</string><string>{gid}</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>/var/log/wirest-helper.log</string>
</dict>
</plist>
"#
    );
    let mut sh = copy_files(tools, helper);
    // Copies of a downloaded app carry its quarantine flag, which would stop
    // launchd from running the helper.
    sh += "/usr/bin/xattr -cr \"$H\"; ";
    sh += &format!(
        "/usr/sbin/chown -R root:wheel \"$H\"; /bin/chmod 755 \"$H\" \"$B\" \"$B\"/*; /bin/chmod 700 {}; ",
        wg::shq(STATE_DIR)
    );
    sh += &format!("/usr/bin/printf '%s' {} > {PLIST}; /bin/chmod 644 {PLIST}; ", wg::shq(&plist));
    sh += &format!(
        "/bin/launchctl bootout system/{LABEL} 2>/dev/null || true; /bin/rm -f {SOCKET}; \
         /bin/launchctl bootstrap system {PLIST}; "
    );
    sh + &wait_for_socket("/var/log/wirest-helper.log")
}

#[cfg(not(target_os = "macos"))]
fn install_script(tools: &Tools, helper: &std::path::Path, gid: u32) -> String {
    let unit = format!(
        "[Unit]\nDescription=Wirest helper (brings WireGuard tunnels up/down for Wirest)\nAfter=network.target\n\n\
         [Service]\nExecStart={INSTALL_DIR}/bin/{HELPER_NAME} --socket-gid {gid}\nRestart=always\n\n\
         [Install]\nWantedBy=multi-user.target\n"
    );
    let mut sh = String::from(
        "[ -d /run/systemd/system ] || { echo 'The Wirest helper needs systemd, which this system \
         does not run. Run wirest-tui as root instead (sudo wirest-tui).' >&2; exit 1; }; ",
    );
    sh += &copy_files(tools, helper);
    sh += &format!(
        "chown -R root:root \"$H\"; chmod 755 \"$H\" \"$B\" \"$B\"/*; chmod 700 /var/lib/wirest {}; ",
        wg::shq(STATE_DIR)
    );
    sh += &format!("printf '%s' {} > {UNIT}; chmod 644 {UNIT}; ", wg::shq(&unit));
    sh += &format!(
        "systemctl daemon-reload; systemctl stop wirest-helper.service 2>/dev/null || true; rm -f {SOCKET}; \
         systemctl enable --now wirest-helper.service >/dev/null 2>&1; "
    );
    sh + &wait_for_socket("journalctl -u wirest-helper")
}

fn wait_for_socket(log_hint: &str) -> String {
    format!(
        "for i in $(seq 1 50); do [ -S {SOCKET} ] && exit 0; sleep 0.1; done; \
         echo 'The helper did not start; see {log_hint}' >&2; exit 1"
    )
}

/// Copies the tools and the shipped helper binary to a root-owned location
/// and starts the helper.
pub fn install(tools: &Tools, how: Elevation) -> Result<(), String> {
    let helper = helper_binary().ok_or("wirest-helper is missing next to this program")?;
    let sh = install_script(tools, &helper, socket_gid());
    run_privileged(
        &sh,
        "Wirest needs to install its network helper so it can connect without asking again.",
        how,
    )
}

/// Disconnects tunnels started through the helper, then removes it.
pub fn uninstall(how: Elevation) -> Result<(), String> {
    if let Ok(r) = protocol::request(&Request::Version) {
        for a in r.active {
            protocol::request(&Request::Down { name: a.name }).ok();
        }
    }
    let sh = if cfg!(target_os = "macos") {
        format!(
            "/bin/launchctl bootout system/{LABEL} 2>/dev/null || true; \
             /bin/rm -f {PLIST} {SOCKET}; /bin/rm -rf {}",
            wg::shq(INSTALL_DIR)
        )
    } else {
        format!(
            "systemctl disable --now wirest-helper.service 2>/dev/null || true; rm -f {UNIT} {SOCKET}; \
             systemctl daemon-reload 2>/dev/null || true; rm -rf {} /var/lib/wirest",
            wg::shq(INSTALL_DIR)
        )
    };
    run_privileged(&sh, "Wirest wants to remove its network helper.", how)
}

fn run_privileged(sh: &str, prompt: &str, how: Elevation) -> Result<(), String> {
    match how {
        Elevation::AdminDialog => as_admin(sh, prompt),
        Elevation::Terminal => {
            let mut cmd = if is_root() {
                Command::new("sh")
            } else {
                eprintln!("{prompt}");
                let mut c = Command::new("sudo");
                c.args(["-p", "[sudo] password for %u: ", "sh"]);
                c
            };
            let status = cmd.args(["-c", sh]).status().map_err(|e| e.to_string())?;
            if status.success() {
                Ok(())
            } else {
                Err("Installing or removing the helper failed (see the messages above)".into())
            }
        }
    }
}

/// Runs a shell snippet as root behind the standard macOS password prompt.
fn as_admin(sh: &str, prompt: &str) -> Result<(), String> {
    let esc = |s: &str| s.replace('\\', r"\\").replace('"', "\\\"");
    let script = format!(
        "do shell script \"{}\" with prompt \"{}\" with administrator privileges",
        esc(sh),
        esc(prompt)
    );
    wg::run(Command::new("/usr/bin/osascript").arg("-e").arg(script))
        .map(|_| ())
        .map_err(|e| if e.contains("-128") { "Cancelled".into() } else { e })
}
