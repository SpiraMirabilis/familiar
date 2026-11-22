//! Application state and behaviour: tabs, overlays, keys, mouse, commands and side effects.
//! Drawing is in `ui.rs`; this module never touches the terminal directly.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use familiar_config::{Config, World};
use familiar_engine::{EditRequest, KeyChord, Output, Status};
use familiar_net::SessionEvent;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use tokio::sync::mpsc::UnboundedSender;

use crate::buffer::Buffer;
use crate::form::{FieldKind, WorldForm};
use crate::fuzzy;
use crate::input::Input;
use crate::session::{Effect, Panel, Session};

/// An event from a connection: (session id, connection generation, event).
pub type NetEvent = (u64, u64, SessionEvent);

/// Commands only the terminal client has.
pub const TUI_COMMANDS: &[(&str, &str)] = &[
    ("/connect [<world> [<character>]]", "connect to a saved world, or pick one"),
    ("/world", "save or edit this tab's world (address, character, login)"),
    ("/close", "close this tab"),
    ("/sidebar", "show or hide the sidebar (map, stats, spawn windows)"),
    ("/config", "edit the config file in $EDITOR"),
    ("/keys", "show keyboard shortcuts"),
    ("/quit", "disconnect everything and quit"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tab {
    Main(u64),
    Puppet(u64, String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    Connect,
    NewWorld,
    EditWorld,
    History,
    NextTab,
    PrevTab,
    CloseTab,
    Reconnect,
    Disconnect,
    Clear,
    ToggleSidebar,
    NextPanel,
    ToggleLog,
    ShowMap,
    EditInput,
    EditConfig,
    StopSpeech,
    Help,
    Quit,
}

const PALETTE: &[(Cmd, &str, &str)] = &[
    (Cmd::Connect, "Connect to a world…", "Ctrl+O"),
    (Cmd::NewWorld, "New world…", "Ctrl+O Ctrl+N"),
    (Cmd::EditWorld, "Save or edit this world…", "/world"),
    (Cmd::History, "Search command history…", "Ctrl+R"),
    (Cmd::NextTab, "Next tab", "Alt+→"),
    (Cmd::PrevTab, "Previous tab", "Alt+←"),
    (Cmd::CloseTab, "Close tab", "/close"),
    (Cmd::Reconnect, "Reconnect", "Enter on empty line"),
    (Cmd::Disconnect, "Disconnect", "/disconnect"),
    (Cmd::Clear, "Clear output", "/clear"),
    (Cmd::ToggleSidebar, "Show or hide the sidebar", "Ctrl+B"),
    (Cmd::NextPanel, "Next sidebar panel", "Alt+]"),
    (Cmd::ShowMap, "Show the map", "/map"),
    (Cmd::ToggleLog, "Start or stop logging", "/log"),
    (Cmd::EditInput, "Edit the input in $EDITOR", "Ctrl+G"),
    (Cmd::EditConfig, "Edit the config file", "/config"),
    (Cmd::StopSpeech, "Stop speaking", "/speech stop"),
    (Cmd::Help, "Keyboard shortcuts", "F1"),
    (Cmd::Quit, "Quit", "Ctrl+C Ctrl+C"),
];

pub enum Action {
    Connect { world: Box<World>, character: Option<String> },
    Run(Cmd),
    Insert(String),
}

pub struct Item {
    pub group: &'static str,
    pub label: String,
    pub detail: String,
    pub action: Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    Connect,
    Palette,
    History,
}

pub struct Picker {
    pub kind: PickerKind,
    pub title: &'static str,
    pub query: Input,
    pub items: Vec<Item>,
    /// Indexes into `items` that match, best first, with the matched label positions.
    pub matches: Vec<(usize, Vec<usize>)>,
    pub selected: usize,
}

impl Picker {
    fn new(kind: PickerKind, title: &'static str, items: Vec<Item>) -> Self {
        let mut p = Self { kind, title, query: Input::new(1), items, matches: Vec::new(), selected: 0 };
        p.refilter();
        p
    }

    pub fn refilter(&mut self) {
        let query = self.query.text.trim().to_string();
        let mut scored: Vec<(i64, usize, Vec<usize>)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                if item.group.is_empty() {
                    // Always offered (e.g. "Connect to host:port").
                    return Some((i64::MAX, i, Vec::new()));
                }
                fuzzy::score(&query, &item.label)
                    .or_else(|| fuzzy::score(&query, &item.detail).map(|(s, _)| (s - 40, Vec::new())))
                    .map(|(s, pos)| (s, i, pos))
            })
            .collect();
        if !query.is_empty() {
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }
        self.matches = scored.into_iter().map(|(_, i, pos)| (i, pos)).collect();
        self.selected = 0;
    }

}

pub enum Overlay {
    Picker(Box<Picker>),
    World(Box<WorldForm>),
    Help,
}

/// A suggestion under a `/` command being typed.
pub struct Suggestion {
    /// The input text if it's accepted.
    pub insert: String,
    pub label: String,
    pub detail: String,
}

/// Work that needs the terminal to itself (an external editor).
pub enum External {
    /// Open an editor on `request`, at `line` if given.
    Edit { session: u64, request: EditRequest, line: Option<u32> },
    EditInput,
    EditConfig,
}

/// The map's screen area, the map-unit rectangle it shows (x0, y0, x1, y1), and its area name.
pub type MapHit = (Rect, (f64, f64, f64, f64), String);

/// Where things were drawn, for the mouse.
#[derive(Default)]
pub struct Hits {
    pub tabs: Vec<(Rect, usize)>,
    pub panels: Vec<(Rect, usize)>,
    /// (row, first column, last column + 1, uri)
    pub links: Vec<(u16, u16, u16, String)>,
    pub output: Rect,
    pub sidebar: Rect,
    pub picker_items: Vec<(Rect, usize)>,
    /// The map's area and the map-unit rectangle it shows: (x0, y0, x1, y1).
    pub map: Option<MapHit>,
}

pub struct App {
    pub config: Config,
    config_path: PathBuf,
    /// The config file couldn't be read, so it mustn't be overwritten.
    config_error: Option<String>,
    pub sessions: Vec<Session>,
    next_id: u64,
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub overlay: Option<Overlay>,
    pub sidebar: bool,
    pub toasts: Vec<(String, Instant)>,
    speaker: familiar_speech::Speaker,
    pub focused: bool,
    pub quit_armed: Option<Instant>,
    pub quit: bool,
    net_tx: UnboundedSender<NetEvent>,
    pub hits: Hits,
    pub external: Option<External>,
    pub suggestion: usize,
    pub suggestions_hidden: bool,
    pub started: Instant,
    /// The output pane's size at the last draw.
    pub view: (u16, u16),
    /// The size sent to servers (NAWS).
    naws: (u16, u16),
    esc_at: Option<Instant>,
    /// A saved world Ctrl+D was pressed on once (in the picker).
    delete_armed: Option<String>,
    sounds: Vec<(String, Child)>,
    player: Option<Option<&'static str>>,
}

impl App {
    pub fn new(net_tx: UnboundedSender<NetEvent>) -> Self {
        let config_path = familiar_config::default_config_path();
        let (config, config_error) = match Config::load(&config_path) {
            Ok(c) => (c, None),
            Err(e) => (Config::default(), Some(e.to_string())),
        };
        let mut app = Self {
            config,
            config_path,
            config_error: config_error.clone(),
            sessions: Vec::new(),
            next_id: 1,
            tabs: Vec::new(),
            active: 0,
            overlay: None,
            sidebar: true,
            toasts: Vec::new(),
            speaker: familiar_speech::Speaker::new(familiar_config::APP_NAME),
            focused: true,
            quit_armed: None,
            quit: false,
            net_tx,
            hits: Hits::default(),
            external: None,
            suggestion: 0,
            suggestions_hidden: false,
            started: Instant::now(),
            view: (80, 24),
            naws: (0, 0),
            esc_at: None,
            delete_armed: None,
            sounds: Vec::new(),
            player: None,
        };
        if let Some(e) = config_error {
            app.toast(&format!("Couldn't read the config ({e}); changes won't be saved"));
        }
        app.open_connect_picker();
        app
    }

    pub fn toast(&mut self, text: &str) {
        self.toasts.push((text.to_string(), Instant::now()));
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }

    fn save_config(&mut self) {
        if self.config_error.is_some() {
            return;
        }
        if let Err(e) = self.config.save(&self.config_path) {
            self.toast(&format!("Couldn't save the config: {e}"));
        }
    }

    // ----- Tabs and sessions -----

    pub fn session_index(&self, id: u64) -> Option<usize> {
        self.sessions.iter().position(|s| s.id == id)
    }

    /// The active tab's session index and puppet name.
    pub fn current(&self) -> Option<(usize, Option<String>)> {
        match self.tabs.get(self.active)? {
            Tab::Main(id) => Some((self.session_index(*id)?, None)),
            Tab::Puppet(id, name) => Some((self.session_index(*id)?, Some(name.clone()))),
        }
    }

    pub fn current_input(&mut self) -> Option<&mut Input> {
        let (i, puppet) = self.current()?;
        let s = &mut self.sessions[i];
        Some(match puppet {
            Some(name) => &mut s.puppet_mut(&name).input,
            None => &mut s.input,
        })
    }

    pub fn current_buffer(&mut self) -> Option<&mut Buffer> {
        let (i, puppet) = self.current()?;
        let s = &mut self.sessions[i];
        Some(match puppet {
            Some(name) => &mut s.puppet_mut(&name).buffer,
            None => &mut s.output,
        })
    }

    pub fn tab_title(&self, tab: &Tab) -> String {
        match tab {
            Tab::Main(id) => self.session_index(*id).map(|i| self.sessions[i].title()).unwrap_or_default(),
            Tab::Puppet(_, name) => format!("↳ {name}"),
        }
    }

    pub fn switch_to(&mut self, tab: usize) {
        if tab < self.tabs.len() {
            self.active = tab;
            self.suggestions_hidden = false;
            self.mark_seen();
        }
    }

    /// Clears unread markers for what's on screen.
    pub fn mark_seen(&mut self) {
        let Some((i, puppet)) = self.current() else { return };
        let s = &mut self.sessions[i];
        match puppet {
            Some(name) => s.puppet_mut(&name).unread = 0,
            None => {
                s.unread = 0;
                s.important = false;
            }
        }
    }

    pub fn open_session(&mut self, world: World, character: Option<String>) {
        let character = character.and_then(|name| world.characters.iter().find(|c| c.name.eq_ignore_ascii_case(&name)).cloned()).or_else(|| world.characters.first().cloned());
        let id = self.next_id;
        self.next_id += 1;
        let (session, out) = Session::new(id, &self.config, world, character);
        self.sessions.push(session);
        self.tabs.push(Tab::Main(id));
        self.active = self.tabs.len() - 1;
        let idx = self.sessions.len() - 1;
        self.apply(idx, out);
        self.start_scripting(idx);
        if !self.sessions[idx].engine.map().is_empty() && self.view.0 >= 100 {
            self.sidebar = true;
        }
        self.connect(idx);
    }

    /// Opens a saved world by name (ignoring case), as a named character or the first.
    pub fn open_saved(&mut self, world: &str, character: Option<&str>) -> bool {
        let Some(w) = self.config.worlds.iter().find(|w| w.name.eq_ignore_ascii_case(world)).cloned() else {
            self.toast(&format!("No saved world called {world}"));
            return false;
        };
        self.open_session(w, character.map(str::to_string));
        true
    }

    fn start_scripting(&mut self, idx: usize) {
        let host = match familiar_script::JsHost::new() {
            Ok(h) => h,
            Err(e) => {
                self.sessions[idx].output.notice(&format!("Scripting is unavailable: {e}"));
                return;
            }
        };
        let mut out = self.sessions[idx].engine.set_script_host(Box::new(host));
        let home = std::env::var_os("HOME").map(PathBuf::from);
        for path in self.sessions[idx].engine.startup_scripts() {
            let shown = match home.as_ref().and_then(|h| path.strip_prefix(h).ok()) {
                Some(rest) => format!("~/{}", rest.display()),
                None => path.display().to_string(),
            };
            out.push(Output::Notice(format!("Running {shown}")));
            out.extend(self.sessions[idx].engine.load_script(&path));
        }
        self.apply(idx, out);
    }

    fn connect(&mut self, idx: usize) {
        let s = &mut self.sessions[idx];
        if s.engine.is_connected() {
            return;
        }
        if self.naws.0 > 0 {
            s.engine.set_window_size(self.naws.0, self.naws.1);
        }
        s.generation += 1;
        let (id, generation) = (s.id, s.generation);
        let (mut events, out) = s.engine.connect();
        let tx = self.net_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                if tx.send((id, generation, event)).is_err() {
                    break;
                }
            }
        });
        self.apply(idx, out);
    }

    pub fn close_tab(&mut self) {
        let Some(tab) = self.tabs.get(self.active).cloned() else { return };
        match tab {
            Tab::Main(id) => {
                if let Some(i) = self.session_index(id) {
                    let mut s = self.sessions.remove(i);
                    let _ = s.engine.save_map();
                    s.engine.disconnect();
                    if s.logging {
                        s.engine.toggle_log();
                    }
                }
                self.tabs.retain(|t| !matches!(t, Tab::Main(i) | Tab::Puppet(i, _) if *i == id));
            }
            Tab::Puppet(id, name) => {
                if let Some(i) = self.session_index(id) {
                    self.sessions[i].engine.set_puppet_open(&name, false);
                }
                self.tabs.remove(self.active);
            }
        }
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
        if self.tabs.is_empty() {
            self.open_connect_picker();
        }
        self.mark_seen();
    }

    pub fn shutdown(&mut self) {
        for s in &mut self.sessions {
            let _ = s.engine.save_map();
            s.engine.disconnect();
            if s.logging {
                s.engine.toggle_log();
            }
        }
        for (_, child) in &mut self.sounds {
            let _ = child.kill();
        }
    }

    // ----- Engine output -----

    pub fn on_net(&mut self, (id, generation, event): NetEvent) {
        let Some(idx) = self.session_index(id) else { return };
        if self.sessions[idx].generation != generation {
            return;
        }
        let out = self.sessions[idx].engine.handle_event(event);
        self.apply(idx, out);
    }

    pub fn apply(&mut self, idx: usize, outputs: Vec<Output>) {
        let id = self.sessions[idx].id;
        let visible = self.tabs.get(self.active) == Some(&Tab::Main(id)) && self.overlay_allows_view();
        let effects = self.sessions[idx].apply(outputs, visible);
        for effect in effects {
            self.effect(idx, effect);
        }
    }

    fn overlay_allows_view(&self) -> bool {
        !matches!(&self.overlay, Some(Overlay::Picker(p)) if p.kind == PickerKind::Connect)
    }

    fn effect(&mut self, idx: usize, effect: Effect) {
        let id = self.sessions[idx].id;
        match effect {
            Effect::Notify { title, body } => {
                if !self.focused {
                    spawn_quietly("notify-send", &["-a", familiar_config::APP_NAME, &title, &body]);
                }
            }
            Effect::PlaySound(path) => self.play_sound("", &path, 1.0),
            Effect::Media(cmd) => match cmd {
                familiar_engine::media::MediaCommand::Play { name, url, volume, .. } => {
                    if url.starts_with("http://") || url.starts_with("https://") {
                        // Download off the UI thread; play when it arrives (if still wanted).
                        let player = self.player();
                        tokio::task::spawn_blocking(move || {
                            if let (Ok(path), Some(player)) = (familiar_engine::media::fetch(&url), player) {
                                let _ = player_command(player, &path.to_string_lossy(), volume).spawn();
                            }
                        });
                    } else {
                        self.play_sound(&name, &url, volume);
                    }
                }
                familiar_engine::media::MediaCommand::Stop { name } => {
                    self.sounds.retain_mut(|(n, child)| {
                        let stop = name.as_ref().is_none_or(|wanted| wanted == n);
                        if stop {
                            let _ = child.kill();
                        }
                        !stop
                    });
                }
                familiar_engine::media::MediaCommand::Preload { .. } => {}
            },
            Effect::OpenUri(uri) => {
                if uri.starts_with("http://") || uri.starts_with("https://") || uri.starts_with("mailto:") {
                    spawn_quietly("xdg-open", &[&uri]);
                    self.toast(&format!("Opening {uri}"));
                }
            }
            Effect::Edit(request) => self.external = Some(External::Edit { session: id, request, line: None }),
            Effect::Reconnect => self.connect(idx),
            Effect::ReloadScripts => self.start_scripting(idx),
            Effect::Bell => {
                use std::io::Write;
                let _ = std::io::stdout().write_all(b"\x07");
            }
            Effect::Speak(text) => {
                self.speaker.say(&text);
                if let Some(e) = self.speaker.take_error() {
                    self.toast(&e);
                }
            }
            Effect::SpeechStop => self.speaker.stop(),
            Effect::Variables { scope, variables } => {
                if let Some(rules) = self.config.rules_mut(&scope) {
                    rules.variables = variables;
                }
                self.save_config();
            }
            Effect::Settings(settings) => {
                self.config.settings = settings;
                self.save_config();
            }
            Effect::OpenPuppet { name, focus } => {
                let tab = Tab::Puppet(id, name);
                let pos = match self.tabs.iter().position(|t| *t == tab) {
                    Some(p) => p,
                    None => {
                        // After the session's other tabs.
                        let after = self.tabs.iter().rposition(|t| matches!(t, Tab::Main(i) | Tab::Puppet(i, _) if *i == id)).map_or(self.tabs.len(), |p| p + 1);
                        self.tabs.insert(after, tab);
                        if self.active >= after && !self.tabs.is_empty() && self.active + 1 < self.tabs.len() {
                            self.active += 1;
                        }
                        after
                    }
                };
                if focus {
                    self.switch_to(pos);
                }
            }
            Effect::ShowPanel(panel) => {
                let explicit = panel == Panel::Map;
                let s = &mut self.sessions[idx];
                if let Some(p) = s.panels().iter().position(|p| *p == panel)
                    && (explicit || !self.sidebar || s.panels().len() == 1)
                {
                    s.panel = p;
                }
                if explicit || self.view.0 >= 90 {
                    self.sidebar = true;
                }
            }
        }
    }

    fn player(&mut self) -> Option<&'static str> {
        *self.player.get_or_insert_with(|| ["pw-play", "paplay", "aplay"].into_iter().find(|p| which(p)))
    }

    fn play_sound(&mut self, name: &str, path: &str, volume: f64) {
        let Some(player) = self.player() else {
            self.toast("No sound player found (install pipewire, pulseaudio-utils or alsa-utils)");
            return;
        };
        let path = path.strip_prefix("file://").unwrap_or(path);
        match player_command(player, path, volume).spawn() {
            Ok(child) => self.sounds.push((name.to_string(), child)),
            Err(e) => self.toast(&format!("Couldn't play {path}: {e}")),
        }
    }

    // ----- Time -----

    /// When the app next needs to wake up without input.
    pub fn next_wake(&mut self) -> Instant {
        let now = Instant::now();
        let mut wake = now + Duration::from_secs(1);
        for s in &mut self.sessions {
            if let Some(d) = s.engine.next_deadline() {
                wake = wake.min(d);
            }
        }
        let animating = self.sessions.iter().any(|s| s.status == Status::Connecting) || !self.toasts.is_empty() || self.quit_armed.is_some();
        if animating {
            wake = wake.min(now + Duration::from_millis(120));
        }
        wake
    }

    pub fn on_tick(&mut self) {
        let now = Instant::now();
        for idx in 0..self.sessions.len() {
            if self.sessions[idx].engine.next_deadline().is_some_and(|d| d <= now) {
                let out = self.sessions[idx].engine.tick(now);
                self.apply(idx, out);
            }
        }
        self.toasts.retain(|(_, at)| at.elapsed() < Duration::from_secs(4));
        if self.quit_armed.is_some_and(|at| at.elapsed() > Duration::from_secs(2)) {
            self.quit_armed = None;
        }
        self.sounds.retain_mut(|(_, child)| matches!(child.try_wait(), Ok(None)));
    }

    /// The output pane was drawn at this size. Tells servers (NAWS) when it changes.
    pub fn set_view(&mut self, width: u16, height: u16) {
        self.view = (width, height);
        if self.naws != (width, height) && width > 0 {
            self.naws = (width, height);
            for s in &mut self.sessions {
                s.engine.set_window_size(width, height);
            }
        }
    }

    // ----- Overlays -----

    pub fn open_connect_picker(&mut self) {
        let mut items = vec![Item { group: "Saved worlds", label: "＋ New world…".into(), detail: "Ctrl+N".into(), action: Action::Run(Cmd::NewWorld) }];
        for w in &self.config.worlds {
            if w.characters.is_empty() {
                items.push(Item { group: "Saved worlds", label: w.name.clone(), detail: format!("{}:{}", w.host, w.port), action: Action::Connect { world: Box::new(w.clone()), character: None } });
            }
            for c in &w.characters {
                items.push(Item {
                    group: "Saved worlds",
                    label: format!("{} @ {}", c.name, w.name),
                    detail: format!("{}:{}", w.host, w.port),
                    action: Action::Connect { world: Box::new(w.clone()), character: Some(c.name.clone()) },
                });
            }
        }
        for e in familiar_config::directory::entries() {
            if self.config.worlds.iter().any(|w| w.host.eq_ignore_ascii_case(&e.host) && w.port == e.port) {
                continue;
            }
            items.push(Item { group: "Directory", label: e.name.clone(), detail: format!("{}:{}", e.host, e.port), action: Action::Connect { world: Box::new(e.to_world()), character: None } });
        }
        self.overlay = Some(Overlay::Picker(Box::new(Picker::new(PickerKind::Connect, "Connect", items))));
    }

    fn open_palette(&mut self) {
        let mut items: Vec<Item> = PALETTE.iter().map(|(cmd, label, keys)| Item { group: "Commands", label: label.to_string(), detail: keys.to_string(), action: Action::Run(*cmd) }).collect();
        for (i, tab) in self.tabs.iter().enumerate() {
            items.push(Item { group: "Tabs", label: format!("Switch to {}", self.tab_title(tab)), detail: format!("Alt+{}", i + 1), action: Action::Insert(format!("\u{0}tab{i}")) });
        }
        for (usage, description) in familiar_engine::COMMANDS.iter().chain(TUI_COMMANDS) {
            items.push(Item { group: "Client commands", label: usage.to_string(), detail: description.to_string(), action: Action::Insert(insert_for(usage)) });
        }
        self.overlay = Some(Overlay::Picker(Box::new(Picker::new(PickerKind::Palette, "Commands", items))));
    }

    fn open_history(&mut self) {
        let Some(input) = self.current_input() else { return };
        let mut seen = Vec::new();
        let items = input
            .history()
            .iter()
            .rev()
            .filter(|h| {
                let new = !seen.contains(*h);
                if new {
                    seen.push((*h).clone());
                }
                new
            })
            .map(|h| Item { group: "History", label: h.clone(), detail: String::new(), action: Action::Insert(h.clone()) })
            .collect();
        self.overlay = Some(Overlay::Picker(Box::new(Picker::new(PickerKind::History, "History", items))));
    }

    fn picker_query_changed(&mut self) {
        let Some(Overlay::Picker(p)) = &mut self.overlay else { return };
        if p.kind == PickerKind::Connect {
            p.items.retain(|i| !i.group.is_empty());
            if let Some(world) = parse_address(p.query.text.trim()) {
                let label = format!("Connect to {}:{}", world.host, world.port);
                p.items.insert(0, Item { group: "", label, detail: if world.tls { "TLS".into() } else { String::new() }, action: Action::Connect { world: Box::new(world), character: None } });
            }
        }
        p.refilter();
    }

    fn run_picker(&mut self) {
        let Some(Overlay::Picker(picker)) = self.overlay.take() else { return };
        let Some(index) = picker.matches.get(picker.selected).map(|(i, _)| *i) else {
            self.overlay = Some(Overlay::Picker(picker));
            return;
        };
        let mut picker = picker;
        let item = picker.items.swap_remove(index);
        match item.action {
            Action::Connect { world, character } => self.open_session(*world, character),
            Action::Run(cmd) => self.run(cmd),
            Action::Insert(text) => {
                if let Some(i) = text.strip_prefix("\u{0}tab").and_then(|n| n.parse().ok()) {
                    self.switch_to(i);
                } else if let Some(input) = self.current_input() {
                    input.set(&text);
                }
            }
        }
    }

    pub fn run(&mut self, cmd: Cmd) {
        let current = self.current();
        match cmd {
            Cmd::Connect => self.open_connect_picker(),
            Cmd::NewWorld => self.overlay = Some(Overlay::World(Box::new(WorldForm::new(None, None, false)))),
            Cmd::EditWorld => match current {
                Some((i, _)) => {
                    let world = self.sessions[i].engine.world().clone();
                    let saved = self.config.worlds.iter().find(|w| w.name.eq_ignore_ascii_case(&world.name)).cloned();
                    let character = self.sessions[i].engine.character_name();
                    let form = match &saved {
                        Some(w) => WorldForm::new(Some(w), character.as_deref(), true),
                        // A quick connection: offer to save it.
                        None => WorldForm::new(Some(&world), character.as_deref(), false),
                    };
                    self.overlay = Some(Overlay::World(Box::new(form)));
                }
                None => self.run(Cmd::NewWorld),
            },
            Cmd::History => self.open_history(),
            Cmd::NextTab if !self.tabs.is_empty() => self.switch_to((self.active + 1) % self.tabs.len()),
            Cmd::PrevTab if !self.tabs.is_empty() => self.switch_to((self.active + self.tabs.len() - 1) % self.tabs.len()),
            Cmd::CloseTab => self.close_tab(),
            Cmd::Reconnect => {
                if let Some((i, _)) = current {
                    self.connect(i);
                }
            }
            Cmd::Disconnect => {
                if let Some((i, _)) = current {
                    self.sessions[i].engine.disconnect();
                }
            }
            Cmd::Clear => {
                if let Some(b) = self.current_buffer() {
                    b.clear();
                }
            }
            Cmd::ToggleSidebar => self.sidebar = !self.sidebar,
            Cmd::NextPanel => {
                if let Some((i, _)) = current {
                    let s = &mut self.sessions[i];
                    let n = s.panels().len().max(1);
                    s.panel = (s.panel + 1) % n;
                    self.sidebar = true;
                }
            }
            Cmd::ShowMap => {
                if let Some((i, _)) = current {
                    let out = self.sessions[i].engine.submit("/map", false);
                    self.apply(i, out);
                }
            }
            Cmd::ToggleLog => {
                if let Some((i, _)) = current {
                    let out = self.sessions[i].engine.toggle_log();
                    self.apply(i, out);
                }
            }
            Cmd::EditInput => self.external = Some(External::EditInput),
            Cmd::EditConfig => self.external = Some(External::EditConfig),
            Cmd::StopSpeech => self.speaker.stop(),
            Cmd::Help => self.overlay = Some(Overlay::Help),
            Cmd::Quit => self.quit = true,
            Cmd::NextTab | Cmd::PrevTab => {}
        }
    }

    // ----- Suggestions for / commands -----

    pub fn suggestions(&self) -> Vec<Suggestion> {
        let Some((i, puppet)) = self.current() else { return Vec::new() };
        if self.suggestions_hidden || puppet.is_some() || self.overlay.is_some() {
            return Vec::new();
        }
        let s = &self.sessions[i];
        let text = &s.input.text;
        if !text.starts_with('/') || text.starts_with("//") || text.contains('\n') || s.password {
            return Vec::new();
        }
        let (word, rest) = text.split_once(' ').map_or((text.as_str(), None), |(w, r)| (w, Some(r)));
        let lower = word.to_lowercase();
        let Some(rest) = rest else {
            let mut out: Vec<Suggestion> = familiar_engine::COMMANDS
                .iter()
                .chain(TUI_COMMANDS)
                .filter(|(usage, _)| usage.split(' ').next().is_some_and(|w| w.starts_with(&lower)))
                .map(|(usage, detail)| Suggestion { insert: insert_for(usage), label: usage.to_string(), detail: detail.to_string() })
                .collect();
            out.dedup_by(|a, b| a.insert == b.insert && a.label == b.label);
            return out;
        };
        let rest_lower = rest.to_lowercase();
        let arg = |label: String, insert: String, detail: String| Suggestion { insert, label, detail };
        match lower.as_str() {
            "/connect" => self
                .config
                .worlds
                .iter()
                .flat_map(|w| {
                    let world = std::iter::once(arg(w.name.clone(), format!("/connect {}", w.name), format!("{}:{}", w.host, w.port)));
                    let chars = w.characters.iter().map(move |c| arg(format!("{} {}", w.name, c.name), format!("/connect {} {}", w.name, c.name), "character".into()));
                    world.chain(chars)
                })
                .filter(|sug| sug.label.to_lowercase().starts_with(&rest_lower))
                .take(20)
                .collect(),
            "/puppet" => s
                .engine
                .puppets()
                .filter(|p| p.name.to_lowercase().starts_with(&rest_lower))
                .map(|p| arg(p.name.clone(), format!("/puppet {}", p.name), format!("prefix {}", p.send_prefix)))
                .collect(),
            "/map" => {
                let (sub, query) = rest.split_once(' ').unwrap_or((rest, ""));
                if matches!(sub, "go" | "walk" | "here") && !query.is_empty() {
                    let map = s.engine.map();
                    map.search(query).into_iter().take(12).filter_map(|id| map.room(id)).map(|r| arg(r.name.clone(), format!("/map {sub} {}", r.name), r.area.clone())).collect()
                } else if !rest.contains(' ') {
                    ["go", "here", "addroom", "rename", "delete", "import", "track", "clear"]
                        .iter()
                        .filter(|c| c.starts_with(sub))
                        .map(|c| arg(format!("/map {c}"), format!("/map {c} "), String::new()))
                        .collect()
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }

    // ----- Input -----

    pub fn on_event(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key),
            Event::Mouse(mouse) => self.on_mouse(mouse),
            Event::Paste(text) => {
                if let Some(Overlay::World(form)) = &mut self.overlay {
                    form.focused().input.insert(&text.replace(['\r', '\n'], " "));
                } else if let Some(Overlay::Picker(p)) = &mut self.overlay {
                    p.query.insert(&text.replace(['\r', '\n'], " "));
                    self.picker_query_changed();
                } else if let Some(input) = self.current_input() {
                    input.insert(&text.replace("\r\n", "\n").replace('\r', "\n"));
                }
            }
            Event::FocusGained => self.focused = true,
            Event::FocusLost => self.focused = false,
            _ => {}
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        if self.overlay.is_some() {
            return self.on_overlay_key(key);
        }
        // Keys that work everywhere.
        match key.code {
            KeyCode::Char('c') if ctrl => {
                match self.current_input() {
                    Some(input) if !input.is_empty() => {
                        input.take(false);
                    }
                    _ => self.arm_quit(),
                }
                return;
            }
            KeyCode::Char('d') if ctrl && self.current_input().is_none_or(|i| i.is_empty()) => return self.arm_quit(),
            KeyCode::Char('o') if ctrl => return self.open_connect_picker(),
            KeyCode::Char('k') if ctrl && self.current_input().is_none_or(|i| i.is_empty()) => return self.open_palette(),
            KeyCode::Char('p') if ctrl => return self.open_palette(),
            KeyCode::Char('r') if ctrl => return self.open_history(),
            KeyCode::Char('b') if ctrl => return self.run(Cmd::ToggleSidebar),
            KeyCode::Char('g') if ctrl => return self.run(Cmd::EditInput),
            KeyCode::F(1) => return self.run(Cmd::Help),
            KeyCode::Char(c @ '1'..='9') if alt => return self.switch_to(c as usize - '1' as usize),
            KeyCode::Right if alt => return self.run(Cmd::NextTab),
            KeyCode::Left if alt => return self.run(Cmd::PrevTab),
            KeyCode::PageDown if ctrl => return self.run(Cmd::NextTab),
            KeyCode::PageUp if ctrl => return self.run(Cmd::PrevTab),
            KeyCode::Char(']') if alt => return self.run(Cmd::NextPanel),
            KeyCode::Char('e') if alt => {
                // Reopen the last program sent from the editor, at its first error.
                if let Some((i, _)) = self.current()
                    && let Some((request, line)) = self.sessions[i].last_edit.clone()
                {
                    self.external = Some(External::Edit { session: self.sessions[i].id, request, line });
                } else {
                    self.toast("Nothing to reopen: no program has been edited here yet");
                }
                return;
            }
            KeyCode::Char('[') if alt => {
                if let Some((i, _)) = self.current() {
                    let s = &mut self.sessions[i];
                    let n = s.panels().len().max(1);
                    s.panel = (s.panel + n - 1) % n;
                }
                return;
            }
            _ => {}
        }
        let Some((idx, puppet)) = self.current() else { return };

        // Keyboard macros, for keys the client doesn't use itself.
        let reserved = matches!(key.code, KeyCode::Char(_)) && !ctrl && !alt
            || matches!(key.code, KeyCode::Enter | KeyCode::Backspace | KeyCode::Delete | KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End | KeyCode::Tab | KeyCode::BackTab | KeyCode::Esc)
            || (ctrl && matches!(key.code, KeyCode::Char('a' | 'e' | 'u' | 'w' | 'y' | 'j' | 'l')))
            || (matches!(key.code, KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown) && !key.state.contains(KeyEventState::KEYPAD));
        if (!reserved || key.state.contains(KeyEventState::KEYPAD))
            && puppet.is_none()
            && let Some(chord) = chord(&key)
            && let Some(out) = self.sessions[idx].engine.run_macro(&chord)
        {
            self.sessions[idx].output.scroll_to_bottom();
            self.apply(idx, out);
            return;
        }

        // The / command popup.
        let suggestions = self.suggestions();
        if !suggestions.is_empty() {
            let n = suggestions.len();
            match key.code {
                KeyCode::Up => return self.suggestion = (self.suggestion + n - 1) % n,
                KeyCode::Down => return self.suggestion = (self.suggestion + 1) % n,
                KeyCode::Tab => {
                    let pick = &suggestions[self.suggestion.min(n - 1)];
                    self.sessions[idx].input.set(&pick.insert);
                    self.suggestion = 0;
                    return;
                }
                KeyCode::Enter if !shift && !alt => {
                    let pick = &suggestions[self.suggestion.min(n - 1)];
                    let typed = self.sessions[idx].input.text.trim_end().to_string();
                    // Finish an incomplete command word rather than sending it.
                    if pick.insert.trim_end() != typed && pick.insert.to_lowercase().starts_with(&typed.to_lowercase()) && pick.insert.ends_with(' ') {
                        self.sessions[idx].input.set(&pick.insert);
                        self.suggestion = 0;
                        return;
                    }
                }
                KeyCode::Esc => {
                    self.suggestions_hidden = true;
                    return;
                }
                _ => {}
            }
        }

        // Scrolling.
        let (w, h) = self.view;
        let page = (h as isize - 2).max(1);
        match key.code {
            KeyCode::PageUp => return self.scroll(page, w, h),
            KeyCode::PageDown => return self.scroll(-page, w, h),
            KeyCode::Up if shift => return self.scroll(1, w, h),
            KeyCode::Down if shift => return self.scroll(-1, w, h),
            KeyCode::Home if ctrl => return self.scroll(isize::MAX / 4, w, h),
            KeyCode::End if ctrl => {
                if let Some(b) = self.current_buffer() {
                    b.scroll_to_bottom();
                }
                return;
            }
            KeyCode::Esc => {
                let scrolled = self.current_buffer().is_some_and(|b| b.scroll > 0);
                if scrolled {
                    if let Some(b) = self.current_buffer() {
                        b.scroll_to_bottom();
                    }
                    return;
                }
                // Esc twice clears the input.
                if self.esc_at.is_some_and(|t| t.elapsed() < Duration::from_millis(600)) {
                    if let Some(i) = self.current_input() {
                        i.take(false);
                    }
                    self.esc_at = None;
                } else {
                    self.esc_at = Some(Instant::now());
                }
                return;
            }
            _ => {}
        }

        if key.code == KeyCode::Tab {
            // Complete a word from recent output (names, mostly), cycling on repeated presses.
            let s = &mut self.sessions[idx];
            let (word, words) = match &puppet {
                Some(name) => {
                    let p = s.puppet_mut(name);
                    let word = p.input.current_word().1.to_string();
                    let words = p.buffer.words_starting(&word, 400);
                    (word, words)
                }
                None => {
                    let word = s.input.current_word().1.to_string();
                    (word.clone(), s.output.words_starting(&word, 400))
                }
            };
            let input = match &puppet {
                Some(name) => &mut s.puppet_mut(name).input,
                None => &mut s.input,
            };
            if !input.complete(|_| words) && word.is_empty() {
                input.insert("    ");
            }
            return;
        }
        let session = &mut self.sessions[idx];
        let input = match &puppet {
            Some(name) => &mut session.puppet_mut(name).input,
            None => &mut session.input,
        };
        self.suggestions_hidden = self.suggestions_hidden && !input.text.is_empty();
        let before = input.text.clone();
        match key.code {
            KeyCode::Enter if shift || alt => input.insert("\n"),
            KeyCode::Char('j') if ctrl => input.insert("\n"),
            KeyCode::Enter => return self.submit(),
            KeyCode::Backspace if ctrl || alt => input.kill_word(),
            KeyCode::Backspace => input.backspace(),
            KeyCode::Delete => input.delete(),
            KeyCode::Left if ctrl => input.word_left(),
            KeyCode::Right if ctrl => input.word_right(),
            KeyCode::Left => input.left(),
            KeyCode::Right => input.right(),
            KeyCode::Char('b') if alt => input.word_left(),
            KeyCode::Char('f') if alt => input.word_right(),
            KeyCode::Char('d') if alt => input.kill_word_forward(),
            KeyCode::Home => input.home(),
            KeyCode::End => input.end(),
            KeyCode::Char('a') if ctrl => input.home(),
            KeyCode::Char('e') if ctrl => input.end(),
            KeyCode::Char('u') if ctrl => input.kill_to_start(),
            KeyCode::Char('k') if ctrl => input.kill_to_end(),
            KeyCode::Char('w') if ctrl => input.kill_word(),
            KeyCode::Char('y') if ctrl => input.yank(),
            KeyCode::Char('l') if ctrl => {}
            KeyCode::Up => {
                if !input.line_up() {
                    input.history_older();
                }
            }
            KeyCode::Down => {
                if !input.line_down() {
                    input.history_newer();
                }
            }
            KeyCode::Char(c) if !ctrl => {
                let mut buf = [0; 4];
                input.insert(c.encode_utf8(&mut buf));
            }
            _ => {}
        }
        if input.text != before {
            self.suggestion = 0;
            if input.text.is_empty() {
                self.suggestions_hidden = false;
            }
        }
    }

    fn arm_quit(&mut self) {
        if self.quit_armed.is_some() {
            self.quit = true;
        } else {
            self.quit_armed = Some(Instant::now());
        }
    }

    fn scroll(&mut self, delta: isize, w: u16, h: u16) {
        if let Some(b) = self.current_buffer() {
            b.scroll_by(delta, w, h);
        }
    }

    fn on_overlay_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if matches!(self.overlay, Some(Overlay::Help)) {
            self.overlay = None;
            return;
        }
        if matches!(self.overlay, Some(Overlay::World(_))) {
            return self.on_form_key(key);
        }
        // In the world picker: Ctrl+N adds a world, Ctrl+E edits the selected one and Ctrl+D
        // (twice) deletes it.
        if let Some(Overlay::Picker(p)) = &self.overlay
            && p.kind == PickerKind::Connect
            && ctrl
        {
            let selected = p.matches.get(p.selected).map(|(i, _)| &p.items[*i]);
            let saved = selected.and_then(|item| match &item.action {
                Action::Connect { world, character } if item.group == "Saved worlds" => Some((world.name.clone(), character.clone())),
                _ => None,
            });
            match key.code {
                KeyCode::Char('n') => {
                    // Start from an address typed into the filter, if there is one.
                    let typed = parse_address(p.query.text.trim());
                    let mut form = WorldForm::new(None, None, false);
                    if let Some(w) = typed {
                        form.fields[crate::form::HOST].input.set(&w.host);
                        form.fields[crate::form::PORT].input.set(&w.port.to_string());
                        form.fields[crate::form::TLS].on = w.tls;
                    }
                    self.overlay = Some(Overlay::World(Box::new(form)));
                    return;
                }
                KeyCode::Char('e') => {
                    if let Some((name, character)) = saved
                        && let Some(w) = self.config.world(&name).cloned()
                    {
                        self.overlay = Some(Overlay::World(Box::new(WorldForm::new(Some(&w), character.as_deref(), true))));
                    }
                    return;
                }
                KeyCode::Char('d') => {
                    if let Some((name, _)) = saved {
                        if self.delete_armed.as_deref() == Some(name.as_str()) {
                            self.config.worlds.retain(|w| w.name != name);
                            self.save_config();
                            self.delete_armed = None;
                            self.toast(&format!("Deleted {name}"));
                            self.open_connect_picker();
                        } else {
                            self.toast(&format!("Press Ctrl+D again to delete {name}"));
                            self.delete_armed = Some(name);
                        }
                    }
                    return;
                }
                _ => {}
            }
        }
        let Some(Overlay::Picker(p)) = &mut self.overlay else { return };
        let n = p.matches.len().max(1);
        match key.code {
            KeyCode::Esc => {
                if self.tabs.is_empty() {
                    // Nothing behind the picker: Esc clears the filter instead.
                    p.query.take(false);
                    self.picker_query_changed();
                } else {
                    self.overlay = None;
                }
            }
            KeyCode::Char('c') if ctrl => {
                if self.tabs.is_empty() {
                    self.arm_quit();
                } else {
                    self.overlay = None;
                }
            }
            KeyCode::Enter => self.run_picker(),
            KeyCode::Up => p.selected = (p.selected + n - 1) % n,
            KeyCode::Char('p') if ctrl => p.selected = (p.selected + n - 1) % n,
            KeyCode::Down | KeyCode::Tab => p.selected = (p.selected + 1) % n,
            KeyCode::Char('n') if ctrl => p.selected = (p.selected + 1) % n,
            KeyCode::PageUp => p.selected = p.selected.saturating_sub(8),
            KeyCode::PageDown => p.selected = (p.selected + 8).min(n - 1),
            KeyCode::Backspace => {
                p.query.backspace();
                self.picker_query_changed();
            }
            KeyCode::Char('u') if ctrl => {
                p.query.take(false);
                self.picker_query_changed();
            }
            KeyCode::Char('w') if ctrl => {
                p.query.kill_word();
                self.picker_query_changed();
            }
            KeyCode::Left => p.query.left(),
            KeyCode::Right => p.query.right(),
            KeyCode::Char(c) if !ctrl => {
                let mut buf = [0; 4];
                p.query.insert(c.encode_utf8(&mut buf));
                self.picker_query_changed();
            }
            _ => {}
        }
    }

    fn on_form_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(Overlay::World(form)) = &mut self.overlay else { return };
        form.error = None;
        match key.code {
            KeyCode::Esc => {
                // Back to the picker if that's where the form came from (or nothing's open).
                self.overlay = None;
                if self.tabs.is_empty() {
                    self.open_connect_picker();
                }
            }
            KeyCode::Tab | KeyCode::Down => form.next(),
            KeyCode::BackTab | KeyCode::Up => form.prev(),
            KeyCode::Enter | KeyCode::Char('s') if key.code == KeyCode::Enter || ctrl => {
                let connect = key.code == KeyCode::Enter;
                let Some(Overlay::World(mut form)) = self.overlay.take() else { return };
                match form.save(&mut self.config) {
                    Ok((world, character)) => {
                        self.save_config();
                        self.toast(&format!("Saved {}", world.name));
                        // Open tabs on this world pick up the new character and login.
                        for idx in 0..self.sessions.len() {
                            let out = self.sessions[idx].engine.reload(&self.config);
                            self.apply(idx, out);
                        }
                        if connect && !self.sessions.iter().any(|s| s.engine.world().name == world.name && s.engine.is_connected()) {
                            self.open_session(world, character);
                        } else if self.tabs.is_empty() {
                            self.open_connect_picker();
                        }
                    }
                    Err(e) => {
                        form.error = Some(e);
                        self.overlay = Some(Overlay::World(form));
                    }
                }
            }
            KeyCode::Char(' ') if form.fields[form.focus].kind == FieldKind::Toggle => {
                let f = form.focused();
                f.on = !f.on;
            }
            _ if form.fields[form.focus].kind == FieldKind::Toggle => {}
            KeyCode::Backspace => form.focused().input.backspace(),
            KeyCode::Delete => form.focused().input.delete(),
            KeyCode::Left => form.focused().input.left(),
            KeyCode::Right => form.focused().input.right(),
            KeyCode::Home => form.focused().input.home(),
            KeyCode::End => form.focused().input.end(),
            KeyCode::Char('u') if ctrl => form.focused().input.kill_to_start(),
            KeyCode::Char('w') if ctrl => form.focused().input.kill_word(),
            KeyCode::Char(c) if !ctrl => {
                let mut buf = [0; 4];
                form.focused().input.insert(c.encode_utf8(&mut buf));
            }
            _ => {}
        }
    }

    fn submit(&mut self) {
        let Some((idx, puppet)) = self.current() else { return };
        self.sessions[idx].output.scroll_to_bottom();
        let secret = self.sessions[idx].password && puppet.is_none();
        let input = match &puppet {
            Some(name) => &mut self.sessions[idx].puppet_mut(name).input,
            None => &mut self.sessions[idx].input,
        };
        let remember = !secret && !input.text.trim().is_empty();
        let text = input.take(remember);
        self.suggestion = 0;
        self.suggestions_hidden = false;
        if let Some(name) = puppet {
            let out = self.sessions[idx].engine.puppet_submit(&name, &text);
            self.sessions[idx].puppet_mut(&name).buffer.scroll_to_bottom();
            return self.apply(idx, out);
        }
        if text.is_empty() && matches!(self.sessions[idx].status, Status::Disconnected(_)) {
            return self.connect(idx);
        }
        if !secret && self.client_command(&text) {
            return;
        }
        let out = self.sessions[idx].engine.submit(&text, secret);
        self.apply(idx, out);
    }

    /// Commands the terminal client handles itself. Returns true if `text` was one.
    fn client_command(&mut self, text: &str) -> bool {
        let Some(command) = text.strip_prefix('/') else { return false };
        let (name, args) = command.split_once(' ').unwrap_or((command, ""));
        let args = args.trim();
        match name.to_lowercase().as_str() {
            "connect" if args.is_empty() => self.open_connect_picker(),
            "connect" => self.connect_command(args),
            "close" => self.close_tab(),
            "quit" | "exit" => self.quit = true,
            "sidebar" => self.run(Cmd::ToggleSidebar),
            "world" | "worlds" => self.run(Cmd::EditWorld),
            "config" => self.run(Cmd::EditConfig),
            "keys" | "shortcuts" => self.run(Cmd::Help),
            _ => return false,
        }
        true
    }

    /// `/connect WORLD [CHARACTER]` (world names may have spaces) or `/connect host:port`.
    pub fn connect_command(&mut self, args: &str) {
        let args = args.trim();
        let lower = args.to_lowercase();
        let found = self
            .config
            .worlds
            .iter()
            .map(|w| w.name.clone())
            .filter(|n| lower == n.to_lowercase() || lower.starts_with(&format!("{} ", n.to_lowercase())))
            .max_by_key(String::len);
        match found {
            Some(world) => {
                let character = args[world.len()..].trim();
                self.open_saved(&world, (!character.is_empty()).then_some(character));
            }
            None => match parse_address(args) {
                Some(world) => self.open_session(world, None),
                None => self.toast(&format!("No saved world called {args}")),
            },
        }
    }

    /// Rereads the config file (after editing it) and gives sessions the new rules.
    pub fn reload_config(&mut self) {
        match Config::load(&self.config_path) {
            Ok(config) => {
                self.config = config;
                self.config_error = None;
                for idx in 0..self.sessions.len() {
                    let out = self.sessions[idx].engine.reload(&self.config);
                    self.apply(idx, out);
                }
                self.toast("Config reloaded");
            }
            Err(e) => self.toast(&format!("The config has an error, so it wasn't reloaded: {e}")),
        }
    }

    // ----- Mouse -----

    fn on_mouse(&mut self, mouse: MouseEvent) {
        let at = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let delta = if mouse.kind == MouseEventKind::ScrollUp { 3 } else { -3 };
                if let Some(Overlay::Picker(p)) = &mut self.overlay {
                    let n = p.matches.len().max(1);
                    p.selected = if delta > 0 { p.selected.saturating_sub(1) } else { (p.selected + 1).min(n - 1) };
                } else if self.hits.sidebar.contains(at) {
                    if let Some((i, _)) = self.current() {
                        let s = &mut self.sessions[i];
                        if let Some(Panel::Spawn(name)) = s.panels().get(s.panel).cloned() {
                            let (w, h) = (self.hits.sidebar.width.saturating_sub(2), self.hits.sidebar.height.saturating_sub(2));
                            s.spawn_mut(&name).0.buffer.scroll_by(delta, w, h);
                        }
                    }
                } else {
                    let (w, h) = self.view;
                    self.scroll(delta, w, h);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(Overlay::Picker(p)) = &mut self.overlay {
                    if let Some((_, i)) = self.hits.picker_items.iter().find(|(r, _)| r.contains(at)) {
                        p.selected = *i;
                        self.run_picker();
                    }
                    return;
                }
                if matches!(self.overlay, Some(Overlay::Help)) {
                    self.overlay = None;
                    return;
                }
                if let Some(&(_, t)) = self.hits.tabs.iter().find(|(r, _)| r.contains(at)) {
                    return self.switch_to(t);
                }
                if let Some(&(_, p)) = self.hits.panels.iter().find(|(r, _)| r.contains(at)) {
                    if let Some((i, _)) = self.current() {
                        self.sessions[i].panel = p;
                    }
                    return;
                }
                if let Some(uri) = self.hits.links.iter().find(|(y, x0, x1, _)| *y == at.y && (*x0..*x1).contains(&at.x)).map(|l| l.3.clone()) {
                    if let Some((i, _)) = self.current() {
                        let out = self.sessions[i].engine.follow_link(&uri);
                        self.apply(i, out);
                    }
                    return;
                }
                if let Some((rect, (x0, y0, x1, y1), area)) = self.hits.map.clone()
                    && rect.contains(at)
                    && let Some((i, _)) = self.current()
                {
                    // Walk to the clicked room.
                    let fx = (at.x - rect.x) as f64 + 0.5;
                    let fy = (at.y - rect.y) as f64 + 0.5;
                    let x = x0 + fx / rect.width as f64 * (x1 - x0);
                    let y = y0 + fy / rect.height as f64 * (y1 - y0);
                    let room = self.sessions[i].engine.map().room_at(&area, x, y);
                    if let Some(room) = room {
                        let out = self.sessions[i].engine.map_walk_to(room);
                        self.apply(i, out);
                    }
                }
            }
            _ => {}
        }
    }
}

