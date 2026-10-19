//! The interactive terminal UI: a VPN list with one key to connect, like the
//! GUI's simple view, plus add/paste/edit/delete for running on servers.

use crate::over_ssh;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;
use wirest_core::install::Elevation;
use wirest_core::protocol::Active;
use wirest_core::store::{self, Tunnel};
use wirest_core::{config, format, Backend};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
const LOG_LIMIT: usize = 300;

enum Msg {
    Active(Vec<Active>),
    Done { name: String, connecting: bool, result: Result<(String, Vec<Active>), String> },
}

#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Info,
    Good,
    Warn,
    Bad,
}

enum Mode {
    Normal,
    /// Single-line prompt; `then` decides what the answer is for.
    Input { title: &'static str, value: String, then: Then },
    /// Collecting a pasted (or typed) config.
    Paste { text: String },
    Confirm { question: String, then: Then },
}

enum Then {
    Import,
    SavePaste { text: String },
    Delete { name: String },
    Connect { name: String },
}

struct App {
    backend: Backend,
    dir: PathBuf,
    tunnels: Vec<Tunnel>,
    list: ListState,
    active: Vec<Active>,
    busy: Option<(String, bool)>,
    message: Option<(String, Tone)>,
    log: Vec<String>,
    show_log: bool,
    mode: Mode,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    quit: bool,
}

pub fn run() -> Result<(), String> {
    let mut terminal = start_terminal();
    let result = App::new().run(&mut terminal);
    stop_terminal();
    result
}

fn start_terminal() -> DefaultTerminal {
    let t = ratatui::init();
    execute!(std::io::stdout(), EnableBracketedPaste).ok();
    t
}

fn stop_terminal() {
    execute!(std::io::stdout(), DisableBracketedPaste).ok();
    ratatui::restore();
}

/// Leaves the UI for something that needs the real terminal (sudo's password
/// prompt, a text editor), then comes back.
fn suspended<T>(terminal: &mut DefaultTerminal, f: impl FnOnce() -> T) -> T {
    stop_terminal();
    let out = f();
    *terminal = start_terminal();
    out
}

impl App {
    fn new() -> Self {
        let (tx, rx) = channel();
        let backend = Backend::detect(Elevation::Terminal);
        spawn_poller(backend.clone(), tx.clone());
        let mut app = Self {
            backend,
            dir: config::tunnels_dir(),
            tunnels: Vec::new(),
            list: ListState::default(),
            active: Vec::new(),
            busy: None,
            message: None,
            log: Vec::new(),
            show_log: false,
            mode: Mode::Normal,
            tx,
            rx,
            quit: false,
        };
        app.reload(None);
        if app.backend.tools().is_none() {
            app.say("WireGuard tools not found next to wirest-tui; connecting won't work.", Tone::Bad);
        }
        app
    }

    fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<(), String> {
        while !self.quit {
            terminal.draw(|f| self.draw(f)).map_err(|e| e.to_string())?;
            if event::poll(Duration::from_millis(250)).map_err(|e| e.to_string())? {
                match event::read().map_err(|e| e.to_string())? {
                    Event::Key(k) if k.kind == KeyEventKind::Press => self.key(k, terminal),
                    Event::Paste(s) => {
                        if let Mode::Paste { text } = &mut self.mode {
                            text.push_str(&s.replace("\r\n", "\n").replace('\r', "\n"));
                        } else if let Mode::Input { value, .. } = &mut self.mode {
                            value.push_str(s.trim());
                        }
                    }
                    _ => {}
                }
            }
            while let Ok(msg) = self.rx.try_recv() {
                self.handle(msg);
            }
        }
        Ok(())
    }

    fn say(&mut self, text: impl Into<String>, tone: Tone) {
        self.message = Some((text.into(), tone));
    }

    fn push_log(&mut self, line: String) {
        self.log.push(line);
        if self.log.len() > LOG_LIMIT {
            self.log.drain(..self.log.len() - LOG_LIMIT);
        }
    }

