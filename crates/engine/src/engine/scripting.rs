//! Scripting and timers: the script host, `/delay`, and running what scripts ask for.

use super::*;

impl Engine {
    pub(super) fn script_context(&self) -> ScriptContext {
        ScriptContext {
            world: self.world.name.clone(),
            host: self.world.host.clone(),
            port: self.world.port,
            character: self.character.as_ref().map(|c| c.name.clone()),
            connected: self.is_connected(),
        }
    }

    /// Installs (or replaces) the script host. The caller loads the startup scripts after this.
    pub fn set_script_host(&mut self, mut host: Box<dyn ScriptHost>) -> Vec<Output> {
        host.set_context(&self.script_context());
        host.set_variables(&self.merged_variables());
        self.script = Some(host);
        let mut out = Vec::new();
        self.drain_script(&mut out);
        out
    }

    pub fn has_scripting(&self) -> bool {
        self.script.is_some()
    }

    /// Runs a script file in this session.
    pub fn load_script(&mut self, path: &std::path::Path) -> Vec<Output> {
        let mut out = Vec::new();
        let Some(script) = self.script.as_mut() else {
            out.push(Output::Notice("Scripting isn't available".into()));
            return out;
        };
        if let Err(e) = script.load_file(path) {
            out.push(Output::Notice(format!("Script error in {}: {e}", path.display())));
        }
        self.drain_script(&mut out);
        out
    }

    /// Runs timers that are due. The UI calls this at [`Engine::next_deadline`].
    pub fn tick(&mut self, now: Instant) -> Vec<Output> {
        let mut out = Vec::new();
        if let Some(script) = self.script.as_mut() {
            script.tick(now);
            self.drain_script(&mut out);
        }
        let mut due = Vec::new();
        self.delays.retain_mut(|d| {
            if d.due > now {
                return true;
            }
            due.push(d.command.clone());
            match d.every {
                Some(every) => {
                    d.due = now + every;
                    true
                }
                None => false,
            }
        });
        for command in due {
            out.extend(self.submit(&command, false));
        }
        self.agent_timeouts(now, &mut out);
        out
    }

    pub fn next_deadline(&mut self) -> Option<Instant> {
        let script = self.script.as_mut().and_then(|s| s.next_deadline());
        let delay = self.delays.iter().map(|d| d.due).min();
        script.into_iter().chain(delay).chain(self.agent_deadline()).min()
    }

    /// `/delay [id NAME] [every] TIME COMMAND`, `/delay list`, `/delay kill NAME`,
    /// `/delay killall`. TIME is like 5s, 1.5m, 2h, 500ms or 1m30s.
    pub(super) fn delay_command(&mut self, args: &str, out: &mut Vec<Output>) {
        let mut words = args.split_whitespace();
        match words.next().map(str::to_ascii_lowercase).as_deref() {
            None | Some("list") => {
                if self.delays.is_empty() {
                    out.push(Output::Notice("No delayed commands. Add one with /delay 5s command".into()));
                }
                let now = Instant::now();
                for d in &self.delays {
                    let left = d.due.saturating_duration_since(now).as_secs_f64();
                    let every = d.every.map(|e| format!(", every {}s", e.as_secs_f64())).unwrap_or_default();
                    out.push(Output::Notice(format!("[{}] in {left:.1}s{every}: {}", d.id, d.command)));
                }
                return;
            }
            Some("killall") => {
                self.delays.clear();
                out.push(Output::Notice("All delayed commands cancelled".into()));
                return;
            }
            Some("kill") => {
                let id = words.next().unwrap_or("");
                let before = self.delays.len();
                self.delays.retain(|d| !d.id.eq_ignore_ascii_case(id));
                let message = if self.delays.len() < before { format!("Cancelled {id}") } else { format!("No delayed command called {id}") };
                out.push(Output::Notice(message));
                return;
            }
            _ => {}
        }
        let mut rest = args.trim_start();
        let mut word = || {
            let (w, r) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            rest = r.trim_start();
            w.to_string()
        };
        let mut id = None;
        let mut every = false;
        let mut time = word();
        if time.eq_ignore_ascii_case("id") {
            id = Some(word());
            time = word();
        }
        if time.eq_ignore_ascii_case("every") {
            every = true;
            time = word();
        }
        let command = rest.trim();
        // BeipMU quotes the command: /delay 3s "connect bob pw".
        let command = command.strip_prefix('"').and_then(|c| c.strip_suffix('"')).unwrap_or(command).replace("\\\"", "\"");
        let Some(wait) = parse_duration(&time) else {
            out.push(Output::Notice("Usage: /delay [id NAME] [every] TIME COMMAND, with TIME like 5s, 2m or 500ms. Also /delay list, /delay kill NAME, /delay killall".into()));
            return;
        };
        if command.is_empty() {
            out.push(Output::Notice("/delay needs a command to run".into()));
            return;
        }
        // A repeating delay of less than 100 ms would flood the server.
        let every = every.then(|| wait.max(std::time::Duration::from_millis(100)));
        let id = id.unwrap_or_else(|| {
            let id = self.next_delay_id.to_string();
            self.next_delay_id += 1;
            id
        });
        self.delays.retain(|d| d.id != id);
        self.delays.push(Delay { id, due: Instant::now() + wait, every, command });
    }

