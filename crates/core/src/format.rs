//! Human-readable sizes and times, shared by the frontends.

use crate::wg::IfaceStats;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{n} B") } else { format!("{v:.2} {}", UNITS[u]) }
}

/// "5s ago", "3m 2s ago", …; "never" for 0.
pub fn ago(ts: u64) -> String {
    if ts == 0 {
        return "never".into();
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let d = now.saturating_sub(ts);
    match d {
        0..=59 => format!("{d}s ago"),
        60..=3599 => format!("{}m {}s ago", d / 60, d % 60),
        3600..=86399 => format!("{}h {}m ago", d / 3600, (d % 3600) / 60),
        _ => format!("{}d ago", d / 86400),
    }
}

/// Latest handshake across peers (0 = the server never answered).
pub fn last_handshake(s: &IfaceStats) -> u64 {
    s.peers.iter().map(|p| p.latest_handshake).max().unwrap_or(0)
}

pub fn traffic_summary(s: &IfaceStats) -> String {
    let rx: u64 = s.peers.iter().map(|p| p.rx).sum();
    let tx: u64 = s.peers.iter().map(|p| p.tx).sum();
    format!(
        "{} received · {} sent · last handshake {}",
        human_bytes(rx),
        human_bytes(tx),
        ago(last_handshake(s))
    )
}