    /// Reloads configs from disk, keeping (or moving to) a selection.
    fn reload(&mut self, select: Option<&str>) {
        let keep = select.map(str::to_owned).or_else(|| self.selected().map(|t| t.name.clone()));
        self.tunnels = store::load_all(&self.dir);
        let idx = keep
            .and_then(|n| self.tunnels.iter().position(|t| t.name == n))
            .or_else(|| self.active.first().and_then(|a| self.tunnels.iter().position(|t| t.name == a.name)))
            .or_else(|| self.tunnels.iter().position(Tunnel::ready))
            .or((!self.tunnels.is_empty()).then_some(0));
        self.list.select(idx);
    }

    fn selected(&self) -> Option<&Tunnel> {
        self.list.selected().and_then(|i| self.tunnels.get(i))
    }

    fn active_for(&self, name: &str) -> Option<&Active> {
        self.active.iter().find(|a| a.name == name)
    }

    /// The tunnel Wirest has up, if any.
    fn connected(&self) -> Option<&Active> {
        self.active.iter().find(|a| self.tunnels.iter().any(|t| t.name == a.name))
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Active(a) => self.active = a,
            Msg::Done { name, connecting, result } => {
                self.busy = None;
                match result {
                    Ok((output, active)) => {
                        for l in output.lines().filter(|l| !l.trim().is_empty()) {
                            self.push_log(format!("[{name}] {l}"));
                        }
                        self.active = active;
                        let what = if connecting { "Connected to" } else { "Disconnected" };
                        self.say(format!("{what} {name}"), Tone::Good);
                    }
                    Err(e) => {
                        let verb = if connecting { "connect" } else { "disconnect" };
                        let e = e.trim().to_owned();
                        for l in e.lines() {
                            self.push_log(format!("[{name}] {l}"));
                        }
                        let first = e.lines().last().unwrap_or("").to_owned();
                        self.say(format!("Couldn't {verb} {name}: {first} (press l for the log)"), Tone::Bad);
                    }
                }
            }
        }
    }

    // ---- actions ----

    fn toggle(&mut self, terminal: &mut DefaultTerminal) {
        if self.busy.is_some() {
            return;
        }
        let Some(t) = self.selected() else { return };
        let name = t.name.clone();
        if self.active_for(&name).is_some() {
            self.start_job(name, None, terminal);
        } else if let Some(e) = t.parsed.errors.first() {
            let msg = format!("Press e to finish {name}: {e}");
            self.say(msg, Tone::Warn);
        } else if over_ssh() && config::routes_everything(&t.parsed) {
            self.mode = Mode::Confirm {
                question: format!(
                    "{name} sends ALL traffic through the VPN.\nThis will probably cut off your SSH session.\n\nConnect anyway?"
                ),
                then: Then::Connect { name },
            };
        } else {
            let text = t.text.clone();
            self.start_job(name, Some(text), terminal);
        }
    }