/// The input text for picking a command from its usage: everything up to the first
/// placeholder or choice, plus a space if arguments follow.
pub fn insert_for(usage: &str) -> String {
    let mut literal = Vec::new();
    let mut more = false;
    for (i, token) in usage.split(' ').enumerate() {
        if i > 0 && (token.contains(['<', '[', '|']) || token == "...") {
            more = true;
            break;
        }
        let cut = token.find('<').map_or(token, |p| &token[..p]);
        literal.push(cut);
        if cut.len() < token.len() {
            break;
        }
    }
    let mut text = literal.join(" ");
    if more {
        text.push(' ');
    }
    text
}

/// "host:port", "host port" or "host:port tls" as a new world.
fn parse_address(text: &str) -> Option<World> {
    let mut words = text.split_whitespace();
    let first = words.next()?;
    let (host, port) = match first.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok()?),
        None => (first, words.next()?.parse::<u16>().ok()?),
    };
    if host.is_empty() || !host.contains(['.']) && host != "localhost" {
        return None;
    }
    let tls = words.next().is_some_and(|w| w.eq_ignore_ascii_case("tls") || w.eq_ignore_ascii_case("ssl"));
    Some(World { name: format!("{host}:{port}"), host: host.to_string(), port, tls, ..World::default() })
}

