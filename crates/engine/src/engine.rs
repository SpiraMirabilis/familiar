//! The per-connection engine. It turns network events and user input into [`Output`]s for a UI to
//! render, and owns everything that doesn't depend on the UI toolkit: ANSI parsing, triggers,
//! aliases, macros, variables, client commands and logging.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use familiar_ansi::{AnsiParser, Color, StyledLine};
use familiar_config::{Bouncer, Character, Config, Highlight, Puppet, Scope, Settings, World};
use familiar_net::{BouncerOptions, ConnectOptions, Encoding, SessionEvent, SessionHandle, SessionOptions, spawn_session};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::expand::{Captures, expand};
use crate::mcp::{AgentTool, Disposition, Mcp, McpEvent};
use crate::media::{Media, MediaCommand};
use crate::restore::{self, RestoreLog};
use crate::rules::CompiledRules;
use crate::script::{ScriptAction, ScriptContext, ScriptHost};
use crate::stats::{StatPane, Stats};
use crate::tilemap::{Tilemap, Tilemaps};
use familiar_map::{Atlas, RoomId, RoomInfo};
use crate::{KeyChord, Logger};

/// A trigger that sends more often than this is suspended for the session, so a trigger that
/// matches its own output can't flood the server.
const MAX_SENDS: usize = 20;
const SEND_WINDOW: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Main,
    /// A spawn window, created by the UI the first time it's named.
    Spawn(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Connecting,
    Connected,
    Disconnected(Option<String>),
}

/// Text the server wants the user to edit (MCP simpleedit).
#[derive(Debug, Clone, PartialEq)]
pub struct EditRequest {
    /// Opaque; pass it back to [`Engine::save_edit`].
    pub reference: String,
    pub name: String,
    /// "string", "string-list" or "moo-code" from MCP, or "local" for a scratch editor.
    pub kind: String,
    pub content: String,
    /// Language for highlighting and indentation ("moo", "muf", ...). None for plain text.
    pub language: Option<String>,
}

impl EditRequest {
    /// A scratch editor whose text is sent line by line, e.g. after an @program command.
    pub fn is_local(&self) -> bool {
        self.kind == "local"
    }
}

/// How long after an upload to watch the server's replies for compile errors.
const COMPILE_WATCH: Duration = Duration::from_secs(10);

