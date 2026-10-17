use crate::config::{self, Parsed};
use crate::helper::{self, Active, Request};
use crate::tray::Tray;
use crate::wg::{self, IfaceStats, Tools};
use eframe::egui::{self, Color32, RichText};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tray_icon::menu::MenuEvent;

const GREEN: Color32 = Color32::from_rgb(52, 199, 89);
const GRAY: Color32 = Color32::from_rgb(142, 142, 147);
const ORANGE: Color32 = Color32::from_rgb(255, 159, 10);
const RED: Color32 = Color32::from_rgb(255, 69, 58);
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const LOG_LIMIT: usize = 500;

#[derive(Clone, Copy, PartialEq)]
enum HelperState {
    Unknown,
    Missing,
    Stale,
    Ready,
}

enum Msg {
    Log(String),
    /// Tunnel being switched, and whether it's connecting.
    Busy(Option<(String, bool)>),
    Error(Option<String>),
    Status { helper: HelperState, active: Vec<Active> },
}

enum Job {
    Up { name: String, config: String },
    Down { name: String },
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Overview,
    Edit,
}

enum Action {
    Select(String),
    Toggle(String),
    OpenNew,
    CreateNew,
    CancelNew,
    OpenPaste,
    SavePaste,
    CancelPaste,
    Import(Vec<PathBuf>),
    PickFiles,
    Save,
    Revert,
    AskDelete,
    CancelDelete,
    Delete,
    ShowLog,
    UninstallHelper,
}

struct Tunnel {
    name: String,
    path: PathBuf,
    text: String,
    parsed: Parsed,
    public_key: Option<String>,
}

impl Tunnel {
    fn load(path: PathBuf) -> Option<Self> {
        let name = path.file_stem()?.to_str()?.to_owned();
        let text = std::fs::read_to_string(&path).ok()?;
        let parsed = config::parse(&text);
        let public_key = parsed
            .interface
            .private_key
            .as_deref()
            .and_then(config::public_key);
        Some(Self { name, path, text, parsed, public_key })
    }
}

struct NewDialog {
    name: String,
    error: Option<String>,
}

struct PasteDialog {
    name: String,
    text: String,
    error: Option<String>,
}

/// What the menu-bar handler needs; it runs outside `update`, so the app
/// mirrors its state here every frame.
#[derive(Default)]
struct TrayTarget {
    connected: Option<String>,
    selected: Option<(String, String)>,
    tools: Option<Tools>,
    busy: bool,
}

pub struct App {
    logo: egui::TextureHandle,
    dir: PathBuf,
    tunnels: Vec<Tunnel>,
    selected: Option<String>,
    advanced: bool,
    tab: Tab,
    editor: String,
    editor_name: String,
    notice: Option<String>,
    confirm_delete: bool,
    new_dialog: Option<NewDialog>,
    paste: Option<PasteDialog>,

    tools: Option<Tools>,
    helper: HelperState,
    active: Vec<Active>,
    busy: Option<(String, bool)>,
    error: Option<String>,