/// A key as a macro chord, in the GTK client's naming (GDK key names).
fn chord(key: &KeyEvent) -> Option<KeyChord> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let mut shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let keypad = key.state.contains(KeyEventState::KEYPAD);
    let name = match key.code {
        KeyCode::F(n) => format!("F{n}"),
        KeyCode::Char(c) if keypad => match c {
            '0'..='9' => format!("KP_{c}"),
            '+' => "KP_Add".into(),
            '-' => "KP_Subtract".into(),
            '*' => "KP_Multiply".into(),
            '/' => "KP_Divide".into(),
            '.' => "KP_Decimal".into(),
            _ => return None,
        },
        KeyCode::Char(c) => {
            // Shifted characters already say so.
            shift &= ctrl || alt;
            c.to_uppercase().to_string()
        }
        KeyCode::Enter if keypad => "KP_Enter".into(),
        KeyCode::Up if keypad => "KP_8".into(),
        KeyCode::Down if keypad => "KP_2".into(),
        KeyCode::Left if keypad => "KP_4".into(),
        KeyCode::Right if keypad => "KP_6".into(),
        KeyCode::Home if keypad => "KP_7".into(),
        KeyCode::End if keypad => "KP_1".into(),
        KeyCode::PageUp if keypad => "KP_9".into(),
        KeyCode::PageDown if keypad => "KP_3".into(),
        KeyCode::Up => "Up".into(),
        KeyCode::Down => "Down".into(),
        KeyCode::Left => "Left".into(),
        KeyCode::Right => "Right".into(),
        KeyCode::PageUp => "Page_Up".into(),
        KeyCode::PageDown => "Page_Down".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::Insert => "Insert".into(),
        KeyCode::Delete => "Delete".into(),
        KeyCode::Enter => "Return".into(),
        KeyCode::Esc => "Escape".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::Backspace => "BackSpace".into(),
        _ => return None,
    };
    Some(KeyChord::new(&name, ctrl, alt, shift))
}

