//! The user's tunnel configs on disk (see `config::tunnels_dir`).

use crate::config::{self, Parsed};
use std::path::{Path, PathBuf};

pub struct Tunnel {
    pub name: String,
    pub path: PathBuf,
    pub text: String,
    pub parsed: Parsed,
}

impl Tunnel {
    pub fn load(path: PathBuf) -> Option<Self> {
        let name = path.file_stem()?.to_str()?.to_owned();
        let text = std::fs::read_to_string(&path).ok()?;
        let parsed = config::parse(&text);
        Some(Self { name, path, text, parsed })
    }

    /// Complete enough to connect.
    pub fn ready(&self) -> bool {
        self.parsed.errors.is_empty()
    }
}

pub fn load_all(dir: &Path) -> Vec<Tunnel> {
    config::list_configs(dir).into_iter().filter_map(Tunnel::load).collect()
}

pub fn path_for(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.conf"))
}

/// A valid tunnel name based on `base` that isn't taken yet.
pub fn unique_name(dir: &Path, base: &str) -> String {
    let base = config::sanitize_name(base);
    if !path_for(dir, &base).exists() {
        return base;
    }
    (2..)
        .map(|i| {
            let suffix = format!("-{i}");
            let keep = base.len().min(15 - suffix.len());
            format!("{}{suffix}", &base[..keep])
        })
        .find(|n| !path_for(dir, n).exists())
        .expect("infinite range")
}

/// Saves a new tunnel; fails if the name is invalid or taken.
pub fn add(dir: &Path, name: &str, text: &str) -> Result<(), String> {
    if !config::valid_name(name) {
        return Err("Names are 1–15 characters: letters, digits, _ = + . -".into());
    }
    let path = path_for(dir, name);
    if path.exists() {
        return Err(format!("A VPN named {name} already exists"));
    }
    config::ensure_dir(dir).map_err(|e| e.to_string())?;
    config::write_secure(&path, text).map_err(|e| e.to_string())
}

/// Imports a .conf file under a free name derived from its file name.
pub fn import(dir: &Path, file: &Path) -> Result<String, String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("vpn");
    let name = unique_name(dir, stem);
    add(dir, &name, &text)?;
    Ok(name)
}