    log: Vec<String>,
    show_log: bool,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    tray: Option<Tray>,
    tray_target: Arc<Mutex<TrayTarget>>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let dir = config::tunnels_dir();
        let (tx, rx) = channel();
        let tools = wg::find_tools();
        let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png"))
            .expect("bundled icon is a valid PNG");
        let logo = cc.egui_ctx.load_texture(
            "logo",
            egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            ),
            egui::TextureOptions::LINEAR,
        );
        let tray = Tray::new().inspect_err(|e| eprintln!("menu-bar icon unavailable: {e}")).ok();
        let tray_target = Arc::new(Mutex::new(TrayTarget::default()));
        if let Some(t) = &tray {
            install_menu_handler(t, tray_target.clone(), tx.clone(), cc.egui_ctx.clone());
        }
        let mut app = Self {
            logo,
            dir,
            tunnels: Vec::new(),
            selected: None,
            advanced: false,
            tab: Tab::Overview,
            editor: String::new(),
            editor_name: String::new(),
            notice: None,
            confirm_delete: false,
            new_dialog: None,
            paste: None,
            tools,
            helper: HelperState::Unknown,
            active: Vec::new(),
            busy: None,
            error: None,
            log: Vec::new(),
            show_log: false,
            tx,
            rx,
            tray,
            tray_target,
        };
        if let Err(e) = config::ensure_dir(&app.dir) {
            app.push_log(format!("Cannot create {}: {e}", app.dir.display()));
        }
        if app.tray.is_some() {
            app.push_log("Menu-bar icon ready".into());
        }
        match &app.tools {
            Some(t) => app.push_log(format!("WireGuard tools: {}", t.dir.display())),
            None => app.push_log("WireGuard tools not found; build them with ./run.sh".into()),
        }
        spawn_poller(app.tx.clone(), cc.egui_ctx.clone());
        app.reload();
        app.ensure_selection();
        app
    }

    fn push_log(&mut self, line: String) {
        self.log.push(line);
        if self.log.len() > LOG_LIMIT {
            self.log.drain(..self.log.len() - LOG_LIMIT);
        }
    }

    fn reload(&mut self) {
        self.tunnels = config::list_configs(&self.dir)
            .into_iter()
            .filter_map(Tunnel::load)
            .collect();
    }

    fn find(&self, name: &str) -> Option<&Tunnel> {
        self.tunnels.iter().find(|t| t.name == name)
    }

    fn select(&mut self, name: String) {
        if let Some((text, n)) = self.find(&name).map(|t| (t.text.clone(), t.name.clone())) {
            self.editor = text;
            self.editor_name = n;
        }
        self.selected = Some(name);
        self.notice = None;
        self.confirm_delete = false;
    }

    /// Keeps a valid selection: the connected tunnel if any, else the first.
    fn ensure_selection(&mut self) {
        if self.selected.as_deref().is_some_and(|n| self.find(n).is_some()) {
            return;
        }
        let pick = self
            .active
            .iter()
            .map(|a| a.name.clone())
            .find(|n| self.find(n).is_some())
            .or_else(|| self.tunnels.first().map(|t| t.name.clone()));
        match pick {
            Some(n) => self.select(n),
            None => self.selected = None,
        }
    }

    fn active_for(&self, name: &str) -> Option<&Active> {
        self.active.iter().find(|a| a.name == name)
    }

    fn is_up(&self, name: &str) -> bool {
        self.active_for(name).is_some()
    }

    fn is_busy(&self, name: &str) -> bool {
        self.busy.as_ref().is_some_and(|(n, _)| n == name)
    }

    /// The tunnel Wirest has up, if any.
    fn connected(&self) -> Option<&Active> {
        self.active.iter().find(|a| self.find(&a.name).is_some())
    }

    fn drain(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Log(l) => self.push_log(l),
                Msg::Busy(b) => self.busy = b,
                Msg::Error(e) => {
                    if let Some(e) = &e {
                        self.push_log(format!("Error: {e}"));
                    }
                    self.error = e;
                }
                Msg::Status { helper, active } => {
                    self.helper = helper;
                    self.active = active;
                    self.ensure_selection();
                }
            }
        }
    }

    fn apply(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::Select(name) => self.select(name),
            Action::Toggle(name) => self.toggle(&name, ctx),
            Action::OpenNew => {
                self.new_dialog = Some(NewDialog { name: self.unique_name("wg"), error: None })
            }
            Action::CancelNew => self.new_dialog = None,
            Action::CreateNew => self.create_new(),
            Action::OpenPaste => {
                self.paste = Some(PasteDialog { name: self.unique_name("vpn"), text: String::new(), error: None })
            }
            Action::CancelPaste => self.paste = None,
            Action::SavePaste => self.save_paste(),
            Action::Import(paths) => self.import(paths),
            Action::PickFiles => {
                if let Some(files) = rfd::FileDialog::new()
                    .add_filter("WireGuard config", &["conf"])
                    .pick_files()
                {
                    self.import(files);
                }
            }
            Action::Save => self.save(),
            Action::Revert => {
                if let Some(n) = self.selected.clone() {
                    self.select(n);
                }
            }
            Action::AskDelete => self.confirm_delete = true,
            Action::CancelDelete => self.confirm_delete = false,
            Action::Delete => self.delete(),
            Action::ShowLog => {
                self.advanced = true;
                self.show_log = true;
            }
            Action::UninstallHelper => {
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let line = match helper::uninstall() {
                        Ok(()) => "Helper removed".to_string(),
                        Err(e) => format!("Removing helper failed: {}", e.trim()),
                    };
                    tx.send(Msg::Log(line)).ok();
                });
            }
        }
    }

    fn toggle(&mut self, name: &str, ctx: &egui::Context) {
        let Some(t) = self.find(name) else { return };
        let job = if self.is_up(name) {
            Job::Down { name: name.to_owned() }
        } else if let Some(e) = t.parsed.errors.first() {
            self.error = Some(e.clone());
            return;
        } else {
            Job::Up { name: name.to_owned(), config: t.text.clone() }
        };
        spawn_job(job, self.tools.clone(), self.tx.clone(), ctx.clone());
    }

    fn path_for(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.conf"))
    }

    fn unique_name(&self, base: &str) -> String {
        let base = config::sanitize_name(base);
        if !self.path_for(&base).exists() {
            return base;
        }
        (2..)
            .map(|i| {
                let suffix = format!("-{i}");
                let keep = base.len().min(15 - suffix.len());
                format!("{}{suffix}", &base[..keep])
            })
            .find(|n| !self.path_for(n).exists())
            .expect("infinite range")
    }

    /// Validates a new tunnel name; returns the error to show.
    fn check_new_name(&self, name: &str) -> Option<String> {
        if !config::valid_name(name) {
            Some("Use 1–15 characters: letters, digits, _ = + . -".into())
        } else if self.path_for(name).exists() {
            Some("A VPN with this name already exists.".into())
        } else {
            None
        }
    }

    fn create_new(&mut self) {
        let Some(name) = self.new_dialog.as_ref().map(|d| d.name.trim().to_string()) else { return };
        let text = config::template(&config::generate_private_key());
        let err = self
            .check_new_name(&name)
            .or_else(|| config::write_secure(&self.path_for(&name), &text).err().map(|e| e.to_string()));
        if let Some(e) = err {
            if let Some(d) = self.new_dialog.as_mut() {
                d.error = Some(e);
            }
            return;
        }
        self.new_dialog = None;
        self.reload();
        self.select(name);
        self.tab = Tab::Edit;
    }

    fn save_paste(&mut self) {
        let Some((name, text)) = self.paste.as_ref().map(|d| (d.name.trim().to_string(), d.text.clone())) else {
            return;
        };
        let err = if text.trim().is_empty() {
            Some("Paste the configuration text first.".into())
        } else {
            self.check_new_name(&name)
                .or_else(|| config::write_secure(&self.path_for(&name), &text).err().map(|e| e.to_string()))
        };
        if let Some(e) = err {
            if let Some(d) = self.paste.as_mut() {
                d.error = Some(e);
            }
            return;
        }
        self.paste = None;
        self.push_log(format!("Added {name}"));
        self.reload();
        self.select(name);
    }

    fn import(&mut self, paths: Vec<PathBuf>) {
        let mut last = None;
        for p in paths {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("vpn");
            let text = match std::fs::read_to_string(&p) {
                Ok(t) => t,
                Err(e) => {
                    self.push_log(format!("Import {}: {e}", p.display()));
                    continue;
                }
            };
            let name = self.unique_name(stem);
            match config::write_secure(&self.path_for(&name), &text) {
                Ok(()) => {
                    self.push_log(format!("Imported {} as {name}", p.display()));
                    last = Some(name);
                }
                Err(e) => self.push_log(format!("Import {}: {e}", p.display())),
            }
        }
        self.reload();
        if let Some(n) = last {
            self.select(n);
            self.tab = Tab::Overview;
        }
    }

    fn save(&mut self) {
        let Some(old) = self.selected.clone().and_then(|n| self.find(&n).map(|t| (t.name.clone(), t.path.clone()))) else {
            return;
        };
        let new_name = self.editor_name.trim().to_string();
        if !config::valid_name(&new_name) {
            self.notice = Some("Name must be 1–15 characters: letters, digits, _ = + . -".into());
            return;
        }
        let new_path = self.path_for(&new_name);
        if new_name != old.0 && new_path.exists() {
            self.notice = Some(format!("A VPN named {new_name} already exists."));
            return;
        }
        if let Err(e) = config::write_secure(&new_path, &self.editor) {
            self.notice = Some(format!("Save failed: {e}"));
            return;
        }
        if new_name != old.0 {
            std::fs::remove_file(&old.1).ok();
        }
        self.reload();
        self.select(new_name);
    }

    fn delete(&mut self) {
        let Some(t) = self.selected.as_deref().and_then(|n| self.find(n)) else { return };
        let (name, path) = (t.name.clone(), t.path.clone());
        if let Err(e) = std::fs::remove_file(&path) {
            self.notice = Some(format!("Delete failed: {e}"));
            return;
        }
        self.push_log(format!("Deleted {name}"));
        self.reload();
        self.selected = None;
        self.ensure_selection();
        self.confirm_delete = false;
    }

    /// Mirrors state into the menu-bar icon and its click handler.
    fn sync_tray(&mut self) {
        if self.tray.is_none() {
            return;
        }
        let connected = self.connected().map(|a| a.name.clone());
        let selected = self
            .selected
            .as_deref()
            .and_then(|n| self.find(n))
            .filter(|t| t.parsed.errors.is_empty())
            .map(|t| (t.name.clone(), t.text.clone()));
        let status = match (&self.busy, &connected) {
            (Some((n, true)), _) => format!("Connecting to {n}…"),
            (Some((_, false)), _) => "Disconnecting…".to_string(),
            (None, Some(n)) => format!("Connected to {n}"),
            (None, None) => "Not connected".to_string(),
        };
        let can_toggle = self.busy.is_none() && (connected.is_some() || selected.is_some());
        if let Some(tray) = &mut self.tray {
            tray.update(connected.is_some(), &status, can_toggle);
        }
        let mut t = self.tray_target.lock().unwrap_or_else(|e| e.into_inner());
        t.connected = connected;
        t.selected = selected;
        t.tools = self.tools.clone();
        t.busy = self.busy.is_some();
    }
}