fn which(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

fn spawn_quietly(program: &str, args: &[&str]) {
    let _ = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

fn player_command(player: &str, path: &str, volume: f64) -> Command {
    let mut cmd = Command::new(player);
    match player {
        "pw-play" => {
            cmd.arg(format!("--volume={:.2}", volume.clamp(0.0, 1.0)));
        }
        "paplay" => {
            cmd.arg(format!("--volume={}", (volume.clamp(0.0, 1.0) * 65536.0) as u32));
        }
        _ => {}
    }
    cmd.arg(path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_inserts_stop_at_placeholders() {
        assert_eq!(insert_for("/script load <path>"), "/script load ");
        assert_eq!(insert_for("/script reload"), "/script reload");
        assert_eq!(insert_for("/set [<name> [<value>]]"), "/set ");
        assert_eq!(insert_for("/@<code>"), "/@");
        assert_eq!(insert_for("/images on|off"), "/images ");
        assert_eq!(insert_for("/map [go|here|addroom|import|track|clear] ..."), "/map ");
    }

    #[test]
    fn addresses() {
        let w = parse_address("lambda.moo.mud.org:8888").unwrap();
        assert_eq!((w.host.as_str(), w.port, w.tls), ("lambda.moo.mud.org", 8888, false));
        assert!(parse_address("aardmud.org 4010 tls").unwrap().tls);
        assert!(parse_address("Aardwolf").is_none());
        assert!(parse_address("localhost 4000").is_some());
    }

    #[test]
    fn macro_chords_use_gdk_names() {
        let key = |code, m| KeyEvent::new(code, m);
        assert_eq!(chord(&key(KeyCode::F(5), KeyModifiers::NONE)).unwrap().to_string(), "F5");
        assert_eq!(chord(&key(KeyCode::Char('K'), KeyModifiers::SHIFT)).unwrap().to_string(), "K");
        assert_eq!(chord(&key(KeyCode::Char('k'), KeyModifiers::CONTROL | KeyModifiers::SHIFT)).unwrap().to_string(), "Ctrl+Shift+K");
        let mut kp = key(KeyCode::Char('8'), KeyModifiers::NONE);
        kp.state = KeyEventState::KEYPAD;
        assert_eq!(chord(&kp).unwrap().to_string(), "KP_8");
    }
}
