//! One connection: its engine, scrollback, spawn windows, stat panes, puppets and input.

use std::collections::BTreeMap;

use familiar_ansi::StyledLine;
use familiar_config::{Character, Config, World};
use familiar_engine::stats::StatPane;
use familiar_engine::{EditRequest, Engine, Output, Status, Target};

use crate::buffer::{Buffer, Kind};
use crate::input::Input;

/// Things a session can't do by itself; the app handles them.
pub enum Effect {
    Notify { title: String, body: String },
    PlaySound(String),
    Media(familiar_engine::media::MediaCommand),
    OpenUri(String),
    Edit(EditRequest),
    Reconnect,
    ReloadScripts,
    Bell,
    Speak(String),
    SpeechStop,
    /// Save variables to the config.
    Variables { scope: familiar_config::Scope, variables: BTreeMap<String, String> },
    Settings(familiar_config::Settings),
    /// Open a puppet's tab (and switch to it if `focus`).
    OpenPuppet { name: String, focus: bool },
    /// Show this sidebar panel.
    ShowPanel(Panel),
    /// The tools the world offers an agent changed.
    AgentTools,
    /// An agent tool call finished.
    AgentResult { id: u64, ok: bool, content: String },
    /// `/agent ...`, from an alias or a macro (typed, the app catches it first).
    Agent(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Panel {
    Map,
    Stats(String),
    Spawn(String),
}

impl Panel {
    pub fn title(&self) -> &str {
        match self {
            Panel::Map => "Map",
            Panel::Stats(name) | Panel::Spawn(name) => name,
        }
    }
}

pub struct Pane {
    pub name: String,
    pub buffer: Buffer,
    pub unread: bool,
}

pub struct Puppet {
    pub name: String,
    pub buffer: Buffer,
    pub input: Input,
    pub unread: usize,
}

pub struct Session {
    pub id: u64,
    pub engine: Engine,
    /// Bumped on each connect, so events from an old connection are ignored.
    pub generation: u64,
    pub output: Buffer,
    pub input: Input,
    /// The last prompt from the server, shown on the input box.
    pub prompt: Option<StyledLine>,
    pub status: Status,
    /// MCP status text.
    pub status_text: String,
    pub password: bool,
    pub logging: bool,
    pub unread: usize,
    pub important: bool,
    pub spawns: Vec<Pane>,
    pub stats: BTreeMap<String, StatPane>,
    pub puppets: Vec<Puppet>,
    /// Which sidebar panel is showing.
    pub panel: usize,
    pub scrollback: usize,
    /// The last program sent from the editor, and the first error line reported for it, so
    /// Alt+E can reopen it there.
    pub last_edit: Option<(EditRequest, Option<u32>)>,
}

impl Session {
    pub fn new(id: u64, config: &Config, world: World, character: Option<Character>) -> (Self, Vec<Output>) {
        let (mut engine, out) = Engine::new(config, world, character);
        engine.set_agents_supported(true);
        engine.set_keymap_platform(familiar_engine::keymap::Platform::Terminal);
        let s = &config.settings;
        (
            Self {
                id,
                engine,
                generation: 0,
                output: Buffer::new(s.scrollback_lines),
                input: Input::new(s.input_history_size),
                prompt: None,
                status: Status::Disconnected(None),
                status_text: String::new(),
                password: false,
                logging: false,
                unread: 0,
                important: false,
                spawns: Vec::new(),
                stats: BTreeMap::new(),
                puppets: Vec::new(),
                panel: 0,
                scrollback: s.scrollback_lines,
                last_edit: None,
            },
            out,
        )
    }

    pub fn title(&self) -> String {
        self.engine.title()
    }

    /// Sidebar panels this session has, in order.
    pub fn panels(&self) -> Vec<Panel> {
        let mut panels = Vec::new();
        if !self.engine.map().is_empty() {
            panels.push(Panel::Map);
        }
        panels.extend(self.stats.keys().cloned().map(Panel::Stats));
        panels.extend(self.spawns.iter().map(|p| Panel::Spawn(p.name.clone())));
        panels
    }

    pub fn spawn_mut(&mut self, name: &str) -> (&mut Pane, bool) {
        let existed = self.spawns.iter().any(|p| p.name == name);
        if !existed {
            self.spawns.push(Pane { name: name.to_string(), buffer: Buffer::new(2000), unread: false });
        }
        (self.spawns.iter_mut().find(|p| p.name == name).unwrap(), !existed)
    }

    pub fn puppet_mut(&mut self, name: &str) -> &mut Puppet {
        if !self.puppets.iter().any(|p| p.name == name) {
            let input = Input::new(200);
            self.puppets.push(Puppet { name: name.to_string(), buffer: Buffer::new(self.scrollback), input, unread: 0 });
        }
        self.puppets.iter_mut().find(|p| p.name == name).unwrap()
    }

    /// Shows the engine's outputs. `visible` is whether this session's main view is on screen
    /// (otherwise new lines count as unread).
    pub fn apply(&mut self, outputs: Vec<Output>, visible: bool) -> Vec<Effect> {
        let mut effects = Vec::new();
        for output in outputs {
            match output {
                Output::Line { line, target: Target::Main } => {
                    // An unterminated prompt is replaced by whatever comes next.
                    self.prompt = None;
                    if line.bell {
                        effects.push(Effect::Bell);
                    }
                    self.output.push(line, Kind::Server);
                }
                Output::Line { line, target: Target::Spawn(name) } => {
                    let (pane, new) = self.spawn_mut(&name);
                    pane.buffer.push(line, Kind::Server);
                    pane.unread = true;
                    if new {
                        effects.push(Effect::ShowPanel(Panel::Spawn(name)));
                    }
                }
                // The prompt rides on the input box's border rather than filling the scrollback.
                Output::Prompt { line, .. } => self.prompt = (!line.text.trim().is_empty()).then_some(line),
                Output::Notice(text) => self.output.notice(&text),
                Output::Echo(text) => {
                    for l in text.lines() {
                        self.output.push(StyledLine::plain(l), Kind::Echo);
                    }
                }
                Output::Bell => effects.push(Effect::Bell),
                Output::PasswordMode(on) => self.password = on,
                Output::Status(status) => {
                    if let Status::Disconnected(reason) = &status {
                        let text = match reason {
                            Some(e) => format!("Disconnected: {e}"),
                            None => "Disconnected".into(),
                        };
                        self.output.notice(&format!("{text}. Press Enter on an empty line to reconnect."));
                        self.prompt = None;
                        self.password = false;
                    }
                    self.status = status;
                }
                Output::Activity { important } => {
                    if !visible {
                        self.unread += 1;
                        self.important |= important;
                    }
                }
                Output::Notify { title, body } => effects.push(Effect::Notify { title, body }),
                Output::PlaySound(path) => effects.push(Effect::PlaySound(path)),
                Output::Media(cmd) => effects.push(Effect::Media(cmd)),
                Output::OpenUri(uri) => effects.push(Effect::OpenUri(uri)),
                Output::SetInput(text) => self.input.set(&text),
                Output::ClearOutput => self.output.clear(),
                Output::Reconnect => effects.push(Effect::Reconnect),
                Output::ReloadScripts => effects.push(Effect::ReloadScripts),
                Output::VariablesChanged { scope, variables } => effects.push(Effect::Variables { scope, variables }),
                Output::LoggingChanged(path) => {
                    self.logging = path.is_some();
                    if let Some(path) = path {
                        self.output.notice(&format!("Logging to {}", path.display()));
                    }
                }
                Output::CompileError { line, message, .. } => {
                    let first = match &mut self.last_edit {
                        Some((_, at @ None)) => {
                            *at = Some(line);
                            true
                        }
                        _ => false,
                    };
                    let hint = if first { " · Alt+E to fix it" } else { "" };
                    self.output.notice(&format!("Compile error on line {line}: {message}{hint}"));
                }
                Output::SettingsChanged(settings) => effects.push(Effect::Settings(settings)),
                Output::OpenPuppet { name, focus } => {
                    self.puppet_mut(&name);
                    effects.push(Effect::OpenPuppet { name, focus });
                }
                Output::PuppetLine { name, line } => {
                    let new = !self.puppets.iter().any(|p| p.name == name);
                    let puppet = self.puppet_mut(&name);
                    puppet.buffer.push(line, Kind::Server);
                    puppet.unread += 1;
                    if new {
                        effects.push(Effect::OpenPuppet { name, focus: false });
                    }
                }
                Output::PuppetEcho { name, text } => {
                    self.puppet_mut(&name).buffer.push(StyledLine::plain(text), Kind::Echo);
                }
                Output::Speak(text) => effects.push(Effect::Speak(text)),
                Output::SpeechStop => effects.push(Effect::SpeechStop),
                Output::MapChanged => {}
                Output::ShowMap => effects.push(Effect::ShowPanel(Panel::Map)),
                Output::Stats { pane, data } => match data {
                    Some(data) => {
                        let new = !self.stats.contains_key(&pane);
                        self.stats.insert(pane.clone(), data);
                        if new {
                            effects.push(Effect::ShowPanel(Panel::Stats(pane)));
                        }
                    }
                    None => {
                        self.stats.remove(&pane);
                    }
                },
                Output::StatusText(text) => self.status_text = text,
                Output::Edit(request) => effects.push(Effect::Edit(request)),
                Output::AgentTools(_) => effects.push(Effect::AgentTools),
                Output::AgentResult { id, ok, content } => effects.push(Effect::AgentResult { id, ok, content }),
                Output::Agent(args) => effects.push(Effect::Agent(args)),
                Output::WebView(familiar_engine::WebViewCommand::Open(open)) => {
                    match open.url {
                        Some(url) => self.output.notice(&format!("The server opened a web view; open it in a browser: {url}")),
                        None => self.output.notice("The server opened a web view, which needs the GUI client"),
                    }
                }
                Output::InlineImage { url } => {
                    let _ = url;
                }
                // Pictures, web views, tilemaps and web view traffic need the GUI.
                Output::WebView(_)
                | Output::LineAvatar(_)
                | Output::AvatarUpdated(_)
                | Output::Tilemap { .. }
                | Output::Gmcp { .. }
                | Output::Received(_)
                | Output::Sent(_) => {}
            }
        }
        effects
    }
}

impl familiar_agent::World for Session {
    fn agent_tools(&self) -> Option<&[familiar_engine::AgentTool]> {
        self.engine.agent_tools()
    }

    fn agent_call(&mut self, tool: &str, args: &serde_json::Value) -> Result<u64, String> {
        self.engine.agent_call(tool, args)
    }

    fn last_lines(&self, n: usize) -> Vec<String> {
        self.output.last_lines(n).into_iter().map(str::to_string).collect()
    }
}