fn install_menu_handler(tray: &Tray, target: Arc<Mutex<TrayTarget>>, tx: Sender<Msg>, ctx: egui::Context) {
    let (toggle, open, quit) = (tray.toggle_id.clone(), tray.open_id.clone(), tray.quit_id.clone());
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == quit {
            std::process::exit(0);
        } else if e.id == open {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.request_repaint();
        } else if e.id == toggle {
            let t = target.lock().unwrap_or_else(|e| e.into_inner());
            if t.busy {
                return;
            }
            let job = match (&t.connected, &t.selected) {
                (Some(name), _) => Job::Down { name: name.clone() },
                (None, Some((name, config))) => Job::Up { name: name.clone(), config: config.clone() },
                (None, None) => return,
            };
            spawn_job(job, t.tools.clone(), tx.clone(), ctx.clone());
        }
    }));
}

/// Connects or disconnects through the helper, installing (or updating) it
/// first if needed — the only time a password is asked for.
fn spawn_job(job: Job, tools: Option<Tools>, tx: Sender<Msg>, ctx: egui::Context) {
    let (name, connecting) = match &job {
        Job::Up { name, .. } => (name.clone(), true),
        Job::Down { name } => (name.clone(), false),
    };
    tx.send(Msg::Busy(Some((name.clone(), connecting)))).ok();
    tx.send(Msg::Error(None)).ok();
    ctx.request_repaint();
    std::thread::spawn(move || {
        let result = (|| {
            let current = helper::request(&Request::Version).is_ok_and(|r| r.build == helper::build_id());
            if !current {
                let tools = tools.ok_or_else(|| "WireGuard tools are missing from this build".to_string())?;
                tx.send(Msg::Log("Installing the Wirest helper…".into())).ok();
                helper::install(&tools)?;
                tx.send(Msg::Log("Helper installed".into())).ok();
            }
            helper::request(&match job {
                Job::Up { name, config } => Request::Up { name, config },
                Job::Down { name } => Request::Down { name },
            })
        })();
        match result {
            Ok(r) => {
                for l in r.output.lines().filter(|l| !l.trim().is_empty()) {
                    tx.send(Msg::Log(format!("[{name}] {l}"))).ok();
                }
                tx.send(Msg::Status { helper: HelperState::Ready, active: r.active }).ok();
            }
            Err(e) => {
                let verb = if connecting { "connect" } else { "disconnect" };
                let e = e.trim();
                let msg = if e == "Cancelled" { e.to_string() } else { format!("Couldn't {verb}: {e}") };
                tx.send(Msg::Error(Some(msg))).ok();
            }
        }
        tx.send(Msg::Busy(None)).ok();
        ctx.request_repaint();
    });
}