    /// Connects (`config` = Some) or disconnects in the background. Installing
    /// the helper first needs the terminal for sudo, so that part runs here.
    fn start_job(&mut self, name: String, config: Option<String>, terminal: &mut DefaultTerminal) {
        if self.backend.needs_helper_install() {
            let backend = self.backend.clone();
            let result = suspended(terminal, || {
                println!("\nWirest needs to install its background helper (one time only).");
                println!("After this, connecting won't ask for a password again.\n");
                backend.ensure_helper()
            });
            if let Err(e) = result {
                self.push_log(format!("Helper install: {e}"));
                self.say(format!("Helper not installed: {}", e.trim()), Tone::Bad);
                return;
            }
            self.push_log("Helper installed".into());
        }
        let connecting = config.is_some();
        self.busy = Some((name.clone(), connecting));
        self.message = None;
        let (backend, tx) = (self.backend.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let result = match &config {
                Some(c) => backend.up(&name, c),
                None => backend.down(&name),
            };
            tx.send(Msg::Done { name, connecting, result }).ok();
        });
    }

    fn edit(&mut self, path: &Path, terminal: &mut DefaultTerminal) {
        let editor = std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .unwrap_or_else(|_| if which("nano") { "nano".into() } else { "vi".into() });
        let status = suspended(terminal, || {
            // Through sh so EDITOR may carry arguments, e.g. "code -w".
            Command::new("sh").arg("-c").arg(format!("{editor} \"$1\"")).arg("sh").arg(path).status()
        });
        if let Err(e) = status {
            self.say(format!("Couldn't start {editor}: {e}"), Tone::Bad);
        }
        let name = path.file_stem().and_then(|s| s.to_str()).map(str::to_owned);
        self.reload(name.as_deref());
        match self.selected() {
            Some(t) if !t.ready() => {
                let msg = format!("Saved. Still incomplete: {}", t.parsed.errors[0]);
                self.say(msg, Tone::Warn);
            }
            Some(t) if self.active_for(&t.name).is_some() => {
                self.say("Saved. Reconnect to apply the changes.", Tone::Info)
            }
            _ => self.say("Saved.", Tone::Good),
        }
    }

    fn new_tunnel(&mut self, terminal: &mut DefaultTerminal) {
        let name = store::unique_name(&self.dir, "wg");
        let text = config::template(&config::generate_private_key());
        match store::add(&self.dir, &name, &text) {
            Ok(()) => {
                let path = store::path_for(&self.dir, &name);
                self.reload(Some(&name));
                self.edit(&path, terminal);
            }
            Err(e) => self.say(e, Tone::Bad),
        }
    }

    fn finish(&mut self, then: Then, answer: String, terminal: &mut DefaultTerminal) {
        match then {
            Then::Import => {
                let path = expand_tilde(answer.trim());
                match store::import(&self.dir, &path) {
                    Ok(name) => {
                        self.reload(Some(&name));
                        self.say(format!("Added {name}. Press Enter to connect."), Tone::Good);
                    }
                    Err(e) => self.say(e, Tone::Bad),
                }
            }
            Then::SavePaste { text } => {
                let name = answer.trim().to_owned();
                match store::add(&self.dir, &name, &text) {
                    Ok(()) => {
                        self.reload(Some(&name));
                        let ready = self.selected().is_some_and(Tunnel::ready);
                        if ready {
                            self.say(format!("Added {name}. Press Enter to connect."), Tone::Good);
                        } else {
                            self.say(format!("Added {name}, but it's incomplete. Press e to edit."), Tone::Warn);
                        }
                    }
                    Err(e) => {
                        self.say(e, Tone::Bad);
                        // Let them pick another name without losing the paste.
                        self.mode = Mode::Input { title: "Name for this VPN", value: name, then: Then::SavePaste { text } };
                    }
                }
            }
            Then::Delete { name } => {
                match std::fs::remove_file(store::path_for(&self.dir, &name)) {
                    Ok(()) => self.say(format!("Deleted {name}"), Tone::Info),
                    Err(e) => self.say(format!("Couldn't delete {name}: {e}"), Tone::Bad),
                }
                self.reload(None);
            }
            Then::Connect { name } => {
                if let Some(text) = self.tunnels.iter().find(|t| t.name == name).map(|t| t.text.clone()) {
                    self.start_job(name, Some(text), terminal);
                }
            }
        }
    }

    // ---- keys ----

    fn key(&mut self, k: KeyEvent, terminal: &mut DefaultTerminal) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && k.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match std::mem::replace(&mut self.mode, Mode::Normal) {
            Mode::Normal => self.key_normal(k, terminal),
            Mode::Input { title, mut value, then } => match k.code {
                KeyCode::Esc => {}
                KeyCode::Enter => self.finish(then, value, terminal),
                KeyCode::Backspace => {
                    value.pop();
                    self.mode = Mode::Input { title, value, then };
                }
                KeyCode::Char(c) if !ctrl => {
                    value.push(c);
                    self.mode = Mode::Input { title, value, then };
                }
                _ => self.mode = Mode::Input { title, value, then },
            },
            Mode::Paste { mut text } => match k.code {
                KeyCode::Esc => {}
                KeyCode::Char('s' | 'd') if ctrl => {
                    if text.trim().is_empty() {
                        self.mode = Mode::Paste { text };
                    } else {
                        let value = store::unique_name(&self.dir, "vpn");
                        self.mode = Mode::Input { title: "Name for this VPN", value, then: Then::SavePaste { text } };
                    }
                }
                KeyCode::Enter => {
                    text.push('\n');
                    self.mode = Mode::Paste { text };
                }
                KeyCode::Backspace => {
                    text.pop();
                    self.mode = Mode::Paste { text };
                }
                KeyCode::Char(c) if !ctrl => {
                    text.push(c);
                    self.mode = Mode::Paste { text };
                }
                _ => self.mode = Mode::Paste { text },
            },
            Mode::Confirm { question, then } => match k.code {
                KeyCode::Char('y' | 'Y') => self.finish(then, String::new(), terminal),
                KeyCode::Char('n' | 'N') | KeyCode::Esc | KeyCode::Enter => {}
                _ => self.mode = Mode::Confirm { question, then },
            },
        }
    }

    fn key_normal(&mut self, k: KeyEvent, terminal: &mut DefaultTerminal) {
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Up | KeyCode::Char('k') => self.list.select_previous(),
            KeyCode::Down | KeyCode::Char('j') => self.list.select_next(),
            KeyCode::Enter | KeyCode::Char(' ') => self.toggle(terminal),
            KeyCode::Char('a') => {
                self.mode = Mode::Input { title: "Path to a .conf file", value: String::new(), then: Then::Import }
            }
            KeyCode::Char('p') => self.mode = Mode::Paste { text: String::new() },
            KeyCode::Char('n') => self.new_tunnel(terminal),
            KeyCode::Char('e') => {
                if let Some(path) = self.selected().map(|t| t.path.clone()) {
                    self.edit(&path, terminal);
                }
            }
            KeyCode::Char('d') => {
                let Some(name) = self.selected().map(|t| t.name.clone()) else { return };
                if self.active_for(&name).is_some() {
                    self.say("Disconnect it before deleting.", Tone::Warn);
                } else {
                    self.mode = Mode::Confirm { question: format!("Delete {name}?"), then: Then::Delete { name } };
                }
            }
            KeyCode::Char('l') => self.show_log = !self.show_log,
            KeyCode::Char('r') => {
                self.reload(None);
                self.active = self.backend.active();
            }
            _ => {}
        }
    }

    // ---- drawing ----

    fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        let log_h = if self.show_log { (area.height / 3).max(6) } else { 0 };
        let [header, body, log, footer] = Layout::vertical([
            Constraint::Length(4),
            Constraint::Min(6),
            Constraint::Length(log_h),
            // Key hints wrap onto a second line on narrow terminals.
            Constraint::Length(if area.width < 100 { 3 } else { 2 }),
        ])
        .areas(area);
        self.draw_header(f, header);
        // Side by side when there's room, stacked on narrow terminals.
        let [list_area, detail_area] = if body.width >= 72 {
            Layout::horizontal([Constraint::Length(28), Constraint::Min(30)]).areas(body)
        } else {
            Layout::vertical([Constraint::Length((self.tunnels.len() as u16 + 2).clamp(3, 8)), Constraint::Min(4)])
                .areas(body)
        };
        self.draw_list(f, list_area);
        self.draw_details(f, detail_area);
        if self.show_log {
            let lines: Vec<Line> = self.log.iter().rev().take(log.height as usize).rev().map(|l| Line::raw(l.as_str())).collect();
            f.render_widget(Paragraph::new(lines).block(Block::bordered().title(" Log ")), log);
        }
        self.draw_footer(f, footer);
        self.draw_modal(f, area);
    }

    fn draw_header(&self, f: &mut Frame, area: Rect) {
        let connected = self.connected();
        let answered = connected.is_some_and(|a| format::last_handshake(&a.stats) > 0);
        let (headline, color) = match (&self.busy, connected) {
            (Some((n, true)), _) => (format!("Connecting to {n}…"), Color::Yellow),
            (Some((n, false)), _) => (format!("Disconnecting {n}…"), Color::Yellow),
            (None, Some(a)) if answered => (format!("Connected to {}", a.name), Color::Green),
            (None, Some(a)) => (format!("Waiting for server ({})…", a.name), Color::Yellow),
            (None, None) => ("Not connected".to_owned(), Color::Gray),
        };
        let detail = match connected {
            Some(a) if answered => format::traffic_summary(&a.stats),
            Some(_) => "No reply from the server yet. Check the internet connection, or ask whoever runs the VPN.".into(),
            None if self.tunnels.is_empty() => "Add a VPN to get started: press a (file) or p (paste).".into(),
            None => "Select a VPN and press Enter to connect.".into(),
        };
        let mode = match &self.backend {
            Backend::Direct(_) => "root",
            Backend::Helper { .. } if self.backend.needs_helper_install() => "helper: not installed",
            Backend::Helper { .. } => "helper",
        };
        let block = Block::bordered()
            .title(Line::from(" Wirest ").style(Style::new().add_modifier(Modifier::BOLD)))
            .title_top(Line::from(format!(" {mode} ")).right_aligned().style(Style::new().fg(Color::DarkGray)));
        let text = Text::from(vec![
            Line::from(vec![
                Span::styled("● ", Style::new().fg(color)),
                Span::styled(headline, Style::new().fg(color).add_modifier(Modifier::BOLD)),
            ]),
            Line::styled(detail, Style::new().fg(Color::DarkGray)),
        ]);
        f.render_widget(Paragraph::new(text).block(block), area);
    }

    fn draw_list(&mut self, f: &mut Frame, area: Rect) {
        let items: Vec<ListItem> = self
            .tunnels
            .iter()
            .map(|t| {
                let (dot, color) = if self.busy.as_ref().is_some_and(|(n, _)| n == &t.name) {
                    ("◐", Color::Yellow)
                } else if self.active_for(&t.name).is_some() {
                    ("●", Color::Green)
                } else if t.ready() {
                    ("○", Color::Gray)
                } else {
                    ("!", Color::Yellow)
                };
                let mut spans = vec![Span::styled(format!("{dot} "), Style::new().fg(color)), Span::raw(t.name.clone())];
                if !t.ready() {
                    spans.push(Span::styled("  incomplete", Style::new().fg(Color::DarkGray)));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();
        let block = Block::bordered().title(" VPNs ");
        if items.is_empty() {
            let hint = "No VPNs yet.\n\na  add a .conf file\np  paste a config\nn  new (opens your editor)";
            f.render_widget(Paragraph::new(hint).block(block).style(Style::new().fg(Color::Gray)), area);
            return;
        }
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol("▶ ");
        f.render_stateful_widget(list, area, &mut self.list);
    }

    fn draw_details(&self, f: &mut Frame, area: Rect) {
        let Some(t) = self.selected() else {
            f.render_widget(Block::bordered(), area);
            return;
        };
        let label = |s: &str| Span::styled(format!("{s:<13}"), Style::new().fg(Color::DarkGray));
        let mut lines = Vec::new();
        for e in &t.parsed.errors {
            lines.push(Line::styled(format!("! {e}"), Style::new().fg(Color::Yellow)));
        }
        if !t.parsed.errors.is_empty() {
            lines.push(Line::raw(""));
        }
        let live = self.active_for(&t.name);
        let state = if live.is_some() { "Connected" } else { "Not connected" };
        lines.push(Line::from(vec![label("Status"), Span::raw(state)]));
        if let Some(a) = live {
            lines.push(Line::from(vec![label("Interface"), Span::raw(a.stats.iface.clone())]));
        }
        let i = &t.parsed.interface;
        if let Some(pk) = i.private_key.as_deref().and_then(config::public_key) {
            lines.push(Line::from(vec![label("Your key"), Span::raw(pk)]));
        }
        if !i.addresses.is_empty() {
            lines.push(Line::from(vec![label("Address"), Span::raw(i.addresses.join(", "))]));
        }
        if !i.dns.is_empty() {
            lines.push(Line::from(vec![label("DNS"), Span::raw(i.dns.join(", "))]));
        }
        for (n, peer) in t.parsed.peers.iter().enumerate() {
            lines.push(Line::raw(""));
            lines.push(Line::styled(format!("Peer {}", n + 1), Style::new().add_modifier(Modifier::BOLD)));
            let stats = live.and_then(|a| a.stats.peers.iter().find(|p| Some(&p.public_key) == peer.public_key.as_ref()));
            if let Some(e) = peer.endpoint.as_deref() {
                lines.push(Line::from(vec![label("Endpoint"), Span::raw(e.to_owned())]));
            }
            if !peer.allowed_ips.is_empty() {
                lines.push(Line::from(vec![label("Routes"), Span::raw(peer.allowed_ips.join(", "))]));
            }
            if let Some(s) = stats {
                lines.push(Line::from(vec![label("Handshake"), Span::raw(format::ago(s.latest_handshake))]));
                lines.push(Line::from(vec![
                    label("Traffic"),
                    Span::raw(format!("{} in · {} out", format::human_bytes(s.rx), format::human_bytes(s.tx))),
                ]));
            }
        }
        let block = Block::bordered().title(format!(" {} ", t.name));
        f.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }), area);
    }

    fn draw_footer(&self, f: &mut Frame, area: Rect) {
        let connected = self.selected().is_some_and(|t| self.active_for(&t.name).is_some());
        let enter = if connected { "disconnect" } else { "connect" };
        let keys: Vec<(&str, &str)> = vec![
            ("enter", enter),
            ("a", "add file"),
            ("p", "paste"),
            ("n", "new"),
            ("e", "edit"),
            ("d", "delete"),
            ("l", "log"),
            ("q", "quit"),
        ];
        let mut spans = Vec::new();
        for (k, what) in keys {
            spans.push(Span::styled(format!(" {k} "), Style::new().fg(Color::Black).bg(Color::Gray)));
            spans.push(Span::raw(format!(" {what}  ")));
        }
        let message = match &self.message {
            Some((m, tone)) => {
                let color = match tone {
                    Tone::Info => Color::Reset,
                    Tone::Good => Color::Green,
                    Tone::Warn => Color::Yellow,
                    Tone::Bad => Color::Red,
                };
                Line::styled(m.clone(), Style::new().fg(color))
            }
            None if self.connected().is_some() => {
                Line::styled("Quitting leaves the VPN connected.", Style::new().fg(Color::DarkGray))
            }
            None => Line::raw(""),
        };
        let [keys, msg] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(area);
        f.render_widget(Paragraph::new(Line::from(spans)).wrap(Wrap { trim: true }), keys);
        f.render_widget(Paragraph::new(message), msg);
    }

    fn draw_modal(&self, f: &mut Frame, area: Rect) {
        let (title, body, hint, height): (&str, Text, &str, u16) = match &self.mode {
            Mode::Normal => return,
            Mode::Input { title, value, .. } => (title, Text::raw(format!("{value}▏")), "enter ok · esc cancel", 5),
            Mode::Paste { text } => {
                let lines = text.lines().count() as u16;
                let body = if text.is_empty() {
                    Text::styled(
                        "Paste the WireGuard config from your VPN provider here\n(or type it).",
                        Style::new().fg(Color::DarkGray),
                    )
                } else {
                    Text::raw(format!("{text}▏"))
                };
                ("Paste VPN config", body, "ctrl+s save · esc cancel", (lines + 4).clamp(8, area.height.saturating_sub(4)))
            }
            Mode::Confirm { question, .. } => {
                let lines = question.lines().count() as u16;
                ("Confirm", Text::raw(question.as_str()), "y yes · n no", lines + 4)
            }
        };
        let w = area.width.saturating_sub(4).min(76);
        let h = height.min(area.height);
        let rect = Rect { x: area.x + (area.width - w) / 2, y: area.y + (area.height - h) / 2, width: w, height: h };
        let block = Block::bordered()
            .title(format!(" {title} "))
            .title_bottom(Line::from(format!(" {hint} ")).right_aligned())
            .border_style(Style::new().fg(Color::Cyan));
        f.render_widget(Clear, rect);
        f.render_widget(Paragraph::new(body).block(block).wrap(Wrap { trim: false }), rect);
    }
}

fn spawn_poller(backend: Backend, tx: Sender<Msg>) {
    std::thread::spawn(move || loop {
        if tx.send(Msg::Active(backend.active())).is_err() {
            return;
        }
        std::thread::sleep(POLL_INTERVAL);
    });
}

fn expand_tilde(p: &str) -> PathBuf {
    match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => Path::new(&home).join(rest),
        _ => PathBuf::from(p),
    }
}

fn which(cmd: &str) -> bool {
    Command::new("sh").arg("-c").arg(format!("command -v {cmd} >/dev/null")).status().is_ok_and(|s| s.success())
}
