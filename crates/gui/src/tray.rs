//! Menu-bar icon, like Tailscale's: shows whether you're connected and
//! connects or disconnects in one click. Must live on the main thread.

use tray_icon::menu::{Menu, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub struct Tray {
    icon: TrayIcon,
    status: MenuItem,
    toggle: MenuItem,
    on: Icon,
    off: Icon,
    pub toggle_id: MenuId,
    pub open_id: MenuId,
    pub quit_id: MenuId,
    shown: Option<(bool, String, bool)>,
}

fn load(png: &[u8]) -> Result<Icon, String> {
    let d = eframe::icon_data::from_png_bytes(png).map_err(|e| e.to_string())?;
    Icon::from_rgba(d.rgba, d.width, d.height).map_err(|e| e.to_string())
}

impl Tray {
    pub fn new() -> Result<Self, String> {
        let on = load(include_bytes!("../../../assets/tray-on.png"))?;
        let off = load(include_bytes!("../../../assets/tray-off.png"))?;
        let status = MenuItem::new("Not connected", false, None);
        let toggle = MenuItem::new("Connect", false, None);
        let open = MenuItem::new("Open Wirest", true, None);
        let quit = MenuItem::new("Quit Wirest", true, None);
        let menu = Menu::new();
        menu.append_items(&[
            &status,
            &PredefinedMenuItem::separator(),
            &toggle,
            &open,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .map_err(|e| e.to_string())?;
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon_templated(off.clone())
            .with_tooltip("Wirest")
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            icon,
            toggle_id: toggle.id().clone(),
            open_id: open.id().clone(),
            quit_id: quit.id().clone(),
            status,
            toggle,
            on,
            off,
            shown: None,
        })
    }

    /// `status` is the first menu line; `can_toggle` enables Connect/Disconnect.
    pub fn update(&mut self, connected: bool, status: &str, can_toggle: bool) {
        let next = (connected, status.to_owned(), can_toggle);
        if self.shown.as_ref() == Some(&next) {
            return;
        }
        self.status.set_text(status);
        self.toggle.set_text(if connected { "Disconnect" } else { "Connect" });
        self.toggle.set_enabled(can_toggle);
        let icon = if connected { &self.on } else { &self.off };
        self.icon.set_icon_templated(Some(icon.clone())).ok();
        self.shown = Some(next);
    }
}
