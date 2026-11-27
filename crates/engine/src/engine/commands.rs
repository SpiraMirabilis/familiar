//! What the user sends: input, aliases, macros, and the client commands (`/set`, `/log`, ...).

use super::*;

/// Client commands, for help and for completion in frontends: (usage, description).
pub const COMMANDS: &[(&str, &str)] = &[
    ("/set [<name> [<value>]]", "show variables, or set one (use as %name% in triggers and aliases)"),
    ("/unset <name>", "remove a variable"),
    ("/delay [every] <time> <command>", "run a command later (or repeatedly); /delay list, kill ID, killall"),
    ("/echo <text>", "show text locally"),
    ("/log", "start or stop logging to a file"),
    ("/clear", "clear the output"),
    ("/reconnect", "connect again"),
    ("/disconnect", "close the connection (through a bouncer: detach, leaving the character online)"),
    ("/bnc [list|reconnect|close]", "bouncer: list its sessions, reconnect it to the world, or log off and forget this session"),
    ("/gmcp [<Package> <json>]", "show or hide GMCP traffic, or send a GMCP message"),
    ("/mcp [debug]", "show MCP status, or show and hide MCP traffic"),
    ("/webview <url>", "open a web page in a web view"),
    ("/@<code>", "run JavaScript (multi-line with Shift+Enter)"),
    ("/script load <path>", "run a script file"),
    ("/script reload", "restart scripting and reload the startup scripts"),
    ("/script <code>", "run JavaScript (like /@)"),
    ("/edit [<language>]", "open an editor whose text is sent line by line (moo, muf, lua)"),
    ("/map [go|here|addroom|import|track|clear] ...", "show the map, walk to a room, add rooms, import a BeipMU map"),
    ("/images on|off", "show thumbnails under links to images"),
    ("/puppet [<name>]", "list this character's puppets, or open one's tab"),
    ("/speech on|off|stop", "speak incoming lines aloud"),
    ("/speak <text>", "say something aloud"),
    ("/help", "list client commands"),
];

