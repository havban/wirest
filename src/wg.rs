//! Talks to wireguard-tools (`wg`, `wg-quick`), elevating via macOS's admin
//! prompt or, when the user opted in, a NOPASSWD sudoers rule.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// GUI apps get a minimal PATH; the tools' own directory is prepended to this.
const PATH_ENV: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
const SUDOERS_FILE: &str = "/etc/sudoers.d/wirus";

#[derive(Clone)]
pub struct Tools {
    pub dir: PathBuf,
    pub wg: PathBuf,
    pub wg_quick: PathBuf,
    /// wg-quick needs bash >= 4; it is always run through this one so a
    /// reset PATH under sudo can't pick macOS's bash 3.2.
    pub bash: PathBuf,
    pub bundled: bool,
}

/// Candidate tool directories, most preferred first: inside the .app
/// bundle, the dev build from scripts/deps.sh, then Homebrew.
fn tool_dirs() -> Vec<(PathBuf, bool)> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(contents) = exe.parent().and_then(Path::parent) {
            dirs.push((contents.join("Resources/bin"), true));
        }
    }
    if cfg!(debug_assertions) {
        dirs.push((Path::new(env!("CARGO_MANIFEST_DIR")).join(".deps/out/bin"), true));
    }
    for d in ["/opt/homebrew/bin", "/usr/local/bin"] {
        dirs.push((PathBuf::from(d), false));
    }
    dirs
}

pub fn find_tools() -> Option<Tools> {
    tool_dirs().into_iter().find_map(|(dir, bundled)| {
        let t = Tools {
            wg: dir.join("wg"),
            wg_quick: dir.join("wg-quick"),
            bash: dir.join("bash"),
            dir,
            bundled,
        };
        (t.wg.exists() && t.wg_quick.exists() && t.bash.exists()).then_some(t)
    })
}

impl Tools {
    fn path_env(&self) -> String {
        format!("{}:{PATH_ENV}", self.dir.display())
    }
}

#[derive(Clone)]
pub struct PeerStats {
    pub public_key: String,
    pub endpoint: String,
    pub latest_handshake: u64,
    pub rx: u64,
    pub tx: u64,
}

#[derive(Clone)]
pub struct IfaceStats {
    pub iface: String,
    pub public_key: String,
    pub peers: Vec<PeerStats>,
}

fn run(cmd: &mut Command) -> Result<String, String> {
    match cmd.stdin(Stdio::null()).output() {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            if o.status.success() {
                Ok(s)
            } else {
                Err(s)
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn applescript_escape(s: &str) -> String {
    s.replace('\\', r"\\").replace('"', "\\\"")
}

/// Runs a shell snippet as root behind the standard macOS password prompt.
fn as_admin(sh: &str, prompt: &str) -> Result<String, String> {
    let script = format!(
        "do shell script \"{}\" with prompt \"{}\" with administrator privileges",
        applescript_escape(sh),
        applescript_escape(prompt)
    );
    run(Command::new("/usr/bin/osascript").arg("-e").arg(script))
}

/// `sudo -l` only says whether a command is permitted at all (any admin
/// is), so probe by running harmless commands without a password instead.
fn sudo_runs(cmd: &[&Path]) -> bool {
    Command::new("/usr/bin/sudo")
        .arg("-n")
        .args(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// True when both tools can be run as root without a password prompt.
pub fn sudo_allowed(t: &Tools) -> bool {
    sudo_runs(&[&t.bash, &t.wg_quick, Path::new("--help")])
        && sudo_runs(&[&t.wg, Path::new("show"), Path::new("all"), Path::new("dump")])
}

pub fn sudoers_installed() -> bool {
    Path::new(SUDOERS_FILE).exists()
}

/// `wg-quick up|down <conf>`.
pub fn quick(t: &Tools, sudo_ok: bool, verb: &str, conf: &Path) -> Result<String, String> {
    if sudo_ok {
        return run(Command::new("/usr/bin/sudo")
            .arg("-n")
            .arg(&t.bash)
            .arg(&t.wg_quick)
            .arg(verb)
            .arg(conf)
            .env("PATH", t.path_env()));
    }
    let sh = format!(
        "export PATH={}; {} {} {verb} {} 2>&1",
        shq(&t.path_env()),
        shq(&t.bash.to_string_lossy()),
        shq(&t.wg_quick.to_string_lossy()),
        shq(&conf.to_string_lossy())
    );
    as_admin(&sh, &format!("Wirus wants to bring a WireGuard tunnel {verb}."))
}

pub fn install_sudoers(t: &Tools) -> Result<String, String> {
    let user = std::env::var("USER").unwrap_or_default();
    if user.is_empty() || !user.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) {
        return Err(format!("unsupported user name {user:?}"));
    }
    // sudoers needs spaces in paths escaped.
    let esc = |p: &Path| p.display().to_string().replace(' ', "\\ ");
    let rule = format!(
        "{user} ALL=(root) NOPASSWD: {} {} *, {} show *",
        esc(&t.bash),
        esc(&t.wg_quick),
        esc(&t.wg)
    );
    let sh = format!(
        "f=$(/usr/bin/mktemp) && printf '%s\\n' {} > \"$f\" && /usr/sbin/visudo -cf \"$f\" >/dev/null \
         && /usr/bin/install -m 0440 -o root -g wheel \"$f\" {SUDOERS_FILE}; r=$?; /bin/rm -f \"$f\"; exit $r",
        shq(&rule)
    );
    as_admin(&sh, "Wirus wants to allow connecting WireGuard tunnels without a password.")
}

pub fn remove_sudoers() -> Result<String, String> {
    as_admin(
        &format!("/bin/rm -f {SUDOERS_FILE}"),
        "Wirus wants to remove its passwordless sudo rule.",
    )
}

/// Parses `wg show all dump`: interface lines have 5 tab-separated fields,
/// peer lines have 9.
pub fn show_dump(t: &Tools) -> Result<Vec<IfaceStats>, String> {
    let out = run(Command::new("/usr/bin/sudo")
        .arg("-n")
        .arg(&t.wg)
        .args(["show", "all", "dump"]))?;
    let mut ifaces: Vec<IfaceStats> = Vec::new();
    for line in out.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        match f.len() {
            5 => ifaces.push(IfaceStats {
                iface: f[0].into(),
                public_key: f[2].into(),
                peers: Vec::new(),
            }),
            9 => {
                if let Some(i) = ifaces.iter_mut().rev().find(|i| i.iface == f[0]) {
                    i.peers.push(PeerStats {
                        public_key: f[1].into(),
                        endpoint: f[3].into(),
                        latest_handshake: f[5].parse().unwrap_or(0),
                        rx: f[6].parse().unwrap_or(0),
                        tx: f[7].parse().unwrap_or(0),
                    });
                }
            }
            _ => {}
        }
    }
    Ok(ifaces)
}

/// Best effort status check when stats aren't available: wg-quick records
/// the utun mapping here, though the directory is usually root-only.
pub fn runtime_marker_exists(name: &str) -> bool {
    Path::new("/var/run/wireguard")
        .join(format!("{name}.name"))
        .exists()
}