fn spawn_poller(tx: Sender<Msg>, ctx: egui::Context) {
    std::thread::spawn(move || loop {
        let msg = match helper::request(&Request::Version) {
            Ok(r) => Msg::Status {
                helper: if r.build == helper::build_id() { HelperState::Ready } else { HelperState::Stale },
                active: r.active,
            },
            Err(_) => Msg::Status { helper: HelperState::Missing, active: Vec::new() },
        };
        if tx.send(msg).is_err() {
            return;
        }
        ctx.request_repaint();
        std::thread::sleep(POLL_INTERVAL);
    });
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{n} B") } else { format!("{v:.2} {}", UNITS[u]) }
}

fn ago(ts: u64) -> String {
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

fn status_dot(ui: &mut egui::Ui, up: bool, busy: bool) {
    let color = if busy { ORANGE } else if up { GREEN } else { GRAY };
    paint_dot(ui, color);
}

fn paint_dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

fn row(ui: &mut egui::Ui, label: &str, value: impl Into<egui::WidgetText>) {
    ui.label(RichText::new(label).color(GRAY));
    ui.label(value);
    ui.end_row();
}

fn mono(s: &str) -> RichText {
    RichText::new(s).monospace()
}

fn key_row(ui: &mut egui::Ui, label: &str, key: &str) {
    ui.label(RichText::new(label).color(GRAY));
    ui.horizontal(|ui| {
        ui.label(mono(key));
        if ui.small_button("Copy").clicked() {
            ui.ctx().copy_text(key.to_owned());
        }
    });
    ui.end_row();
}

/// Lays out `width` worth of widgets horizontally, centered.
fn centered_row(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
        add(ui);
    });
}