impl Engine {
    /// Handles text the user entered. `secret` is for password entry: sent as is, never echoed,
    /// expanded or logged.
    pub fn submit(&mut self, text: &str, secret: bool) -> Vec<Output> {
        let mut out = Vec::new();
        if !self.is_connected() && !text.starts_with('/') {
            out.push(Output::Notice(format!("Not connected. {}", self.reconnect_hint())));
            return out;
        }
        if secret {
            self.send_raw(text);
            return out;
        }
        // "/@" runs the whole input as JavaScript, so functions can span lines.
        if let Some(code) = text.strip_prefix("/@") {
            self.eval_script(code, &mut out);
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
                    // An alias can expand to client commands (BeipMU aliases often run /script).
                    match command.strip_prefix('/') {
                        Some(client) if !client.starts_with('/') => self.client_command(client, &mut out),
                        Some(literal) => self.send_command(literal, &mut out),
                        None => self.send_command(&command, &mut out),
                    }
                }
            }
        }
        out
    }

    pub(super) fn send_command(&mut self, command: &str, out: &mut Vec<Output>) {
        let changed;
        let mut command = command;
        if let Some(script) = self.script.as_mut() {
            let result = script.on_send(command);
            self.drain_script(out);
            match result {
                Some(c) => {
                    changed = c;
                    command = &changed;
                }
                None => return, // A script dropped it.
            }
        }
        // Remember movement, for the automapper and live tracking.
        if familiar_map::offset(command).is_some() || self.current_room.and_then(|r| self.map.room(r)).is_some_and(|r| r.exit(command).is_some()) {
            self.last_move = Some(command.to_string());
        }
        if self.send_raw(command) {
            out.push(Output::Sent(command.to_string()));
            if self.settings.local_echo {
                out.push(Output::Echo(command.to_string()));
            }
        } else {
            out.push(Output::Notice(format!("Not connected. {}", self.reconnect_hint())));
        }
    }

    /// Applies aliases. Substituting aliases edit the line in place and processing carries on;
    /// the first other alias that matches replaces the whole line and ends it. Results aren't
    /// expanded again, so aliases can't loop.
    pub fn expand_aliases(&self, line: &str) -> Vec<String> {
        let mut text = line.to_string();
        for alias in &self.rules.aliases {
            if alias.alias.substitute {
                if let Some(changed) = alias.substitute(&text, |name| self.variable(name)) {
                    text = changed;
                    if alias.alias.stop {
                        break;
                    }
                }
            } else if let Some(caps) = alias.matches(&text) {
                text = expand(&alias.alias.replacement, &caps, |name| self.variable(name));
                break;
            }
        }
        if text.is_empty() {
            return vec![text];
        }
        text.lines().map(str::to_string).collect()
    }

    /// Runs the macro bound to a key. None if there isn't one.
    pub fn run_macro(&mut self, chord: &KeyChord) -> Option<Vec<Output>> {
        let m = self.rules.macro_for(chord)?.clone();
        Some(if m.insert { vec![Output::SetInput(m.command)] } else { self.submit(&m.command, false) })
    }

    pub(super) fn client_command(&mut self, command: &str, out: &mut Vec<Output>) {
        if let Some(code) = command.strip_prefix('@') {
            self.eval_script(code, out);
            return;
        }
        let (name, args) = command.split_once(char::is_whitespace).unwrap_or((command, ""));
        let args = args.trim();
        match name.to_lowercase().as_str() {
            "help" | "?" => out.push(Output::Notice(help_text(self.agents_supported))),
            "agent" | "agents" if self.agents_supported => out.push(Output::Agent(args.to_string())),
            "set" if args.is_empty() => {
                let all = self.merged_variables();
                if all.is_empty() {
                    out.push(Output::Notice("No variables. Set one with /set name value".into()));
                }
                for (k, v) in all {
                    out.push(Output::Notice(format!("%{k}% = {v}")));
                }
            }
            "set" => match args.split_once(char::is_whitespace) {
                Some((var, value)) => {
                    if !var.chars().all(|c| c.is_alphanumeric() || c == '_') {
                        out.push(Output::Notice("Variable names can only contain letters, digits and _".into()));
                        return;
                    }
                    self.set_variable(var, Some(value.trim_start()), false, out);
                }
                None => match self.variable(args) {
                    Some(v) => out.push(Output::Notice(format!("%{args}% = {v}"))),
                    None => out.push(Output::Notice(format!("%{args}% is not set"))),
                },
            },
            "unset" if !args.is_empty() => self.set_variable(args, None, false, out),
            "script" => {
                let (sub, rest) = args.split_once(char::is_whitespace).unwrap_or((args, ""));
                match sub {
                    "load" if !rest.trim().is_empty() => out.extend(self.load_script(std::path::Path::new(rest.trim()))),
                    "reload" => out.push(Output::ReloadScripts),
                    // BeipMU's form of /@: /script code.
                    _ if !args.is_empty() => self.eval_script(args, out),
                    _ => out.push(Output::Notice(format!(
                        "Scripting is {}. Use /@code, /script load path or /script reload.",
                        if self.script.is_some() { "on" } else { "off" }
                    ))),
                }
            }
            "echo" => out.push(Output::Notice(args.to_string())),
            "delay" => self.delay_command(args, out),
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
            "bnc" | "bouncer" => {
                let request = match args.to_ascii_lowercase().as_str() {
                    "" | "list" => familiar_net::BouncerRequest::List,
                    "reconnect" => familiar_net::BouncerRequest::Reconnect,
                    "close" => familiar_net::BouncerRequest::Close,
                    _ => {
                        out.push(Output::Notice("Usage: /bnc [list|reconnect|close]".into()));
                        return;
                    }
                };
                match &self.handle {
                    Some(handle) if self.via_bouncer => {
                        handle.bouncer(request);
                    }
                    _ if !self.world.bouncer => out.push(Output::Notice("This world doesn't use the bouncer. Turn on bouncer for it in the world's settings.".into())),
                    _ if self.bouncer.is_unset() => out.push(Output::Notice("No bouncer is set up. Add a [bouncer] section to config.toml.".into())),
                    _ => out.push(Output::Notice("Not connected to the bouncer. Type /reconnect first.".into())),
                }
            }
            "gmcp" if args.is_empty() => {
                self.gmcp_debug = !self.gmcp_debug;
                out.push(Output::Notice(format!("GMCP traffic is {}", if self.gmcp_debug { "shown" } else { "hidden" })));
            }
            "gmcp" => {
                let (package, data) = args.split_once(char::is_whitespace).unwrap_or((args, ""));
                let data = data.trim();
                out.extend(self.send_gmcp(package, (!data.is_empty()).then_some(data)));
            }
            "mcp" if args == "debug" => {
                self.mcp_debug = !self.mcp_debug;
                out.push(Output::Notice(format!("MCP traffic is {}", if self.mcp_debug { "shown" } else { "hidden" })));
            }
            "mcp" => out.push(Output::Notice(if !self.world.mcp {
                "MCP is off for this world (mcp = false in its config)".into()
            } else if self.mcp.server_confirmed() {
                "MCP 2.1 is active".into()
            } else if self.mcp.is_active() {
                "The server started MCP and the client answered with its key, but the server hasn't replied. \
                 If it's a MOO built with OUT_OF_BAND_PREFIX, the answer went to #0:do_out_of_band_command. \
                 /mcp debug shows the traffic."
                    .into()
            } else {
                "MCP is on, but the server hasn't started it (no #$#mcp line yet)".into()
            })),
            "map" => self.map_command(args, out),
            "puppet" | "puppets" if args.is_empty() => {
                let names: Vec<String> = self.puppets().map(|p| p.name.clone()).collect();
                out.push(Output::Notice(if names.is_empty() {
                    "This character has no puppets. Add them in the Connect dialog (edit the world).".into()
                } else {
                    format!("Puppets: {}. /puppet name opens one.", names.join(", "))
                }));
            }
            "puppet" => {
                let found = self.puppets().find(|p| p.name.eq_ignore_ascii_case(args)).map(|p| p.name.clone());
                match found {
                    Some(name) => self.open_puppet(&name, true, out),
                    None => out.push(Output::Notice(format!("There's no puppet called {args}"))),
                }
            }
            "speak" if !args.is_empty() => out.push(Output::Speak(args.to_string())),
            "speech" => match args {
                "on" | "off" => {
                    self.settings.speak_output = args == "on";
                    out.push(Output::SettingsChanged(self.settings.clone()));
                    out.push(Output::Notice(format!("Speaking incoming lines is {args}")));
                }
                "stop" => out.push(Output::SpeechStop),
                _ => out.push(Output::Notice("/speech on|off speaks every incoming line; /speech stop stops; /speak text says something.".into())),
            },
            "images" => {
                match args {
                    "on" | "off" => {
                        self.settings.inline_images = args == "on";
                        out.push(Output::SettingsChanged(self.settings.clone()));
                    }
                    _ => {}
                }
                out.push(Output::Notice(if self.settings.inline_images {
                    "Inline images are on: links to images show a thumbnail. Fetching an image tells its site your IP address. /images off turns them off.".into()
                } else {
                    "Inline images are off. /images on shows thumbnails under links to images.".into()
                }));
            }
            "edit" => {
                let language = if args.is_empty() { self.program_language() } else { args.to_lowercase() };
                out.push(Output::Edit(EditRequest {
                    reference: String::new(),
                    name: format!("Scratch ({language})"),
                    kind: "local".into(),
                    content: String::new(),
                    language: Some(language),
                }));
            }
            "webview" if !args.is_empty() => {
                // Accept BeipMU's form, /webview url="...", as well as a bare URL.
                let url = args.strip_prefix("url=").unwrap_or(args).trim_matches('"').to_string();
                out.push(Output::WebView(WebViewCommand::Open(WebViewOpen {
                    id: url.clone(),
                    url: Some(url),
                    source: None,
                    dock: Some("right".into()),
                    width: None,
                    height: None,
                    headers: Vec::new(),
                    from_server: false,
                })));
            }
            _ => out.push(Output::Notice(format!(
                "Unknown command /{name}. Type /help for client commands, or start the line with // to send a /."
            ))),
        }
    }

    /// Sets or removes a variable at the most specific scope. `quiet` skips the notice (for
    /// scripts).
    pub(super) fn set_variable(&mut self, name: &str, value: Option<&str>, quiet: bool, out: &mut Vec<Output>) {
        let Some(vars) = self.variables.first_mut() else { return };
        match value {
            Some(value) => {
                vars.insert(name.to_string(), value.to_string());
                if !quiet {
                    out.push(Output::Notice(format!("%{name}% = {value}")));
                }
            }
            None => {
                if vars.remove(name).is_none() {
                    if !quiet {
                        out.push(Output::Notice(format!("%{name}% isn't set here")));
                    }
                    return;
                }
                if !quiet {
                    out.push(Output::Notice(format!("%{name}% removed")));
                }
            }
        }
        out.push(Output::VariablesChanged { scope: self.scopes[0].clone(), variables: vars.clone() });
        let merged = self.merged_variables();
        if let Some(script) = self.script.as_mut() {
            script.set_variables(&merged);
        }
    }

    /// Handles a clicked link: `send:` sends a command, `prompt:` fills the input, anything
    /// else is opened by the UI (restricted to safe URL schemes).
    pub fn follow_link(&mut self, uri: &str) -> Vec<Output> {
        let mut out = Vec::new();
        if let Some(command) = uri.strip_prefix("send:") {
            let decoded = familiar_config::paths::percent_decode(command).replace(['\r', '\n'], "");
            self.send_command(&decoded, &mut out);
        } else if let Some(text) = uri.strip_prefix("prompt:") {
            out.push(Output::SetInput(familiar_config::paths::percent_decode(text)));
        } else if crate::media::is_safe_open_uri(uri) {
            out.push(Output::OpenUri(uri.to_string()));
        } else {
            out.push(Output::Notice(format!("Refused to open unsafe link: {uri}")));
        }
        out
    }
}

/// `/agent`, for frontends that can run agents ([`Engine::set_agents_supported`]).
pub const AGENT_COMMAND: (&str, &str) = ("/agent [<name>|stop|list]", "run an LLM agent (claude, codex, agy, grok) beside the world, with the world's tools");

pub(super) fn help_text(agents: bool) -> String {
    let mut text = String::from("Client commands:");
    for (usage, description) in COMMANDS.iter().chain(agents.then_some(&AGENT_COMMAND)) {
        text.push_str(&format!("\n  {usage:<34} {description}"));
    }
    text.push_str("\n  //<text>                           send text that starts with a /");
    text
}
