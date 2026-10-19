//! What the frontends and the root helper say to each other: one JSON
//! request line, one JSON response line, over a Unix socket.

use crate::wg::IfaceStats;
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// Where the helper listens.
pub const SOCKET: &str = if cfg!(target_os = "macos") { "/var/run/wirest.sock" } else { "/run/wirest.sock" };

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

/// Identifies a helper build. Frontends compare the running helper's id with
/// the helper binary they ship and reinstall when they differ. Releases use
/// the version only, so the GUI and TUI of one release share a helper; dev
/// builds add a hash of the binary so every rebuild gets installed.
pub fn file_build_id(path: &Path) -> String {
    let version = env!("CARGO_PKG_VERSION");
    if !cfg!(debug_assertions) {
        return version.to_owned();
    }
    let bytes = std::fs::read(path).unwrap_or_default();
    let mut h = std::hash::DefaultHasher::new();
    bytes.hash(&mut h);
    format!("{version}-{:016x}", h.finish())
}

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
