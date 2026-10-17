use crate::config::{self, Parsed};
use crate::wg::{self, IfaceStats, Tools};
use eframe::egui::{self, Color32, RichText};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const GREEN: Color32 = Color32::from_rgb(52, 199, 89);
const GRAY: Color32 = Color32::from_rgb(142, 142, 147);
const ORANGE: Color32 = Color32::from_rgb(255, 159, 10);
const RED: Color32 = Color32::from_rgb(255, 69, 58);
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const LOG_LIMIT: usize = 500;

enum Msg {
    Log(String),
    Done { name: String, up: bool, ok: bool },
    Sudo(bool),
    Stats(Result<Vec<IfaceStats>, String>),
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
    Import(Vec<PathBuf>),
    Save,
    Revert,
    AskDelete,
    CancelDelete,
    Delete,
    InstallSudo,
    RevokeSudo,
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

pub struct App {
    dir: PathBuf,
    tunnels: Vec<Tunnel>,
    selected: Option<String>,
    tab: Tab,
    editor: String,
    editor_name: String,
    notice: Option<String>,
    confirm_delete: bool,
    new_dialog: Option<NewDialog>,

    tools: Option<Tools>,
    sudo_ok: bool,
    stats: Vec<IfaceStats>,
    stats_ok: bool,
    /// Tunnels we brought up ourselves; used when live stats aren't available.
    known_up: HashSet<String>,
    busy: HashSet<String>,

    log: Vec<String>,
    show_log: bool,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let dir = config::tunnels_dir();
        let (tx, rx) = channel();
        let tools = wg::find_tools();
        let mut app = Self {
            dir,
            tunnels: Vec::new(),
            selected: None,
            tab: Tab::Overview,
            editor: String::new(),
            editor_name: String::new(),
            notice: None,
            confirm_delete: false,
            new_dialog: None,
            tools: tools.clone(),
            sudo_ok: false,
            stats: Vec::new(),
            stats_ok: false,
            known_up: HashSet::new(),
            busy: HashSet::new(),
            log: Vec::new(),
            show_log: false,
            tx,
            rx,
        };
        if let Err(e) = config::ensure_dir(&app.dir) {
            app.push_log(format!("Cannot create {}: {e}", app.dir.display()));
        }
        match tools {
            Some(t) => {
                let source = if t.bundled { "bundled" } else { "Homebrew" };
                app.push_log(format!("Using {source} WireGuard tools from {}", t.dir.display()));
                spawn_poller(t, app.tx.clone(), cc.egui_ctx.clone());
            }
            None => app.push_log("WireGuard tools not found; build them with ./run.sh".into()),
        }
        app.reload();
        if let Some(first) = app.tunnels.first().map(|t| t.name.clone()) {
            app.select(first);
        }
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

    fn stats_for(&self, t: &Tunnel) -> Option<&IfaceStats> {
        let pk = t.public_key.as_ref()?;
        self.stats.iter().find(|s| &s.public_key == pk)
    }

    fn is_up(&self, t: &Tunnel) -> bool {
        if self.stats_ok {
            self.stats_for(t).is_some()
        } else {
            self.known_up.contains(&t.name) || wg::runtime_marker_exists(&t.name)
        }
    }