/// An image shown to the left of a line (BeipMU avatars).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Avatar {
    /// The beip.line.id this came from, so later beip.ids updates can find it.
    pub id: Option<String>,
    /// None until the server says (beip.ids), for an id we've asked about.
    pub url: Option<String>,
    pub hover: Option<String>,
    pub click_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WebViewOpen {
    /// Opening another web view with the same id replaces it.
    pub id: String,
    pub url: Option<String>,
    /// HTML to show when there's no URL.
    pub source: Option<String>,
    /// "left", "right", "top" or "bottom" to dock it; None for its own window.
    pub dock: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub headers: Vec<(String, String)>,
    /// Opened by the server (GMCP) rather than the user, so the UI should ask permission.
    pub from_server: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WebViewCommand {
    Open(WebViewOpen),
    Close { id: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    Line { line: StyledLine, target: Target },
    /// A prompt. `terminated` is false for an unterminated line shown after a pause, which the
    /// next output replaces.
    Prompt { line: StyledLine, terminated: bool },
    /// A message from the client, shown dimmed.
    Notice(String),
    /// A command that was sent, shown in the output.
    Echo(String),
    Bell,
    PasswordMode(bool),
    Status(Status),
    /// New output arrived. `important` is set by triggers.
    Activity { important: bool },
    Notify { title: String, body: String },
    PlaySound(String),
    OpenUri(String),
    SetInput(String),
    ClearOutput,
    /// The UI should reconnect (the engine can't, since the UI owns the event loop).
    Reconnect,
    /// The UI should start a fresh script host and call [`Engine::set_script_host`] again.
    ReloadScripts,
    /// Variables at `scope` changed and should be saved.
    VariablesChanged { scope: Scope, variables: BTreeMap<String, String> },
    LoggingChanged(Option<PathBuf>),
    /// The server reported a compile error in a program that was just sent from an editor.
    /// `line` counts from 1.
    CompileError { reference: String, line: u32, message: String },
    /// Put this avatar at the start of the next main-window line.
    LineAvatar(Avatar),
    /// beip.ids changed an avatar; update images already shown with this id.
    AvatarUpdated(Avatar),
    /// Show this image under the line just added (a link to an image, with inline images on).
    InlineImage { url: String },
    /// Settings changed (e.g. /images); the UI should save them.
    SettingsChanged(Settings),
    /// Open the tab for this puppet. `focus` switches to it (when the user asked for it, not
    /// when a line arrived).
    OpenPuppet { name: String, focus: bool },
    /// A line for a puppet's tab.
    PuppetLine { name: String, line: StyledLine },
    /// A command typed in a puppet's tab, to show there.
    PuppetEcho { name: String, text: String },
    /// Speak this text aloud.
    Speak(String),
    /// Stop speaking.
    SpeechStop,
    /// A Beip.Tilemap map was created or changed.
    Tilemap { name: String, map: Tilemap },
    /// The map or the current room changed; redraw it.
    MapChanged,
    /// The user asked to see the map.
    ShowMap,
    /// A Beip.Stats pane changed. None means it was removed.
    Stats { pane: String, data: Option<StatPane> },
    Media(MediaCommand),
    /// MCP status text (dns-com-awns-status). Empty clears it.
    StatusText(String),
    /// MCP simpleedit: open an editor.
    Edit(EditRequest),
    WebView(WebViewCommand),
    /// Every GMCP message, for web views that subscribe to them.
    Gmcp { package: String, data: String },
    /// A line from the server before any processing, for web views.
    Received(String),
    /// A command sent to the server, for web views.
    Sent(String),
    /// The tools the world offers an LLM agent changed (dns-com-familiar-agent).
    AgentTools(Vec<AgentTool>),
    /// The result of [`Engine::agent_call`]: text for the agent, and whether the tool succeeded.
    AgentResult { id: u64, ok: bool, content: String },
    /// `/agent <args>`, for the frontend: run, stop or list LLM agents (only if it said it can,
    /// with [`Engine::set_agents_supported`]).
    Agent(String),
}

pub struct Engine {
    world: World,
    character: Option<Character>,
    settings: Settings,
    bouncer: Bouncer,
    /// The current connection goes through the bouncer, which logs in by itself.
    via_bouncer: bool,
    /// Most specific first. Index 0 is where `/set` stores variables.
    scopes: Vec<Scope>,
    variables: Vec<BTreeMap<String, String>>,
    rules: CompiledRules,
    ansi: AnsiParser,
    logger: Option<Logger>,
    /// Records output for the character's restore log, while it's connected.
    restore: Option<RestoreLog>,
    handle: Option<SessionHandle>,
    window_size: Option<(u16, u16)>,
    server_echo: bool,
    stats: Stats,
    tilemaps: Tilemaps,
    avatars: std::collections::HashMap<String, Avatar>,
    /// Ids we've sent beip.id.request for.
    avatars_requested: std::collections::HashSet<String>,
    /// From beip.line.*, for the next line.
    next_avatar: Option<Avatar>,
    media: Media,
    mcp: Mcp,
    /// Show GMCP traffic as notices.
    gmcp_debug: bool,
    /// Show MCP traffic as notices.
    mcp_debug: bool,
    webviews_supported: bool,
    agents_supported: bool,
    script: Option<Box<dyn ScriptHost>>,
    puppets: Vec<(Puppet, Option<fancy_regex::Regex>)>,
    /// Puppets whose tabs are open.
    open_puppets: std::collections::HashSet<String>,
    map: Atlas,
    map_path: Option<PathBuf>,
    current_room: Option<RoomId>,
    /// The last movement command sent, for placing new rooms and live tracking.
    last_move: Option<String>,
    /// Follow room names in the output to track position (when there's no Room.Info).
    map_tracking: bool,
    /// The server sends Room.Info, so tracking by name isn't needed.
    room_info_seen: bool,
    /// After an upload: which editor it came from, and until when to look for errors.
    compile_watch: Option<(String, Instant)>,
    /// Tools the server offers an LLM agent.
    agent_tools: Vec<AgentTool>,
    /// Agent tool calls waiting for results, by id.
    agent_calls: std::collections::HashMap<u64, agent::AgentCall>,
    next_agent_call: u64,
    /// How deeply script actions are nested (a script typing input that runs a script...).
    script_depth: u32,
    /// Commands waiting to run (/delay).
    delays: Vec<Delay>,
    next_delay_id: u32,
}

/// Deeper nesting than this is assumed to be a loop.
const MAX_SCRIPT_DEPTH: u32 = 8;

impl Engine {
    pub fn new(config: &Config, world: World, character: Option<Character>) -> (Self, Vec<Output>) {
        let mut engine = Self {
            settings: config.settings.clone(),
            bouncer: config.bouncer.clone(),
            via_bouncer: false,
            world,
            character,
            scopes: Vec::new(),
            variables: Vec::new(),
            rules: CompiledRules::default(),
            ansi: AnsiParser::new(),
            logger: None,
            restore: None,
            handle: None,
            window_size: None,
            server_echo: false,
            stats: Stats::default(),
            tilemaps: Tilemaps::default(),
            avatars: Default::default(),
            avatars_requested: Default::default(),
            next_avatar: None,
            media: Media::default(),
            mcp: Mcp::default(),
            gmcp_debug: false,
            mcp_debug: false,
            webviews_supported: false,
            agents_supported: false,
            script: None,
            puppets: Vec::new(),
            open_puppets: Default::default(),
            map: Atlas::default(),
            map_path: None,
            current_room: None,
            last_move: None,
            map_tracking: true,
            room_info_seen: false,
            compile_watch: None,
            agent_tools: Vec::new(),
            agent_calls: Default::default(),
            next_agent_call: 1,
            script_depth: 0,
            delays: Vec::new(),
            next_delay_id: 1,
        };
        let mut out = engine.reload(config);
        engine.replay_restore(&mut out);
        let path = familiar_config::map_path(&engine.world.name);
        match Atlas::load(&path) {
            Ok(map) => engine.map = map,
            Err(e) => out.push(Output::Notice(format!("Couldn't load the map: {e}"))),
        }
        engine.map_path = Some(path);
        (engine, out)
    }

    /// Picks up changed rules, variables and settings. Returns notices for rules that failed to
    /// compile.
    pub fn reload(&mut self, config: &Config) -> Vec<Output> {
        self.settings = config.settings.clone();
        self.bouncer = config.bouncer.clone();
        // Pick up edits to the character (its puppets, login) from the config.
        if let Some(name) = self.character.as_ref().map(|c| c.name.clone())
            && let Some(saved) = config.world(&self.world.name).and_then(|w| w.characters.iter().find(|c| c.name == name))
        {
            self.character = Some(saved.clone());
        }
        if !self.character.as_ref().is_some_and(|c| c.restore_log) {
            self.restore = None;
        }
        let mut notices = Vec::new();
        self.puppets = self
            .character
            .iter()
            .flat_map(|c| c.puppets.iter())
            .filter(|p| !p.name.is_empty() && !p.receive_prefix.is_empty())
            .map(|p| {
                let regex = p.regex.then(|| fancy_regex::Regex::new(&p.receive_prefix));
                match regex {
                    Some(Err(e)) => {
                        notices.push(Output::Notice(format!("Puppet {}'s receive pattern is invalid: {e}", p.name)));
                        (p.clone(), None)
                    }
                    Some(Ok(re)) => (p.clone(), Some(re)),
                    None => (p.clone(), None),
                }
            })
            .filter(|(p, re)| !p.regex || re.is_some())
            .collect();
        let character = self.character.as_ref().map(|c| c.name.as_str());
        self.scopes = config.scopes_for(&self.world.name, character);
        let layers: Vec<_> = self.scopes.iter().filter_map(|s| config.rules(s)).collect();
        self.variables = layers.iter().map(|r| r.variables.clone()).collect();
        let (rules, errors) = CompiledRules::compile(layers);
        self.rules = rules;
        let merged = self.merged_variables();
        if let Some(script) = self.script.as_mut() {
            script.set_variables(&merged);
        }
        notices.extend(errors.into_iter().map(Output::Notice));
        notices
    }

    /// All variables visible to this session; more specific scopes win.
    pub fn merged_variables(&self) -> BTreeMap<String, String> {
        let mut all = BTreeMap::new();
        for vars in self.variables.iter().rev() {
            all.extend(vars.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        all
    }

    pub fn title(&self) -> String {
        match &self.character {
            Some(c) if !c.name.is_empty() => format!("{} @ {}", c.name, self.world.name),
            _ if self.world.name.is_empty() => format!("{}:{}", self.world.host, self.world.port),
            _ => self.world.name.clone(),
        }
    }

    /// The character this session logs in as, if any.
    pub fn character_name(&self) -> Option<String> {
        self.character.as_ref().map(|c| c.name.clone())
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn scopes(&self) -> &[Scope] {
        &self.scopes
    }

    pub fn is_connected(&self) -> bool {
        self.handle.as_ref().is_some_and(|h| !h.is_closed())
    }

    /// Tells the engine the UI can show web views, so it advertises "WebView 1" over GMCP.
    pub fn set_webviews_supported(&mut self, supported: bool) {
        self.webviews_supported = supported;
    }

    /// Tells the engine the UI can run LLM agents beside the world: it offers MOOs the
    /// dns-com-familiar-agent MCP package from the next connection, and passes `/agent` on as
    /// [`Output::Agent`].
    pub fn set_agents_supported(&mut self, supported: bool) {
        self.agents_supported = supported;
        self.mcp = Mcp::new(supported);
    }

    pub fn is_logging(&self) -> bool {
        self.logger.is_some()
    }

    pub fn variable(&self, name: &str) -> Option<String> {
        self.variables.iter().find_map(|v| v.get(name)).cloned()
    }

    /// Uses an existing handle instead of opening a connection. For tests and embedding.
    pub fn attach(&mut self, handle: SessionHandle) {
        self.handle = Some(handle);
    }

    /// Opens the connection. Must be called inside a tokio runtime. The UI should feed every
    /// event from the returned receiver to [`Engine::handle_event`].
    pub fn connect(&mut self) -> (UnboundedReceiver<SessionEvent>, Vec<Output>) {
        let mut connect = ConnectOptions::new(&self.world.host, self.world.port);
        connect.tls = self.world.tls;
        connect.verify_certificate = self.world.verify_certificate;
        let mut opts = SessionOptions::new(connect);
        opts.encoding = Encoding::from_name(&self.world.encoding).unwrap_or_default();
        opts.prompt_timeout = Some(Duration::from_millis(self.settings.prompt_timeout_ms));
        opts.telnet.client_name = familiar_config::APP_NAME.into();
        opts.telnet.gmcp_supports = vec!["Beip.Stats 1".into(), "Client.Media 1".into()];
        if self.webviews_supported {
            opts.telnet.gmcp_supports.push("WebView 1".into());
        }
        self.mcp = Mcp::new(self.agents_supported);
        self.via_bouncer = self.world.bouncer && !self.bouncer.is_unset();
        if self.via_bouncer {
            let mut connect = ConnectOptions::new(&self.bouncer.host, self.bouncer.port);
            connect.tls = self.bouncer.tls;
            connect.verify_certificate = self.bouncer.verify_certificate;
            let login = self.character.as_ref().map(|c| c.login.as_str()).unwrap_or_default();
            let (steps, _) = familiar_net::login_script(login, scripting::parse_duration);
            opts.bouncer = Some(BouncerOptions {
                connect,
                password: self.bouncer.password.clone(),
                // The bouncer keys sessions by host, port and character, and needs a name.
                character: self.character_name().filter(|n| !n.is_empty()).unwrap_or_else(|| self.world.name.clone()),
                // Always send it, so a changed login (or a removed one) reaches the bouncer.
                login: Some(steps),
            });
        }

        let (handle, events) = spawn_session(opts);
        if let Some((w, h)) = self.window_size {
            handle.set_window_size(w, h);
        }
        self.handle = Some(handle);
        self.ansi = AnsiParser::new();
        let via = if self.via_bouncer { format!(" through the bouncer at {}:{}", self.bouncer.host, self.bouncer.port) } else { String::new() };
        let mut out = vec![
            Output::Status(Status::Connecting),
            Output::Notice(format!(
                "Connecting to {}:{}{}{via}...",
                self.world.host,
                self.world.port,
                if self.world.tls { " with TLS" } else { "" }
            )),
        ];
        if self.world.bouncer && self.bouncer.is_unset() {
            out.push(Output::Notice("This world is set to use a bouncer, but config.toml has no [bouncer] section, so the connection is direct.".into()));
        }
        (events, out)
    }

    pub fn disconnect(&mut self) {
        if let Some(handle) = &self.handle {
            handle.disconnect();
        }
    }

    pub fn set_window_size(&mut self, width: u16, height: u16) {
        if self.window_size == Some((width, height)) {
            return;
        }
        self.window_size = Some((width, height));
        if let Some(handle) = &self.handle {
            handle.set_window_size(width, height);
        }
    }

    /// Sends a GMCP message, e.g. from a web view.
    pub fn send_gmcp(&mut self, package: &str, data: Option<&str>) -> Vec<Output> {
        let mut out = Vec::new();
        if self.gmcp_debug {
            out.push(Output::Notice(format!("GMCP → {package} {}", data.unwrap_or(""))));
        }
        if let Some(handle) = &self.handle {
            handle.send_gmcp(package, data.map(str::to_string));
        }
        out
    }

    /// Sends text for a web view or script: through aliases like typed input, or as is.
    pub fn send_text(&mut self, text: &str, process_aliases: bool) -> Vec<Output> {
        if process_aliases {
            return self.submit(text, false);
        }
        let mut out = Vec::new();
        for line in text.lines() {
            self.send_command(line, &mut out);
        }
        out
    }

    fn program_language(&self) -> String {
        if self.world.program_language.is_empty() { "moo".into() } else { self.world.program_language.clone() }
    }

    /// Sends the user's edited text: back to the server for an MCP simpleedit request, or line
    /// by line for a scratch editor. Afterwards, compile errors in the replies are reported.
    pub fn save_edit(&mut self, request: &EditRequest, content: &str) -> Vec<Output> {
        let out = if request.is_local() {
            let lines: Vec<&str> = content.lines().collect();
            for line in &lines {
                self.send_raw(line);
            }
            vec![Output::Notice(format!("Sent {} lines", lines.len()))]
        } else {
            let mut out = Vec::new();
            for line in self.mcp.simpleedit_set(&request.reference, &request.kind, content) {
                if self.mcp_debug {
                    out.push(Output::Notice(format!("MCP → {line}")));
                }
                self.send_raw(&line);
            }
            out.push(Output::Notice(format!("Sent your changes to \"{}\"", request.name)));
            out
        };
        self.compile_watch = Some((request.reference.clone(), Instant::now() + COMPILE_WATCH));
        out
    }

    /// Records a trigger send. Returns false (and suspends the trigger) if it's firing too often.
    fn note_trigger_send(&mut self, index: usize, out: &mut Vec<Output>) -> bool {
        let t = &mut self.rules.triggers[index];
        let now = Instant::now();
        while t.recent_sends.front().is_some_and(|&at| now.duration_since(at) > SEND_WINDOW) {
            t.recent_sends.pop_front();
        }
        if t.recent_sends.len() >= MAX_SENDS {
            t.suspended = true;
            out.push(Output::Notice(format!(
                "Trigger \"{}\" fired more than {MAX_SENDS} times in {} seconds, so it is off until you reconnect or edit your triggers.",
                t.label(),
                SEND_WINDOW.as_secs()
            )));
            return false;
        }
        t.recent_sends.push_back(now);
        true
    }

    fn send_raw(&self, line: &str) -> bool {
        self.handle.as_ref().is_some_and(|h| h.send_line(line))
    }

    fn log(&mut self, text: &str, out: &mut Vec<Output>) {
        if let Some(log) = self.logger.as_mut()
            && let Err(e) = log.line(text)
        {
            self.logger = None;
            out.push(Output::Notice(format!("Logging stopped: {e}")));
            out.push(Output::LoggingChanged(None));
        }
    }

    /// Starts the character's autolog when it connects, unless a log is already running.
    fn start_autolog(&mut self, out: &mut Vec<Output>) {
        let Some(character) = self.character.as_ref().filter(|c| !c.log_file.trim().is_empty()) else { return };
        if self.logger.is_some() {
            return;
        }
        let path = familiar_config::log_path(&character.log_file);
        match Logger::open_path(&path, character.log_date, self.settings.log_timestamps) {
            Ok(log) => {
                let path = log.path().to_path_buf();
                self.logger = Some(log);
                out.push(Output::LoggingChanged(Some(path.clone())));
                out.push(Output::Notice(format!("Logging to {}", path.display())));
            }
            Err(e) => out.push(Output::Notice(format!("Couldn't start logging to {}: {e}", path.display()))),
        }
    }

    fn restore_path(&self) -> Option<PathBuf> {
        let character = self.character.as_ref().filter(|c| c.restore_log && !c.name.is_empty())?;
        Some(familiar_config::restore_log_path(&self.world.name, &character.name))
    }

    /// Shows the end of the character's last sessions, if it keeps a restore log.
    fn replay_restore(&mut self, out: &mut Vec<Output>) {
        let Some(path) = self.restore_path() else { return };
        let entries = match RestoreLog::read(&path) {
            Ok(entries) => entries,
            Err(e) => {
                out.push(Output::Notice(format!("Couldn't read the restore log: {e}")));
                return;
            }
        };
        if entries.is_empty() {
            return;
        }
        let mut ansi = AnsiParser::new();
        for entry in entries {
            match entry {
                restore::Entry::Start(time) => {
                    ansi = AnsiParser::new();
                    out.push(Output::Notice(format!("--- Restored from the session of {} ---", time.format("%Y-%m-%d %H:%M"))));
                }
                restore::Entry::Line(raw) => {
                    let mut line = ansi.parse_line(&raw);
                    line.linkify();
                    out.push(Output::Line { line, target: Target::Main });
                }
            }
        }
        out.push(Output::Notice("--- End of the restored log ---".into()));
    }

    /// Starts recording the restore log when the character connects.
    fn start_restore(&mut self, out: &mut Vec<Output>) {
        if self.restore.is_some() {
            return;
        }
        let Some(path) = self.restore_path() else { return };
        match RestoreLog::open(&path) {
            Ok(log) => self.restore = Some(log),
            Err(e) => out.push(Output::Notice(format!("Couldn't open the restore log: {e}"))),
        }
    }

    fn record_restore(&mut self, raw: &str, out: &mut Vec<Output>) {
        if let Some(log) = self.restore.as_mut()
            && let Err(e) = log.line(raw)
        {
            self.restore = None;
            out.push(Output::Notice(format!("Stopped keeping the restore log: {e}")));
        }
    }

    /// Starts logging to the default log folder, or stops it.
    pub fn toggle_log(&mut self) -> Vec<Output> {
        if let Some(log) = self.logger.take() {
            let mut out = vec![Output::LoggingChanged(None), Output::Notice("Logging stopped".into())];
            if let Err(e) = log.close() {
                out.push(Output::Notice(format!("Error closing the log: {e}")));
            }
            return out;
        }
        match Logger::open(&familiar_config::log_dir(), &self.title(), self.settings.log_timestamps) {
            Ok(log) => {
                let path = log.path().to_path_buf();
                self.logger = Some(log);
                vec![Output::LoggingChanged(Some(path.clone())), Output::Notice(format!("Logging to {}", path.display()))]
            }
            Err(e) => vec![Output::Notice(format!("Couldn't start logging: {e}"))],
        }
    }
}

struct Delay {
    id: String,
    due: Instant,
    every: Option<std::time::Duration>,
    command: String,
}

mod agent;
mod commands;
mod incoming;
mod map;
mod puppets;
mod scripting;
#[cfg(test)]
mod tests;

pub use commands::{AGENT_COMMAND, COMMANDS};
