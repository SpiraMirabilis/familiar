//! Application state and behaviour: tabs, overlays, keys, mouse, commands and side effects.
//! Drawing is in `ui.rs`; this module never touches the terminal directly.

use std::ops::Range;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use familiar_config::{Config, Settings, World};
use familiar_engine::keymap::{Platform, Scope};
use familiar_engine::{EditRequest, KeyChord, Keymap, Output, Status};
use familiar_net::SessionEvent;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use tokio::sync::mpsc::UnboundedSender;

use familiar_agent::{AgentRequest, Agents};
use crate::buffer::{Buffer, Cell, Selection};
use crate::editor::Document;
use crate::syntax::{Editor, Installed};
use crate::rules::RulesView;
use crate::term::{PtyEvent, PtySender, Term};
use crate::form::{FieldKind, WorldForm};
use familiar_engine::fuzzy;
use crate::keyboard::KeysView;
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
    ("/colors [standard|terminal]", "draw the 16 basic colours exactly, or with the terminal's theme"),
    ("/config", "edit the config file in $EDITOR"),
    ("/editor [tab|sidebar|fullscreen]", "open $EDITOR in a tab, beside the world, or with the whole terminal"),
    ("/editor syntax [force]", "install MOO highlighting for $EDITOR (micro, Vim, Neovim, Emacs)"),
    ("/agent [<name>|stop|list]", "run an LLM agent (claude, codex, agy, grok) beside the world, with the world's tools"),
    ("/rules", "edit triggers, aliases, keyboard macros and variables"),
    ("/keys", "change the keyboard shortcuts"),
    ("/import <path>", "import worlds, characters and rules from a BeipMU Config.txt"),
    ("/quit", "disconnect everything and quit"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tab {
    Main(u64),
    Puppet(u64, String),
    /// An `$EDITOR` running in the tab (an `EditorTab::id`).
    Editor(u64),
}

/// How often the output scrolls while a selection is held at its top or bottom edge.
const AUTOSCROLL: Duration = Duration::from_millis(40);

pub enum Action {
    Connect { world: Box<World>, character: Option<String> },
    /// A keyboard shortcut's action, by id (`familiar_engine::keymap`).
    Run(&'static str),
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
    Rules(Box<RulesView>),
    /// The keyboard shortcuts, to change.
    Keys(Box<KeysView>),
    /// The search bar over the output.
    Find(Box<Find>),
    Help,
}

/// Searching the output: what's typed, and the match being shown.
pub struct Find {
    pub query: Input,
    /// The `Buffer::id` searched.
    pub buffer: u64,
    /// The match shown: its entry and bytes.
    pub current: Option<(u64, Range<usize>)>,
    /// Which match that is, counting from the newest, and how many there are.
    pub position: (usize, usize),
}

/// A suggestion under a `/` command being typed.
pub struct Suggestion {
    /// The input text if it's accepted.
    pub insert: String,
    pub label: String,
    pub detail: String,
}

/// Something to open in `$EDITOR`.
#[derive(Clone)]
pub enum External {
    /// Open an editor on `request`, at `line` if given.
    Edit { session: u64, request: EditRequest, line: Option<u32> },
    /// The input box of a session's tab, or of one of its puppets.
    EditInput { session: u64, puppet: Option<String> },
    EditConfig,
    /// An LLM agent working on a session's world. It isn't an edit: nothing comes back.
    Agent { session: u64 },
}

/// An editor running in a tab, or beside the world it was opened from.
pub struct EditorTab {
    pub id: u64,
    pub title: String,
    pub term: Term,
    job: External,
    /// The file being edited; an agent has none.
    document: Option<Document>,
    /// An agent's MCP endpoint token.
    agent: Option<String>,
    /// The session whose main tab it sits beside, taking the sidebar's place. It has no tab
    /// of its own then.
    pub docked: Option<u64>,
    /// For a docked editor: keys go to it rather than to the world's input.
    pub focused: bool,
}

impl EditorTab {
    pub fn is_agent(&self) -> bool {
        self.agent.is_some()
    }

    /// Throws away the edit, if it is one. Returns what to tell the user.
    fn discard(self) -> String {
        match self.document {
            Some(document) => {
                document.discard();
                format!("Closed the editor on {} without saving", self.title)
            }
            None => format!("Stopped {}", self.title),
        }
    }
}

/// The map's screen area, the map-unit rectangle it shows (x0, y0, x1, y1), and its area name.
pub type MapHit = (Rect, (f64, f64, f64, f64), String);

/// A row of a buffer on screen.
pub struct RowHit {
    pub y: u16,
    pub x: u16,
    pub width: u16,
    /// The `Buffer::id`, the entry and which of its wrapped rows this is.
    pub buffer: u64,
    pub seq: u64,
    pub row: usize,
    /// In the live tail shown under a scrolled-back view, which stays where it is.
    pub live: bool,
}

/// Where the left button went down on a buffer's text.
#[derive(Clone, Copy)]
pub struct Press {
    /// The `Buffer::id`.
    pub buffer: u64,
    pub cell: Cell,
    /// It went down in the live tail (see [`RowHit::live`]).
    pub live: bool,
}

/// Where things were drawn, for the mouse.
#[derive(Default)]
pub struct Hits {
    pub tabs: Vec<(Rect, usize)>,
    pub panels: Vec<(Rect, usize)>,
    /// (row, first column, last column + 1, uri)
    pub links: Vec<(u16, u16, u16, String)>,
    pub rows: Vec<RowHit>,
    pub output: Rect,
    /// The part of the output that scrolls (all of it, or the history above the live tail),
    /// and the `Buffer::id` shown there.
    pub scrolling: Option<(u64, Rect)>,
    pub sidebar: Rect,
    pub picker_items: Vec<(Rect, usize)>,
    /// The map's area and the map-unit rectangle it shows: (x0, y0, x1, y1).
    pub map: Option<MapHit>,
    /// Where an editor tab's screen is.
    pub editor: Rect,
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
    /// An edit that needs the terminal to itself; the main loop runs it.
    pub external: Option<External>,
    pub editors: Vec<EditorTab>,
    pty_tx: PtySender,
    /// LLM agents: the MCP server that serves them the worlds' tools, and their calls.
    agents: Agents,
    /// The size of the whole terminal at the last draw.
    pub screen: (u16, u16),
    /// Text being selected (or last selected) with the mouse.
    pub selection: Option<Selection>,
    press: Option<Press>,
    /// Where the mouse is while it's dragging a selection, and when the view last scrolled to
    /// keep up with it.
    drag: Option<(Position, Instant)>,
    /// Text for the main loop to put on the clipboard.
    pub clipboard: Option<String>,
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
    /// The keyboard shortcuts, from `[settings.terminal_keys]` and the defaults.
    pub keymap: Keymap,
    /// What was last searched for in the output, for Find Next with the search bar closed.
    last_find: Option<Box<Find>>,
}

impl App {
    pub fn new(net_tx: UnboundedSender<NetEvent>, pty_tx: PtySender, agent_tx: UnboundedSender<AgentRequest>) -> Self {
        let config_path = familiar_config::default_config_path();
        let (config, config_error) = match Config::load(&config_path) {
            Ok(c) => (c, None),
            Err(e) => (Config::default(), Some(e.to_string())),
        };
        crate::theme::set_terminal_colors(config.settings.terminal_colors);
        let keymap = Keymap::new(&config.settings, Platform::Terminal);
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
            editors: Vec::new(),
            pty_tx,
            agents: Agents::new(agent_tx, None),
                    screen: (80, 24),
            selection: None,
            press: None,
            drag: None,
            clipboard: None,
            suggestion: 0,
            suggestions_hidden: false,
            started: Instant::now(),
            view: (80, 24),
            naws: (0, 0),
            esc_at: None,
            delete_armed: None,
            sounds: familiar_platform::SoundPlayer::new(),
            keymap,
            last_find: None,
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

    /// An action's first key as people read it ("Alt+→"), for hints. None if it has no key.
    pub fn key_for(&self, id: &str) -> Option<String> {
        self.keymap.keys(id).first().map(crate::keys::pretty)
    }

    /// A message in the current tab's output, or a toast where there's no output.
    pub(super) fn notice(&mut self, text: &str) {
        match self.current_buffer() {
            Some(buffer) => {
                buffer.notice(text);
                buffer.scroll_to_bottom();
            }
            None => self.toast(text),
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
            Tab::Editor(_) => None,
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
            Tab::Editor(id) => match self.editors.iter().find(|e| e.id == *id) {
                Some(e) if e.is_agent() => format!("✦ {}", e.title),
                Some(e) => format!("✎ {}", e.title),
                None => String::new(),
            },
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
                // An editor beside the world goes with it.
                let docked: Vec<u64> = self.editors.iter().filter(|e| e.docked == Some(id)).map(|e| e.id).collect();
                for editor in docked {
                    if let Some(editor) = self.remove_editor(editor) {
                        let message = editor.discard();
                        self.toast(&message);
                    }
                }
                // Calls still waiting fail: their results can't arrive now.
                self.agents.forget(id);
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
            Tab::Editor(id) => {
                // Dropping it stops the editor.
                if let Some(editor) = self.remove_editor(id) {
                    let message = editor.discard();
                    self.toast(&message);
                }
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
        // A selection held at the edge of the output keeps scrolling without the mouse moving.
        if self.press.is_some() && self.drag.is_some() {
            wake = wake.min(now + AUTOSCROLL);
        }
        wake
    }

    pub fn on_tick(&mut self) {
        let now = Instant::now();
        if let Some((at, _)) = self.drag {
            self.drag_to(at);
        }
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
                crate::theme::set_terminal_colors(self.config.settings.terminal_colors);
                self.keymap = Keymap::new(&self.config.settings, Platform::Terminal);
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

mod actions;
mod agents;
mod editing;
pub use editing::{DOCK_MIN_WIDTH, dock_width};
mod effects;
mod events;
mod keyboard;
mod overlays;
mod rules;

use events::parse_address;

#[cfg(test)]
mod tests {
    use super::events::{chord, is_readline_key, reaches_past_editor};
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
    fn an_editor_beside_a_world_leaves_the_world_room() {
        // 80 columns for the editor (and its border) once the world can keep 40.
        assert_eq!(dock_width(120), 80);
        assert_eq!(dock_width(160), 80);
        // Wider than that, half each.
        assert_eq!(dock_width(200), 100);
        // Narrower, the world keeps its 40 and the editor has the rest.
        assert_eq!(dock_width(110), 70);
        assert_eq!(dock_width(DOCK_MIN_WIDTH), 50);
        assert!((DOCK_MIN_WIDTH..300).all(|w| w - dock_width(w) >= 40));
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
        assert_eq!(chord(&key(KeyCode::Char(']'), KeyModifiers::ALT)).unwrap().to_string(), "Alt+bracketright");
        assert_eq!(chord(&key(KeyCode::Char('.'), KeyModifiers::CONTROL)).unwrap().to_string(), "Ctrl+period");
    }

    #[test]
    fn key_presses_find_their_shortcuts() {
        let keymap = Keymap::new(&Settings::default(), Platform::Terminal);
        let action = |code, m| keymap.lookup(Scope::Window, &chord(&KeyEvent::new(code, m)).unwrap());
        assert_eq!(action(KeyCode::Char('k'), KeyModifiers::CONTROL), Some("palette"));
        assert_eq!(action(KeyCode::Char(']'), KeyModifiers::ALT), Some("next-panel"));
        assert_eq!(action(KeyCode::Char('3'), KeyModifiers::ALT), Some("tab-3"));
        assert_eq!(action(KeyCode::PageDown, KeyModifiers::CONTROL), Some("next-tab"));
        assert_eq!(action(KeyCode::F(1), KeyModifiers::NONE), Some("keyboard"));
        assert_eq!(action(KeyCode::F(3), KeyModifiers::SHIFT), Some("find-previous"));
        assert_eq!(action(KeyCode::Char('b'), KeyModifiers::ALT), cfg!(target_os = "macos").then_some("prev-tab"));
        assert_eq!(action(KeyCode::Char('a'), KeyModifiers::NONE), None);
    }

    #[test]
    fn an_editor_keeps_most_keys() {
        let key = |text: &str| KeyChord::parse(text).unwrap();
        assert!(reaches_past_editor("next-tab", &key("Alt+Right"), false));
        assert!(reaches_past_editor("tab-2", &key("Alt+2"), false));
        assert!(reaches_past_editor("move-editor", &key("F7"), false));
        assert!(!reaches_past_editor("editor-focus", &key("F6"), false), "only beside a world");
        assert!(reaches_past_editor("editor-focus", &key("F6"), true));
        assert!(!reaches_past_editor("palette", &key("Ctrl+K"), true));
        // Esc then b, quickly, in Vim.
        assert!(!reaches_past_editor("prev-tab", &key("Alt+B"), false));
        assert!(!reaches_past_editor("next-tab", &key("Alt+F"), true));
    }

    #[test]
    fn readline_keys_are_shortcuts_only_on_an_empty_line() {
        assert!(is_readline_key(&KeyChord::parse("Ctrl+K").unwrap()));
        assert!(is_readline_key(&KeyChord::parse("Ctrl+W").unwrap()));
        assert!(!is_readline_key(&KeyChord::parse("Ctrl+P").unwrap()));
        assert!(!is_readline_key(&KeyChord::parse("Ctrl+Shift+K").unwrap()));
    }
}
