//! Application state and behaviour: tabs, overlays, keys, mouse, commands and side effects.
//! Drawing is in `ui.rs`; this module never touches the terminal directly.

use std::path::PathBuf;
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
    sounds: familiar_platform::SoundPlayer,
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
            sounds: familiar_platform::SoundPlayer::new(),
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
        for path in self.sessions[idx].engine.startup_scripts() {
            out.push(Output::Notice(format!("Running {}", familiar_config::paths::display(&path))));
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
        self.sounds.stop(None);
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
        self.sounds.tidy();
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

    // ----- Input -----

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

}

mod effects;
mod events;
mod overlays;

use events::parse_address;

#[cfg(test)]
mod tests {
    use super::events::chord;
    use super::overlays::insert_for;
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
