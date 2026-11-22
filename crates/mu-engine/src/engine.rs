//! The per-connection engine. It turns network events and user input into [`Output`]s for a UI to
//! render, and owns everything that doesn't depend on the UI toolkit: ANSI parsing, triggers,
//! aliases, macros, variables, client commands and logging.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use mu_ansi::{AnsiParser, Color, StyledLine};
use mu_config::{Character, Config, Highlight, Scope, Settings, World};
use mu_net::{ConnectOptions, Encoding, SessionEvent, SessionHandle, SessionOptions, spawn_session};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::expand::{Captures, expand};
use crate::rules::CompiledRules;
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
    /// Variables at `scope` changed and should be saved.
    VariablesChanged { scope: Scope, variables: BTreeMap<String, String> },
    LoggingChanged(Option<PathBuf>),
}

pub struct Engine {
    world: World,
    character: Option<Character>,
    settings: Settings,
    /// Most specific first. Index 0 is where `/set` stores variables.
    scopes: Vec<Scope>,
    variables: Vec<BTreeMap<String, String>>,
    rules: CompiledRules,
    ansi: AnsiParser,
    logger: Option<Logger>,
    handle: Option<SessionHandle>,
    window_size: Option<(u16, u16)>,
    server_echo: bool,
}

const HELP: &str = "Client commands:
  /set [name [value]]  show variables, or set one (use as %name% in triggers and aliases)
  /unset name          remove a variable
  /echo text           show text locally
  /log                 start or stop logging to a file
  /clear               clear the output
  /reconnect, /disconnect
  //text               send text that starts with a /";

impl Engine {
    pub fn new(config: &Config, world: World, character: Option<Character>) -> (Self, Vec<Output>) {
        let mut engine = Self {
            settings: config.settings.clone(),
            world,
            character,
            scopes: Vec::new(),
            variables: Vec::new(),
            rules: CompiledRules::default(),
            ansi: AnsiParser::new(),
            logger: None,
            handle: None,
            window_size: None,
            server_echo: false,
        };
        let out = engine.reload(config);
        (engine, out)
    }

    /// Picks up changed rules, variables and settings. Returns notices for rules that failed to
    /// compile.
    pub fn reload(&mut self, config: &Config) -> Vec<Output> {
        self.settings = config.settings.clone();
        let character = self.character.as_ref().map(|c| c.name.as_str());
        self.scopes = config.scopes_for(&self.world.name, character);
        let layers: Vec<_> = self.scopes.iter().filter_map(|s| config.rules(s)).collect();
        self.variables = layers.iter().map(|r| r.variables.clone()).collect();
        let (rules, errors) = CompiledRules::compile(layers);
        self.rules = rules;
        errors.into_iter().map(Output::Notice).collect()
    }

