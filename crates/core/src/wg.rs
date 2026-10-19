//! Locates and runs wireguard-tools (`wg`, `wg-quick`). The run functions
//! need root; they are called by the helper daemon, or directly when a
//! frontend already runs as root.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// System directories appended after the tools' own directory in PATH.
const PATH_ENV: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Everything wg-quick needs, copied into the helper's root-owned bin dir.
pub const TOOL_FILES: [&str; 5] = ["wg", "wg-quick", "bash", "wireguard-go", "boringtun-cli"];

#[derive(Clone)]
pub struct Tools {
    pub dir: PathBuf,
    pub wg: PathBuf,
    pub wg_quick: PathBuf,
    /// wg-quick needs bash >= 4; it is always run through this one rather
    /// than whatever `bash` the system has (macOS ships 3.2, Alpine none).
    pub bash: PathBuf,
}

impl Tools {
    pub fn in_dir(dir: PathBuf) -> Self {
        Self {
            wg: dir.join("wg"),
            wg_quick: dir.join("wg-quick"),
            bash: dir.join("bash"),
            dir,
        }
    }

    fn exists(&self) -> bool {
        self.wg.exists() && self.wg_quick.exists() && self.bash.exists()
    }

    fn path_env(&self) -> String {
        format!("{}:{PATH_ENV}", self.dir.display())
    }
}

/// Directory of the running executable.
pub fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

/// Candidate tool directories, most preferred first: inside the macOS .app
/// bundle, next to the executable (release tarballs), the dev build from
/// scripts/deps.sh, then system installs.
fn tool_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(exe) = exe_dir() {
        if let Some(contents) = exe.parent() {
            dirs.push(contents.join("Resources/bin"));
        }
        dirs.push(exe);
    }
    if cfg!(debug_assertions) {
        let plat = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
        dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.deps").join(plat).join("bin"));
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from));
    dirs
}

/// Tools to run directly (as root) or to hand to the helper installer.
pub fn find_tools() -> Option<Tools> {
    tool_dirs().into_iter().map(Tools::in_dir).find(Tools::exists)
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PeerStats {
    pub public_key: String,
    pub endpoint: String,
    pub latest_handshake: u64,
    pub rx: u64,
    pub tx: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct IfaceStats {
    pub iface: String,
    pub public_key: String,
    pub peers: Vec<PeerStats>,
}

pub(crate) fn run(cmd: &mut Command) -> Result<String, String> {
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

pub fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `wg-quick up|down <conf>`; must run as root.
pub fn quick(t: &Tools, verb: &str, conf: &Path) -> Result<String, String> {
    run(Command::new(&t.bash)
        .arg(&t.wg_quick)
        .arg(verb)
        .arg(conf)
        .env("PATH", t.path_env()))
}

/// Parses `wg show all dump` (root only): interface lines have 5
/// tab-separated fields, peer lines have 9.
pub fn show_dump(t: &Tools) -> Result<Vec<IfaceStats>, String> {
    let out = run(Command::new(&t.wg).args(["show", "all", "dump"]))?;
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