fn traffic_summary(s: &IfaceStats) -> String {
    let rx: u64 = s.peers.iter().map(|p| p.rx).sum();
    let tx: u64 = s.peers.iter().map(|p| p.tx).sum();
    let hs = s.peers.iter().map(|p| p.latest_handshake).max().unwrap_or(0);
    format!("{} received · {} sent · handshake {}", human_bytes(rx), human_bytes(tx), ago(hs))
}

impl App {
    fn simple_view(&mut self, ui: &mut egui::Ui, act: &mut Option<Action>) {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.08).min(40.0));
            ui.add(
                egui::Image::new(egui::load::SizedTexture::from_handle(&self.logo))
                    .fit_to_exact_size(egui::vec2(110.0, 110.0)),
            );
            ui.add_space(8.0);

            // Unfinished configs (e.g. a fresh template) only show up in Advanced.
            let ready: Vec<String> = self
                .tunnels
                .iter()
                .filter(|t| t.parsed.errors.is_empty() || self.is_up(&t.name))
                .map(|t| t.name.clone())
                .collect();
            if ready.is_empty() {
                ui.label(RichText::new("Add your VPN").size(24.0).strong());
                ui.add_space(6.0);
                ui.label(
                    RichText::new("Drop the .conf file from your VPN provider onto this window,\nor choose one of these:")
                        .color(GRAY),
                );
                ui.add_space(14.0);
                centered_row(ui, 250.0, |ui| {
                    if ui.add(egui::Button::new("Import file…").min_size(egui::vec2(120.0, 32.0))).clicked() {
                        *act = Some(Action::PickFiles);
                    }
                    if ui.add(egui::Button::new("Paste config…").min_size(egui::vec2(120.0, 32.0))).clicked() {
                        *act = Some(Action::OpenPaste);
                    }
                });
                return;
            }

            let name = self
                .selected
                .clone()
                .filter(|n| ready.contains(n))
                .unwrap_or_else(|| ready[0].clone());
            let Some(t) = self.find(&name) else { return };
            let up = self.is_up(&t.name);
            let busy = self.busy.clone();
            let (headline, color) = match &busy {
                Some((_, true)) => ("Connecting…", ORANGE),
                Some((_, false)) => ("Disconnecting…", ORANGE),
                None if up => ("Connected", GREEN),
                None => ("Not connected", GRAY),
            };
            ui.label(RichText::new(headline).size(28.0).strong().color(color));
            let endpoint = t.parsed.peers.first().and_then(|p| p.endpoint.as_deref()).unwrap_or("");
            ui.label(RichText::new(format!("{}  {endpoint}", t.name)).color(GRAY));
            ui.add_space(18.0);

            let blocked = !up && !t.parsed.errors.is_empty();
            let label = if up { "Disconnect" } else { "Connect" };
            let mut btn = egui::Button::new(RichText::new(label).size(18.0).strong())
                .min_size(egui::vec2(220.0, 52.0))
                .corner_radius(26.0);
            if !up && !blocked {
                btn = btn.fill(GREEN.gamma_multiply(0.7));
            }
            if ui.add_enabled(busy.is_none() && !blocked, btn).clicked() {
                *act = Some(Action::Toggle(t.name.clone()));
            }
            ui.add_space(10.0);

            if let Some(a) = self.active_for(&t.name) {
                ui.label(RichText::new(traffic_summary(&a.stats)).color(GRAY).small());
            } else if self.helper == HelperState::Missing && busy.is_none() {
                ui.label(
                    RichText::new("The first time, macOS asks for your password to install Wirest's helper.")
                        .color(GRAY)
                        .small(),
                );
            }
            if let Some(e) = &self.error {
                ui.add_space(4.0);
                ui.label(RichText::new(e).color(RED));
                if ui.small_button("Show log").clicked() {
                    *act = Some(Action::ShowLog);
                }
            }

            ui.add_space(22.0);
            if ready.len() > 1 {
                let mut choice = t.name.clone();
                centered_row(ui, 220.0, |ui| {
                    ui.label(RichText::new("VPN").color(GRAY));
                    egui::ComboBox::from_id_salt("vpn-choice")
                        .selected_text(&choice)
                        .width(170.0)
                        .show_ui(ui, |ui| {
                            for n in &ready {
                                ui.selectable_value(&mut choice, n.clone(), n);
                            }
                        });
                });
                if choice != t.name {
                    *act = Some(Action::Select(choice));
                }
                ui.add_space(8.0);
            }
            centered_row(ui, 230.0, |ui| {
                if ui.small_button("Add VPN from file…").clicked() {
                    *act = Some(Action::PickFiles);
                }
                if ui.small_button("Paste config…").clicked() {
                    *act = Some(Action::OpenPaste);
                }
            });
        });
    }

    fn advanced_view(&mut self, ctx: &egui::Context, act: &mut Option<Action>) {
        if self.show_log {
            egui::TopBottomPanel::bottom("log")
                .resizable(true)
                .default_height(140.0)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.strong("Log");
                        if ui.small_button("Clear").clicked() {
                            self.log.clear();
                        }
                    });
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for l in &self.log {
                                ui.label(mono(l).small());
                            }
                        });
                });
        }

        egui::SidePanel::left("tunnels")
            .resizable(true)
            .default_width(220.0)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("+ New").clicked() {
                        *act = Some(Action::OpenNew);
                    }
                    if ui.button("Import…").clicked() {
                        *act = Some(Action::PickFiles);
                    }
                    if ui.button("Paste…").clicked() {
                        *act = Some(Action::OpenPaste);
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    if self.tunnels.is_empty() {
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new("No tunnels yet.\nCreate one, import a file,\nor drop a .conf here.")
                                .color(GRAY),
                        );
                    }
                    for t in &self.tunnels {
                        let selected = self.selected.as_deref() == Some(t.name.as_str());
                        ui.horizontal(|ui| {
                            status_dot(ui, self.is_up(&t.name), self.is_busy(&t.name));
                            if ui.selectable_label(selected, &t.name).clicked() {
                                *act = Some(Action::Select(t.name.clone()));
                            }
                        });
                    }
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            let selected = self.selected.as_deref();
            let Some(t) = selected.and_then(|n| self.tunnels.iter().find(|t| t.name == n)) else {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("Select or create a tunnel").color(GRAY));
                });
                return;
            };
            let up = self.is_up(&t.name);
            let busy = self.is_busy(&t.name);
            let stats = self.active_for(&t.name).map(|a| a.stats.clone());

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading(&t.name);
                status_dot(ui, up, busy);
                let state = if busy { "Working…" } else if up { "Connected" } else { "Not connected" };
                ui.label(RichText::new(state).color(GRAY));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if up { "Disconnect" } else { "Connect" };
                    // A broken config can't come up, but an active tunnel can always go down.
                    let blocked = !up && !t.parsed.errors.is_empty();
                    let btn = egui::Button::new(RichText::new(label).strong())
                        .min_size(egui::vec2(110.0, 28.0));
                    let btn = if up || blocked { btn } else { btn.fill(GREEN.gamma_multiply(0.55)) };
                    let resp = ui.add_enabled(self.busy.is_none() && !blocked, btn);
                    if resp.clicked() {
                        *act = Some(Action::Toggle(t.name.clone()));
                    }
                    if blocked {
                        resp.on_disabled_hover_text(t.parsed.errors.join("\n"));
                        let n = t.parsed.errors.len();
                        let issues = if n == 1 { "1 issue" } else { &format!("{n} issues") };
                        ui.label(RichText::new(format!("Fix {issues} to connect")).color(ORANGE).small());
                    }
                    if busy {
                        ui.spinner();
                    }
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Overview, "Overview");
                ui.selectable_value(&mut self.tab, Tab::Edit, "Edit");
            });
            ui.separator();

            for n in self.notice.iter().chain(self.error.iter()) {
                ui.label(RichText::new(n).color(RED));
            }

            match self.tab {
                Tab::Overview => overview(ui, t, up, stats.as_ref()),
                Tab::Edit => {
                    let dirty = t.text != self.editor || t.name != self.editor_name;
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        ui.add_enabled(
                            !up,
                            egui::TextEdit::singleline(&mut self.editor_name).desired_width(160.0),
                        )
                        .on_disabled_hover_text("Disconnect to rename it");
                    });
                    ui.add_space(4.0);
                    let errors = config::parse(&self.editor).errors;
                    let reserve = 44.0 + 18.0 * errors.len().min(6) as f32;
                    egui::ScrollArea::vertical()
                        .max_height((ui.available_height() - reserve).max(120.0))
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut self.editor)
                                    .code_editor()
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(20),
                            );
                        });
                    for e in errors.iter().take(6) {
                        ui.label(RichText::new(format!("⚠ {e}")).color(ORANGE).small());
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(dirty, egui::Button::new("Save")).clicked() {
                            *act = Some(Action::Save);
                        }
                        if ui.add_enabled(dirty, egui::Button::new("Revert")).clicked() {
                            *act = Some(Action::Revert);
                        }
                        if dirty && up {
                            ui.label(RichText::new("Reconnect to apply changes").color(GRAY).small());
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if self.confirm_delete {
                                if ui.button("Cancel").clicked() {
                                    *act = Some(Action::CancelDelete);
                                }
                                if ui
                                    .button(RichText::new("Delete").color(RED).strong())
                                    .clicked()
                                {
                                    *act = Some(Action::Delete);
                                }
                                ui.label(format!("Delete {}?", t.name));
                            } else if ui
                                .add_enabled(!up, egui::Button::new(RichText::new("Delete…").color(RED)))
                                .on_disabled_hover_text("Disconnect first")
                                .clicked()
                            {
                                *act = Some(Action::AskDelete);
                            }
                        });
                    });
                }
            }
        });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain();
        // With a menu-bar icon, closing the window keeps Wirest running, like
        // Tailscale; quit from the menu bar or with Cmd+Q.
        if self.tray.is_some() && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        let mut act: Option<Action> = None;

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect()
        });
        if !dropped.is_empty() {
            act = Some(Action::Import(dropped));
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if self.advanced {
                    ui.add(
                        egui::Image::new(egui::load::SizedTexture::from_handle(&self.logo))
                            .fit_to_exact_size(egui::vec2(30.0, 30.0)),
                    );
                }
                ui.heading("Wirest");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.toggle_value(&mut self.advanced, "Advanced");
                    if self.advanced {
                        ui.toggle_value(&mut self.show_log, "Log");
                        if matches!(self.helper, HelperState::Ready | HelperState::Stale)
                            && ui
                                .small_button("Remove helper…")
                                .on_hover_text("Uninstall the background helper. Connecting will ask to install it again.")
                                .clicked()
                        {
                            act = Some(Action::UninstallHelper);
                        }
                    }
                });
            });
            if self.tools.is_none() {
                ui.add_space(4.0);
                egui::Frame::new()
                    .fill(ORANGE.gamma_multiply(0.15))
                    .inner_margin(8.0)
                    .corner_radius(6.0)
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("WireGuard tools not found.").strong());
                            ui.label("Build them with");
                            ui.code("./run.sh");
                            ui.label("(no Homebrew needed).");
                        });
                    });
            }
            ui.add_space(6.0);
        });

        if self.advanced {
            self.advanced_view(ctx, &mut act);
        } else {
            egui::CentralPanel::default().show(ctx, |ui| self.simple_view(ui, &mut act));
        }

        if let Some(d) = self.new_dialog.as_mut() {
            egui::Window::new("New Tunnel")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("A fresh key pair will be generated.");
                    ui.add_space(4.0);
                    let resp = ui.add(egui::TextEdit::singleline(&mut d.name).hint_text("name, e.g. wg0"));
                    if let Some(e) = &d.error {
                        ui.label(RichText::new(e).color(RED).small());
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.button("Create").clicked() || enter {
                            act = Some(Action::CreateNew);
                        }
                        if ui.button("Cancel").clicked() {
                            act = Some(Action::CancelNew);
                        }
                    });
                });
        }

        if let Some(d) = self.paste.as_mut() {
            egui::Window::new("Paste VPN config")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("Paste the WireGuard configuration from your VPN provider.");
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(160.0));
                    });
                    ui.add(
                        egui::TextEdit::multiline(&mut d.text)
                            .code_editor()
                            .desired_rows(12)
                            .desired_width(460.0)
                            .hint_text("[Interface]\nPrivateKey = …\n\n[Peer]\nPublicKey = …"),
                    );
                    if let Some(e) = &d.error {
                        ui.label(RichText::new(e).color(RED).small());
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Add").clicked() {
                            act = Some(Action::SavePaste);
                        }
                        if ui.button("Cancel").clicked() {
                            act = Some(Action::CancelPaste);
                        }
                    });
                });
        }

        if let Some(a) = act {
            self.apply(a, ctx);
        }
        self.sync_tray();
        // Keep "latest handshake" ages ticking.
        if !self.active.is_empty() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }
}