    pub fn title(&self) -> String {
        match &self.character {
            Some(c) if !c.name.is_empty() => format!("{} @ {}", c.name, self.world.name),
            _ if self.world.name.is_empty() => format!("{}:{}", self.world.host, self.world.port),
            _ => self.world.name.clone(),
        }
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
        opts.telnet.client_name = "MU Client".into();

        let (handle, events) = spawn_session(opts);
        if let Some((w, h)) = self.window_size {
            handle.set_window_size(w, h);
        }
        self.handle = Some(handle);
        self.ansi = AnsiParser::new();
        let out = vec![
            Output::Status(Status::Connecting),
            Output::Notice(format!(
                "Connecting to {}:{}{}...",
                self.world.host,
                self.world.port,
                if self.world.tls { " with TLS" } else { "" }
            )),
        ];
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

    pub fn handle_event(&mut self, event: SessionEvent) -> Vec<Output> {
        let mut out = Vec::new();
        match event {
            SessionEvent::Connected { addr } => {
                out.push(Output::Status(Status::Connected));
                out.push(Output::Notice(format!("Connected to {addr}")));
                let login = self.character.as_ref().map(|c| c.login.clone()).unwrap_or_default();
                for line in login.lines().filter(|l| !l.is_empty()) {
                    self.send_raw(line);
                }
            }
            SessionEvent::Line(text) => self.incoming(&text, None, &mut out),
            SessionEvent::Prompt(text) => self.incoming(&text, Some(true), &mut out),
            SessionEvent::PartialLine(text) => {
                // Parse with a copy of the ANSI state: the full line will be parsed again later.
                let line = self.ansi.clone().parse_line(&text);
                out.push(Output::Prompt { line, terminated: false });
                out.push(Output::Activity { important: false });
            }
            SessionEvent::ServerEcho(on) => {
                self.server_echo = on;
                out.push(Output::PasswordMode(on));
            }
            SessionEvent::EncodingChanged(e) => out.push(Output::Notice(format!("Server switched encoding to {}", e.name()))),
            SessionEvent::Notice(text) => out.push(Output::Notice(text)),
            SessionEvent::Gmcp { .. } | SessionEvent::Mssp(_) | SessionEvent::Compression(_) => {}
            SessionEvent::Disconnected { error } => {
                let reason = match &error {
                    Some(e) => format!("Disconnected: {e}"),
                    None => "Disconnected".to_string(),
                };
                out.push(Output::Notice(format!("{reason}. Press Ctrl+R or type /reconnect to reconnect.")));
                self.log(&format!("--- {reason} ---"), &mut out);
                out.push(Output::Status(Status::Disconnected(error)));
                if self.server_echo {
                    self.server_echo = false;
                    out.push(Output::PasswordMode(false));
                }
                self.handle = None;
            }
        }
        out
    }

    /// Handles a line (prompt = None) or prompt (prompt = Some(terminated)) from the server.
    fn incoming(&mut self, text: &str, prompt: Option<bool>, out: &mut Vec<Output>) {
        let mut line = self.ansi.parse_line(text);
        line.linkify();
        let plain = line.text.clone();

        let mut gag = false;
        let mut gag_log = false;
        let mut important = false;
        let mut spawns: Vec<String> = Vec::new();
        let mut sends: Vec<String> = Vec::new();

        for i in 0..self.rules.triggers.len() {
            let t = &self.rules.triggers[i];
            if t.suspended {
                continue;
            }
            let Ok(Some(caps)) = t.regex.captures(plain.as_str()) else { continue };
            let trigger = &t.trigger;
            if let Some(h) = &trigger.highlight {
                if h.whole_line {
                    line.restyle(0, plain.len(), |s| apply_highlight(s, h));
                } else {
                    for m in t.regex.find_iter(plain.as_str()).flatten() {
                        line.restyle(m.start(), m.end(), |s| apply_highlight(s, h));
                    }
                }
            }
            gag |= trigger.gag;
            gag_log |= trigger.gag_log;
            important |= trigger.important;
            if !trigger.spawn.is_empty() && !spawns.contains(&trigger.spawn) {
                spawns.push(trigger.spawn.clone());
            }
            if !trigger.sound.is_empty() {
                out.push(Output::PlaySound(trigger.sound.clone()));
            }
            if trigger.notify {
                out.push(Output::Notify { title: self.title(), body: plain.clone() });
            }
            let stop = trigger.stop;
            if !trigger.send.is_empty() {
                let caps = Captures::from_regex(&t.regex, &caps);
                let template = trigger.send.clone();
                if self.note_trigger_send(i, out) {
                    let text = expand(&template, &caps, |name| self.variable(name));
                    sends.extend(text.lines().map(str::to_string));
                }
            }
            if stop {
                break;
            }
        }

        if !gag_log {
            self.log(&plain, out);
        }
        if line.bell {
            out.push(Output::Bell);
        }
        if !gag {
            match prompt {
                Some(terminated) => out.push(Output::Prompt { line: line.clone(), terminated }),
                None => out.push(Output::Line { line: line.clone(), target: Target::Main }),
            }
        }
        for name in spawns {
            out.push(Output::Line { line: line.clone(), target: Target::Spawn(name) });
        }
        out.push(Output::Activity { important });
        for command in sends {
            self.send_raw(&command);
            if self.settings.local_echo {
                out.push(Output::Echo(command));
            }
        }
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

    /// Handles text the user entered. `secret` is for password entry: sent as is, never echoed,
    /// expanded or logged.
    pub fn submit(&mut self, text: &str, secret: bool) -> Vec<Output> {
        let mut out = Vec::new();
        if !self.is_connected() && !text.starts_with('/') {
            out.push(Output::Notice("Not connected. Press Ctrl+R or type /reconnect to reconnect.".into()));
            return out;
        }
        if secret {
            self.send_raw(text);
            return out;
        }
        // Enter on an empty input still sends an empty line, which many MUDs use.
        let lines: Vec<&str> = if text.is_empty() { vec![""] } else { text.lines().collect() };
        for line in lines {
            if let Some(literal) = line.strip_prefix("//") {
                self.send_command(&format!("/{literal}"), &mut out);
            } else if let Some(command) = line.strip_prefix('/') {
                self.client_command(command, &mut out);
            } else {
                for command in self.expand_aliases(line) {
                    self.send_command(&command, &mut out);
                }
            }
        }
        out
    }

    fn send_command(&mut self, command: &str, out: &mut Vec<Output>) {
        if self.send_raw(command) {
            if self.settings.local_echo {
                out.push(Output::Echo(command.to_string()));
            }
        } else {
            out.push(Output::Notice("Not connected. Press Ctrl+R or type /reconnect to reconnect.".into()));
        }
    }

    /// Applies the first matching alias. Results aren't expanded again, so aliases can't loop.
    pub fn expand_aliases(&self, line: &str) -> Vec<String> {
        for alias in &self.rules.aliases {
            if let Some(caps) = alias.matches(line) {
                let text = expand(&alias.alias.replacement, &caps, |name| self.variable(name));
                return text.lines().map(str::to_string).collect();
            }
        }
        vec![line.to_string()]
    }

    /// The command bound to a key, if any. The UI should pass it to [`Engine::submit`].
    pub fn macro_for(&self, chord: &KeyChord) -> Option<String> {
        self.rules.macro_for(chord).map(|m| m.command.clone())
    }

    fn client_command(&mut self, command: &str, out: &mut Vec<Output>) {
        let (name, args) = command.split_once(char::is_whitespace).unwrap_or((command, ""));
        let args = args.trim();
        match name.to_lowercase().as_str() {
            "help" | "?" => out.push(Output::Notice(HELP.into())),
            "set" if args.is_empty() => {
                let mut all: BTreeMap<&str, &str> = BTreeMap::new();
                for vars in self.variables.iter().rev() {
                    all.extend(vars.iter().map(|(k, v)| (k.as_str(), v.as_str())));
                }
                if all.is_empty() {
                    out.push(Output::Notice("No variables. Set one with /set name value".into()));
                }
                for (k, v) in all {
                    out.push(Output::Notice(format!("%{k}% = {v}")));
                }
            }
            "set" => {
                let (var, value) = args.split_once(char::is_whitespace).unwrap_or((args, ""));
                if !var.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    out.push(Output::Notice("Variable names can only contain letters, digits and _".into()));
                    return;
                }
                self.set_variable(var, Some(value.trim_start()), out);
            }
            "unset" if !args.is_empty() => self.set_variable(args, None, out),
            "echo" => out.push(Output::Notice(args.to_string())),
            "clear" => out.push(Output::ClearOutput),
            "log" => out.extend(self.toggle_log()),
            "reconnect" | "connect" => {
                if self.is_connected() {
                    out.push(Output::Notice("Already connected.".into()));
                } else {
                    out.push(Output::Reconnect);
                }
            }
            "disconnect" | "quit" => self.disconnect(),
            _ => out.push(Output::Notice(format!(
                "Unknown command /{name}. Type /help for client commands, or start the line with // to send a /."
            ))),
        }
    }

    fn set_variable(&mut self, name: &str, value: Option<&str>, out: &mut Vec<Output>) {
        let Some(vars) = self.variables.first_mut() else { return };
        match value {
            Some(value) => {
                vars.insert(name.to_string(), value.to_string());
                out.push(Output::Notice(format!("%{name}% = {value}")));
            }
            None => {
                if vars.remove(name).is_none() {
                    out.push(Output::Notice(format!("%{name}% isn't set here")));
                    return;
                }
                out.push(Output::Notice(format!("%{name}% removed")));
            }
        }
        out.push(Output::VariablesChanged { scope: self.scopes[0].clone(), variables: vars.clone() });
    }

    /// Handles a clicked link: `send:` sends a command, `prompt:` fills the input, anything
    /// else is opened by the UI.
    pub fn follow_link(&mut self, uri: &str) -> Vec<Output> {
        let mut out = Vec::new();
        if let Some(command) = uri.strip_prefix("send:") {
            self.send_command(&percent_decode(command), &mut out);
        } else if let Some(text) = uri.strip_prefix("prompt:") {
            out.push(Output::SetInput(percent_decode(text)));
        } else {
            out.push(Output::OpenUri(uri.to_string()));
        }
        out
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

    /// Starts logging to the default log folder, or stops it.
    pub fn toggle_log(&mut self) -> Vec<Output> {
        if let Some(log) = self.logger.take() {
            let mut out = vec![Output::LoggingChanged(None), Output::Notice("Logging stopped".into())];
            if let Err(e) = log.close() {
                out.push(Output::Notice(format!("Error closing the log: {e}")));
            }
            return out;
        }
        match Logger::open(&mu_config::log_dir(), &self.title(), self.settings.log_timestamps) {
            Ok(log) => {
                let path = log.path().to_path_buf();
                self.logger = Some(log);
                vec![Output::LoggingChanged(Some(path.clone())), Output::Notice(format!("Logging to {}", path.display()))]
            }
            Err(e) => vec![Output::Notice(format!("Couldn't start logging: {e}"))],
        }
    }
}

fn parse_hex(color: &str) -> Option<Color> {
    let hex = color.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some(Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

fn apply_highlight(style: &mut mu_ansi::Style, h: &Highlight) {
    if let Some(fg) = h.fg.as_deref().and_then(parse_hex) {
        style.fg = fg;
    }
    if let Some(bg) = h.bg.as_deref().and_then(parse_hex) {
        style.bg = bg;
    }
    style.bold |= h.bold;
    style.italic |= h.italic;
    style.underline |= h.underline;
}

/// Decodes %XX escapes. Invalid escapes are kept as they are.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = s.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests;