    /// Applies what scripts asked for since the last hook.
    pub(super) fn drain_script(&mut self, out: &mut Vec<Output>) {
        let Some(actions) = self.script.as_mut().map(|s| s.take_actions()) else { return };
        if actions.is_empty() {
            return;
        }
        if self.script_depth >= MAX_SCRIPT_DEPTH {
            out.push(Output::Notice("Script actions are nested too deeply (a script loop?); some were skipped.".into()));
            return;
        }
        self.script_depth += 1;
        for action in actions {
            match action {
                ScriptAction::Send(text) => {
                    for line in text.lines() {
                        if self.send_raw(line) {
                            out.push(Output::Sent(line.to_string()));
                            if self.settings.local_echo {
                                out.push(Output::Echo(line.to_string()));
                            }
                        }
                    }
                }
                ScriptAction::Input(text) => out.extend(self.submit(&text, false)),
                ScriptAction::Notice(text) => out.push(Output::Notice(text)),
                ScriptAction::Display(text) => {
                    let mut line = AnsiParser::new().parse_line(&text);
                    line.linkify();
                    out.push(Output::Line { line, target: Target::Main });
                }
                ScriptAction::Spawn { window, text } => {
                    let mut line = AnsiParser::new().parse_line(&text);
                    line.linkify();
                    out.push(Output::Line { line, target: Target::Spawn(window) });
                }
                ScriptAction::Gmcp { package, data } => out.extend(self.send_gmcp(&package, data.as_deref())),
                ScriptAction::SetVariable { name, value } => self.set_variable(&name, value.as_deref(), true, out),
                ScriptAction::Speak(text) => out.push(Output::Speak(text)),
                ScriptAction::Receive(text) => {
                    for line in text.lines() {
                        self.incoming(line, None, out);
                    }
                }
                ScriptAction::SetInput(text) => out.push(Output::SetInput(text)),
                ScriptAction::PlaySound(path) => out.push(Output::PlaySound(path)),
                ScriptAction::Error(e) => out.push(Output::Notice(format!("Script error: {e}"))),
            }
        }
        self.script_depth -= 1;
        // Applying actions can run more hooks, which can queue more actions.
        self.drain_script(out);
    }

    pub(super) fn script_event(&mut self, name: &str, out: &mut Vec<Output>) {
        let context = self.script_context();
        if let Some(script) = self.script.as_mut() {
            script.set_context(&context);
            script.on_event(name);
            self.drain_script(out);
        }
    }

    /// Scripts to run when scripting starts: the global startup.js, then the world's script.
    pub fn startup_scripts(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        let startup = familiar_config::startup_script_path();
        if startup.is_file() {
            paths.push(startup);
        }
        let script = self.world.script.trim();
        if !script.is_empty() {
            paths.push(familiar_config::paths::expand(script));
        }
        paths
    }

    pub(super) fn eval_script(&mut self, code: &str, out: &mut Vec<Output>) {
        let Some(script) = self.script.as_mut() else {
            out.push(Output::Notice("Scripting isn't available".into()));
            return;
        };
        let result = script.eval(code);
        self.drain_script(out);
        match result {
            Ok(Some(value)) => out.push(Output::Notice(value)),
            Ok(None) => {}
            Err(e) => out.push(Output::Notice(format!("Script error: {e}"))),
        }
    }
}

/// Parses "5", "5s", "1.5m", "2h", "500ms" or "1m30s".
pub(super) fn parse_duration(text: &str) -> Option<std::time::Duration> {
    let text = text.trim().to_ascii_lowercase();
    if text.is_empty() {
        return None;
    }
    if let Ok(seconds) = text.parse::<f64>() {
        return (seconds >= 0.0).then(|| std::time::Duration::from_secs_f64(seconds));
    }
    let mut total = 0.0;
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let digits = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
        let value: f64 = rest[..digits].parse().ok()?;
        rest = &rest[digits..];
        let unit = rest.find(|c: char| c.is_ascii_digit() || c == '.').unwrap_or(rest.len());
        total += value
            * match &rest[..unit] {
                "ms" => 0.001,
                "s" | "sec" | "secs" | "" => 1.0,
                "m" | "min" | "mins" => 60.0,
                "h" | "hr" | "hrs" => 3600.0,
                _ => return None,
            };
        rest = &rest[unit..];
    }
    Some(std::time::Duration::from_secs_f64(total))
}