fn overview(ui: &mut egui::Ui, t: &Tunnel, up: bool, stats: Option<&IfaceStats>) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for e in &t.parsed.errors {
            ui.label(RichText::new(format!("⚠ {e}")).color(ORANGE));
        }
        if !t.parsed.errors.is_empty() {
            ui.add_space(6.0);
        }

        ui.strong("Interface");
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Grid::new("iface").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                row(ui, "Status", if up { "Connected" } else { "Not connected" });
                if let Some(s) = stats {
                    row(ui, "Device", mono(&s.iface));
                }
                if let Some(pk) = &t.public_key {
                    key_row(ui, "Public key", pk);
                }
                let i = &t.parsed.interface;
                if !i.addresses.is_empty() {
                    row(ui, "Addresses", i.addresses.join(", "));
                }
                if !i.dns.is_empty() {
                    row(ui, "DNS", i.dns.join(", "));
                }
                if let Some(p) = &i.listen_port {
                    row(ui, "Listen port", p.as_str());
                }
                if let Some(m) = &i.mtu {
                    row(ui, "MTU", m.as_str());
                }
                row(ui, "File", mono(&display_path(&t.path)).small());
            });
        });

        for (n, peer) in t.parsed.peers.iter().enumerate() {
            ui.add_space(10.0);
            ui.strong(format!("Peer {}", n + 1));
            let live = stats.and_then(|s| {
                s.peers.iter().find(|p| Some(&p.public_key) == peer.public_key.as_ref())
            });
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::Grid::new(("peer", n)).num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                    if let Some(pk) = peer.public_key.as_ref().filter(|k| !k.is_empty()) {
                        key_row(ui, "Public key", pk);
                    }
                    if peer.has_psk {
                        row(ui, "Preshared key", "enabled");
                    }
                    let endpoint = live
                        .map(|l| l.endpoint.as_str())
                        .filter(|e| *e != "(none)")
                        .or(peer.endpoint.as_deref());
                    if let Some(e) = endpoint {
                        row(ui, "Endpoint", mono(e));
                    }
                    if !peer.allowed_ips.is_empty() {
                        row(ui, "Allowed IPs", peer.allowed_ips.join(", "));
                    }
                    if let Some(k) = &peer.keepalive {
                        row(ui, "Keepalive", format!("every {k}s"));
                    }
                    if let Some(l) = live {
                        row(ui, "Latest handshake", ago(l.latest_handshake));
                        row(ui, "Transfer", format!("{} received · {} sent", human_bytes(l.rx), human_bytes(l.tx)));
                    }
                });
            });
        }
    });
}

fn display_path(p: &Path) -> String {
    match dirs::home_dir().and_then(|h| p.strip_prefix(h).ok().map(|r| r.to_path_buf())) {
        Some(rel) => format!("~/{}", rel.display()),
        None => p.display().to_string(),
    }
}