    fn drain(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Log(l) => self.push_log(l),
                Msg::Done { name, up, ok } => {
                    self.busy.remove(&name);
                    if ok {
                        if up {
                            self.known_up.insert(name.clone());
                        } else {
                            self.known_up.remove(&name);
                        }
                    }
                    let verb = if up { "activate" } else { "deactivate" };
                    if ok {
                        self.push_log(format!("[{name}] {verb}d"));
                    } else {
                        self.push_log(format!("[{name}] failed to {verb}"));
                        self.show_log = true;
                    }
                }
                Msg::Sudo(ok) => self.sudo_ok = ok,
                Msg::Stats(Ok(s)) => {
                    self.stats = s;
                    self.stats_ok = true;
                }
                Msg::Stats(Err(_)) => self.stats_ok = false,
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
            Action::Import(paths) => self.import(paths),
            Action::Save => self.save(),
            Action::Revert => {
                if let Some(n) = self.selected.clone() {
                    self.select(n);
                }
            }
            Action::AskDelete => self.confirm_delete = true,
            Action::CancelDelete => self.confirm_delete = false,
            Action::Delete => self.delete(),
            Action::InstallSudo | Action::RevokeSudo => {
                let install = matches!(action, Action::InstallSudo);
                let Some(tools) = self.tools.clone() else { return };
                let tx = self.tx.clone();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let r = if install { wg::install_sudoers(&tools) } else { wg::remove_sudoers() };
                    let what = if install { "Passwordless control enabled" } else { "Passwordless control removed" };
                    let line = match r {
                        Ok(_) => what.to_string(),
                        Err(e) => format!("Sudoers update failed: {}", e.trim()),
                    };
                    tx.send(Msg::Log(line)).ok();
                    tx.send(Msg::Sudo(wg::sudo_allowed(&tools))).ok();
                    ctx.request_repaint();
                });
            }
        }
    }

    fn toggle(&mut self, name: &str, ctx: &egui::Context) {
        let Some(tools) = self.tools.clone() else {
            self.push_log("WireGuard tools not found; build them with ./run.sh".into());
            self.show_log = true;
            return;
        };
        let Some(t) = self.find(name) else { return };
        if !t.parsed.errors.is_empty() && !self.is_up(t) {
            self.notice = Some("Fix the configuration errors before activating.".into());
            return;
        }
        let up = !self.is_up(t);
        let path = t.path.clone();
        let sudo = self.sudo_ok;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let name = name.to_owned();
        self.busy.insert(name.clone());
        self.push_log(format!("[{name}] {}…", if up { "activating" } else { "deactivating" }));
        std::thread::spawn(move || {
            let (ok, out) = match wg::quick(&tools, sudo, if up { "up" } else { "down" }, &path) {
                Ok(s) => (true, s),
                Err(s) => (false, s),
            };
            for l in out.lines().filter(|l| !l.trim().is_empty()) {
                tx.send(Msg::Log(format!("[{name}] {l}"))).ok();
            }
            tx.send(Msg::Done { name, up, ok }).ok();
            ctx.request_repaint();
        });
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

    fn create_new(&mut self) {
        let Some(d) = self.new_dialog.as_mut() else { return };
        let name = d.name.trim().to_string();
        if !config::valid_name(&name) {
            d.error = Some("Use 1–15 characters: letters, digits, _ = + . -".into());
            return;
        }
        let path = self.dir.join(format!("{name}.conf"));
        if path.exists() {
            d.error = Some("A tunnel with this name already exists.".into());
            return;
        }
        let text = config::template(&config::generate_private_key());
        if let Err(e) = config::write_secure(&path, &text) {
            d.error = Some(e.to_string());
            return;
        }
        self.new_dialog = None;
        self.reload();
        self.select(name);
        self.tab = Tab::Edit;
    }

    fn import(&mut self, paths: Vec<PathBuf>) {
        let mut last = None;
        for p in paths {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("tunnel");
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
            self.notice = Some(format!("A tunnel named {new_name} already exists."));
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
        if let Some(first) = self.tunnels.first().map(|t| t.name.clone()) {
            self.select(first);
        }
        self.confirm_delete = false;
    }
}

fn spawn_poller(tools: Tools, tx: Sender<Msg>, ctx: egui::Context) {
    std::thread::spawn(move || loop {
        let ok = wg::sudo_allowed(&tools);
        if tx.send(Msg::Sudo(ok)).is_err() {
            return;
        }
        let stats = if ok { wg::show_dump(&tools) } else { Err("no sudo".into()) };
        tx.send(Msg::Stats(stats)).ok();
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

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain();
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
                ui.heading("Wirus");
                ui.label(RichText::new("WireGuard tunnels").color(GRAY));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.toggle_value(&mut self.show_log, "Log");
                    if self.tools.is_some() {
                        if self.sudo_ok {
                            if wg::sudoers_installed()
                                && ui
                                    .small_button("Revoke")
                                    .on_hover_text("Remove the passwordless sudo rule")
                                    .clicked()
                            {
                                act = Some(Action::RevokeSudo);
                            }
                            ui.label(RichText::new("Live stats").color(GREEN).small())
                                .on_hover_text("wg/wg-quick run via passwordless sudo");
                            paint_dot(ui, GREEN);
                        } else if ui
                            .button("Enable passwordless control…")
                            .on_hover_text(
                                "Adds a sudoers rule so tunnels toggle without a password prompt \
                                 and live transfer stats work.\nNote: wg-quick runs PostUp/PostDown \
                                 hooks from configs, so this effectively grants root to anyone \
                                 who can edit your configs.",
                            )
                            .clicked()
                        {
                            act = Some(Action::InstallSudo);
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
                            ui.label("(no Homebrew needed). You can still create and edit tunnels.");
                        });
                    });
            }
            ui.add_space(6.0);
        });

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
                        act = Some(Action::OpenNew);
                    }
                    if ui.button("Import…").clicked() {
                        if let Some(files) = rfd::FileDialog::new()
                            .add_filter("WireGuard config", &["conf"])
                            .pick_files()
                        {
                            act = Some(Action::Import(files));
                        }
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
                            status_dot(ui, self.is_up(t), self.busy.contains(&t.name));
                            if ui.selectable_label(selected, &t.name).clicked() {
                                act = Some(Action::Select(t.name.clone()));
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
            let up = self.is_up(t);
            let busy = self.busy.contains(&t.name);
            let stats = self.stats_for(t).cloned();

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.heading(&t.name);
                status_dot(ui, up, busy);
                let state = if busy { "Working…" } else if up { "Active" } else { "Inactive" };
                ui.label(RichText::new(state).color(GRAY));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = if up { "Deactivate" } else { "Activate" };
                    // A broken config can't come up, but an active tunnel can always go down.
                    let blocked = !up && !t.parsed.errors.is_empty();
                    let btn = egui::Button::new(RichText::new(label).strong())
                        .min_size(egui::vec2(110.0, 28.0));
                    let btn = if up || blocked { btn } else { btn.fill(GREEN.gamma_multiply(0.55)) };
                    let resp = ui.add_enabled(!busy && !blocked, btn);
                    if resp.clicked() {
                        act = Some(Action::Toggle(t.name.clone()));
                    }
                    if blocked {
                        resp.on_disabled_hover_text(t.parsed.errors.join("\n"));
                        let n = t.parsed.errors.len();
                        let issues = if n == 1 { "1 issue" } else { &format!("{n} issues") };
                        ui.label(RichText::new(format!("Fix {issues} to activate")).color(ORANGE).small());
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

            if let Some(n) = &self.notice {
                ui.label(RichText::new(n).color(RED));
            }

            match self.tab {
                Tab::Overview => overview(ui, t, up, stats.as_ref(), self.stats_ok),
                Tab::Edit => {
                    let dirty = t.text != self.editor || t.name != self.editor_name;
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        ui.add_enabled(
                            !up,
                            egui::TextEdit::singleline(&mut self.editor_name).desired_width(160.0),
                        )
                        .on_disabled_hover_text("Deactivate the tunnel to rename it");
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
                            act = Some(Action::Save);
                        }
                        if ui.add_enabled(dirty, egui::Button::new("Revert")).clicked() {
                            act = Some(Action::Revert);
                        }
                        if dirty && up {
                            ui.label(RichText::new("Reconnect to apply changes").color(GRAY).small());
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if self.confirm_delete {
                                if ui.button("Cancel").clicked() {
                                    act = Some(Action::CancelDelete);
                                }
                                if ui
                                    .button(RichText::new("Delete").color(RED).strong())
                                    .clicked()
                                {
                                    act = Some(Action::Delete);
                                }
                                ui.label(format!("Delete {}?", t.name));
                            } else if ui
                                .add_enabled(!up, egui::Button::new(RichText::new("Delete…").color(RED)))
                                .on_disabled_hover_text("Deactivate the tunnel first")
                                .clicked()
                            {
                                act = Some(Action::AskDelete);
                            }
                        });
                    });
                }
            }
        });

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

        if let Some(a) = act {
            self.apply(a, ctx);
        }
        // Keep "latest handshake" ages ticking.
        if self.stats_ok {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }
}

fn overview(ui: &mut egui::Ui, t: &Tunnel, up: bool, stats: Option<&IfaceStats>, stats_ok: bool) {
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
                row(ui, "Status", if up { "Active" } else { "Inactive" });
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

        if up && !stats_ok {
            ui.add_space(10.0);
            ui.label(
                RichText::new("Enable passwordless control to see live handshake and transfer stats.")
                    .color(GRAY)
                    .small(),
            );
        }
    });
}

fn display_path(p: &Path) -> String {
    match dirs::home_dir().and_then(|h| p.strip_prefix(h).ok().map(|r| r.to_path_buf())) {
        Some(rel) => format!("~/{}", rel.display()),
        None => p.display().to_string(),
    }
}
