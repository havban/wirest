//! Shared Wirest logic used by the GUI, the TUI and the root helper.

pub mod config;
pub mod daemon;
pub mod format;
pub mod install;
pub mod ops;
pub mod protocol;
pub mod store;
pub mod wg;

use install::{Elevation, HelperStatus};
use protocol::{Active, Request};
use wg::Tools;

/// How a frontend controls tunnels: directly when it already runs as root
/// (e.g. `sudo wirest-tui` on a server), otherwise through the helper.
#[derive(Clone)]
pub enum Backend {
    Direct(Tools),
    Helper { tools: Option<Tools>, elevation: Elevation },
}

impl Backend {
    pub fn detect(elevation: Elevation) -> Self {
        match wg::find_tools() {
            Some(t) if install::is_root() => Backend::Direct(t),
            tools => Backend::Helper { tools, elevation },
        }
    }

    pub fn tools(&self) -> Option<&Tools> {
        match self {
            Backend::Direct(t) => Some(t),
            Backend::Helper { tools, .. } => tools.as_ref(),
        }
    }

    /// Whether connecting would first need to install or update the helper
    /// (and so ask for a password).
    pub fn needs_helper_install(&self) -> bool {
        matches!(self, Backend::Helper { .. }) && !matches!(install::status(), HelperStatus::Ready(_))
    }

    /// Running tunnels; empty if the helper isn't installed yet.
    pub fn active(&self) -> Vec<Active> {
        match self {
            Backend::Direct(t) => ops::active(t),
            Backend::Helper { .. } => protocol::request(&Request::Version).map(|r| r.active).unwrap_or_default(),
        }
    }

    /// Installs or updates the helper if needed (may prompt for a password).
    pub fn ensure_helper(&self) -> Result<(), String> {
        let Backend::Helper { tools, elevation } = self else { return Ok(()) };
        if matches!(install::status(), HelperStatus::Ready(_)) {
            return Ok(());
        }
        let tools = tools.as_ref().ok_or("WireGuard tools are missing from this build")?;
        install::install(tools, *elevation)
    }

    /// Connects (replacing any other Wirest tunnel). Returns wg-quick's output.
    pub fn up(&self, name: &str, config: &str) -> Result<(String, Vec<Active>), String> {
        match self {
            Backend::Direct(t) => ops::up(t, name, config).map(|o| (o, ops::active(t))),
            Backend::Helper { .. } => {
                self.ensure_helper()?;
                protocol::request(&Request::Up { name: name.into(), config: config.into() })
                    .map(|r| (r.output, r.active))
            }
        }
    }

    pub fn down(&self, name: &str) -> Result<(String, Vec<Active>), String> {
        match self {
            Backend::Direct(t) => ops::down(t, name).map(|o| (o, ops::active(t))),
            Backend::Helper { .. } => {
                self.ensure_helper()?;
                protocol::request(&Request::Down { name: name.into() }).map(|r| (r.output, r.active))
            }
        }
    }
}
