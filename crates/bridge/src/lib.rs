//! Familiar's engine behind a C interface, for the apps that aren't written in Rust: the macOS
//! app (Swift) and the Windows app (C#).
//!
//! There is one entry point, [`familiar_call`]: a method name and JSON arguments in, JSON out.
//! The engine isn't thread-safe and doesn't need to be: every call is made on the main thread,
//! as in the GTK client. Network traffic arrives on background threads; when a session has
//! some, the `wake` callback fires and the app calls `session.poll` from the main thread to
//! run it through the engine and collect what to show.
//!
//! Methods that may be called from any thread are marked "(any thread)" in [`call`].

mod json;
mod tty;
#[cfg(windows)]
mod windows;

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::ffi::{CStr, CString, c_char};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use familiar_agent::{AgentRequest, Agents};
use familiar_ansi::{AnsiParser, Palette};
use familiar_config::{Address, Config, World};
use familiar_engine::{Engine, KeyChord, Keymap, Output, Target, keymap};
use familiar_net::SessionEvent;
use serde_json::{Value, json};

/// The most network events handled by one `session.poll`, so a flood can't freeze the window.
const BATCH_LIMIT: usize = 500;

type WakeFn = extern "C" fn(u64);

static WAKE: OnceLock<WakeFn> = OnceLock::new();
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
static MAIN_THREAD: OnceLock<std::thread::ThreadId> = OnceLock::new();

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().thread_name("familiar-net").build().expect("tokio runtime")
    })
}

/// What a session's background tasks share with it.
struct Inbox {
    /// Network events, each with the connection (generation) it came from.
    events: Mutex<VecDeque<(u64, SessionEvent)>>,
    /// A wake has been sent and not yet answered by a poll.
    woken: AtomicBool,
    /// LLM agents' requests of this world, answered at the next poll.
    agent: Mutex<VecDeque<AgentRequest>>,
}

impl Inbox {
    fn wake(&self, id: u64) {
        if !self.woken.swap(true, Ordering::SeqCst)
            && let Some(wake) = WAKE.get()
        {
            wake(id);
        }
    }
}

struct Session {
    engine: Engine,
    /// Bumped on each connect, so events from an old connection are dropped.
    generation: u64,
    inbox: Arc<Inbox>,
    /// Outputs produced outside a call about this session (a config change), for the next poll.
    pending: Vec<Output>,
    /// The task that wakes us for the next script timer or /delay, and when.
    timer: Option<(tokio::task::JoinHandle<()>, Instant)>,
    /// The LLM agent working on this world: its name and endpoint token.
    agent: Option<(String, String)>,
    /// Whether we run agents in a terminal ourselves ("agents": "pty"), rather than the app.
    run_agents: bool,
    /// The agent's terminal, when we run it.
    tty: Option<tty::Tty>,
    /// The last lines of the world's window as plain text, for agents (world_output).
    recent: VecDeque<String>,
}

struct State {
    config: Config,
    config_path: PathBuf,
    /// Set when config.toml couldn't be read. Saving then would replace it with defaults.
    load_error: Option<String>,
    palette: Palette,
    sessions: HashMap<u64, Session>,
    next_id: u64,
    /// LLM agents: the MCP server and the calls waiting on worlds. Started by the first /agent.
    agents: Agents,
    /// The sessions' inboxes, where agents' requests are delivered (from the runtime).
    inboxes: Arc<Mutex<HashMap<u64, Arc<Inbox>>>>,
    /// Whether agents' programs are looked for in the login shell's PATH yet (macOS).
    search_path_set: bool,
}

type Reply = Result<Value, String>;

/// A session as LLM agents see it.
struct AgentWorld<'a>(&'a mut Session);

impl familiar_agent::World for AgentWorld<'_> {
    fn agent_tools(&self) -> Option<&[familiar_engine::AgentTool]> {
        self.0.engine.agent_tools()
    }

    fn agent_call(&mut self, tool: &str, args: &Value) -> Result<u64, String> {
        self.0.engine.agent_call(tool, args)
    }

    fn last_lines(&self, n: usize) -> Vec<String> {
        self.0.recent.iter().skip(self.0.recent.len().saturating_sub(n)).cloned().collect()
    }

    fn agent_send(&mut self, lines: &[String]) -> Result<u64, String> {
        self.0.engine.agent_send(lines)
    }

    fn agent_heard(&self, mark: u64) -> familiar_engine::AgentHeard {
        self.0.engine.agent_heard(mark)
    }
}

fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| format!("missing \"{key}\""))
}

fn number(args: &Value, key: &str) -> Result<f64, String> {
    args.get(key).and_then(Value::as_f64).ok_or_else(|| format!("missing \"{key}\""))
}

fn flag(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn room(args: &Value, key: &str) -> Result<u64, String> {
    args.get(key).and_then(Value::as_u64).ok_or_else(|| format!("missing \"{key}\""))
}

impl State {
    fn new() -> Self {
        let config_path = familiar_config::default_config_path();
        let (config, load_error) = match Config::load(&config_path) {
            Ok(config) => (config, None),
            Err(e) => (Config::default(), Some(e.to_string())),
        };
        // Agents' requests go to their world's inbox, to be answered on the main thread.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentRequest>();
        let inboxes: Arc<Mutex<HashMap<u64, Arc<Inbox>>>> = Arc::default();
        let deliver = inboxes.clone();
        runtime().spawn(async move {
            while let Some(request) = rx.recv().await {
                let id = request.session;
                let inbox = deliver.lock().unwrap().get(&id).cloned();
                // A world that's gone drops the request, and the agent hears it was closed.
                if let Some(inbox) = inbox {
                    inbox.agent.lock().unwrap().push_back(request);
                    inbox.wake(id);
                }
            }
        });
        Self {
            config,
            config_path,
            load_error,
            palette: Palette::default(),
            sessions: HashMap::new(),
            next_id: 1,
            agents: Agents::new(tx, Some(runtime().handle().clone())),
            inboxes,
            search_path_set: false,
        }
    }

    fn keymap(&self) -> Keymap {
        Keymap::new(&self.config.settings, keymap::Platform::desktop())
    }

    fn save(&self) -> Result<(), String> {
        if let Some(e) = &self.load_error {
            return Err(format!("Your settings weren't saved, because config.toml couldn't be read when Familiar started ({e}). Fix the file and start Familiar again."));
        }
        self.config.save(&self.config_path).map_err(|e| format!("Couldn't save your settings: {e}"))
    }

    /// Every open session picks up the config as it is now.
    fn reload_sessions(&mut self) {
        for (id, session) in &mut self.sessions {
            let out = session.engine.reload(&self.config);
            session.pending.extend(out);
            session.inbox.wake(*id);
        }
    }

    fn session(&mut self, args: &Value) -> Result<(u64, &mut Session), String> {
        let id = args.get("id").and_then(Value::as_u64).ok_or("missing \"id\"")?;
        self.sessions.get_mut(&id).map(|s| (id, s)).ok_or_else(|| format!("no session {id}"))
    }

    fn start_scripting(&mut self, id: u64) -> Vec<Output> {
        let Some(session) = self.sessions.get_mut(&id) else { return Vec::new() };
        let host = match familiar_script::JsHost::new() {
            Ok(host) => host,
            Err(e) => return vec![Output::Notice(format!("Scripting is unavailable: {e}"))],
        };
        let mut out = session.engine.set_script_host(Box::new(host));
        for path in session.engine.startup_scripts() {
            out.push(Output::Notice(format!("Running {}", familiar_config::paths::display(&path))));
            out.extend(session.engine.load_script(&path));
        }
        out
    }

    fn connect(&mut self, id: u64) -> Vec<Output> {
        let Some(session) = self.sessions.get_mut(&id) else { return Vec::new() };
        if session.engine.is_connected() {
            return Vec::new();
        }
        let (mut events, out) = {
            let _guard = runtime().enter();
            session.engine.connect()
        };
        session.generation += 1;
        let generation = session.generation;
        let inbox = session.inbox.clone();
        runtime().spawn(async move {
            while let Some(event) = events.recv().await {
                inbox.events.lock().unwrap().push_back((generation, event));
                inbox.wake(id);
            }
        });
        out
    }

    /// Turns the engine's outputs into JSON for the app, doing the ones that are ours to do
    /// (reconnecting, restarting scripts, saving variables and settings) on the way.
    fn finish(&mut self, id: u64, outputs: Vec<Output>) -> Vec<Value> {
        let mut queue: VecDeque<Output> = outputs.into();
        let mut shown = Vec::new();
        let mut config_changed = false;
        while let Some(output) = queue.pop_front() {
            let v = match output {
                Output::Line { line, target } => {
                    if target == Target::Main {
                        self.remember(id, &line.text);
                    }
                    let target = match target {
                        Target::Main => Value::Null,
                        Target::Spawn(name) => name.into(),
                    };
                    json!({ "t": "line", "target": target, "plain": line.text, "bell": line.bell, "line": json::line(&line, &self.palette) })
                }
                Output::Prompt { line, terminated } => json!({ "t": "prompt", "line": json::line(&line, &self.palette), "terminated": terminated }),
                Output::Notice(text) => {
                    self.remember(id, &text);
                    json!({ "t": "notice", "text": text })
                }
                Output::Echo(text) => {
                    self.remember(id, &text);
                    json!({ "t": "echo", "text": text })
                }
                Output::Bell => json!({ "t": "bell" }),
                Output::PasswordMode(on) => json!({ "t": "password", "on": on }),
                Output::Status(status) => json!({ "t": "status", "status": json::status(&status) }),
                Output::Activity { important } => json!({ "t": "activity", "important": important }),
                Output::Notify { title, body } => json!({ "t": "notify", "title": title, "body": body }),
                Output::PlaySound(path) => json!({ "t": "sound", "path": familiar_config::paths::expand(&path).to_string_lossy() }),
                Output::OpenUri(uri) => json!({ "t": "open_uri", "uri": uri }),
                Output::SetInput(text) => json!({ "t": "set_input", "text": text }),
                Output::ClearOutput => json!({ "t": "clear" }),
                Output::Reconnect => {
                    queue.extend(self.connect(id));
                    continue;
                }
                Output::ReloadScripts => {
                    queue.push_back(Output::Notice("Restarting scripting".into()));
                    queue.extend(self.start_scripting(id));
                    continue;
                }
                Output::VariablesChanged { scope, variables } => {
                    if let Some(rules) = self.config.rules_mut(&scope) {
                        rules.variables = variables;
                    }
                    config_changed = true;
                    continue;
                }
                Output::SettingsChanged(settings) => {
                    self.config.settings = settings;
                    config_changed = true;
                    continue;
                }
                Output::LoggingChanged(path) => json!({ "t": "logging", "path": path.map(|p| p.to_string_lossy().into_owned()) }),
                Output::CompileError { reference, line, message } => json!({ "t": "compile_error", "reference": reference, "line": line, "message": message }),
                Output::LineAvatar(avatar) => json!({ "t": "line_avatar", "avatar": json::avatar(&avatar) }),
                Output::AvatarUpdated(avatar) => json!({ "t": "avatar_updated", "avatar": json::avatar(&avatar) }),
                Output::InlineImage { url } => json!({ "t": "inline_image", "url": url }),
                Output::OpenPuppet { name, focus } => json!({ "t": "open_puppet", "name": name, "focus": focus }),
                Output::PuppetLine { name, line } => json!({ "t": "puppet_line", "name": name, "line": json::line(&line, &self.palette) }),
                Output::PuppetEcho { name, text } => json!({ "t": "puppet_echo", "name": name, "text": text }),
                Output::Speak(text) => json!({ "t": "speak", "text": text }),
                Output::SpeechStop => json!({ "t": "speech_stop" }),
                Output::Tilemap { name, map } => json!({ "t": "tilemap", "name": name, "map": json::tilemap(&map) }),
                Output::MapChanged => json!({ "t": "map_changed" }),
                Output::ShowMap => json!({ "t": "show_map" }),
                Output::Stats { pane, data } => json!({ "t": "stats", "pane": pane, "data": data.as_ref().map(json::stat_pane) }),
                Output::Media(command) => json!({ "t": "media", "command": json::media(&command) }),
                Output::StatusText(text) => json!({ "t": "status_text", "text": text }),
                Output::Edit(request) => json!({ "t": "edit", "request": json::edit_request(&request) }),
                Output::WebView(command) => json!({ "t": "webview", "command": json::webview(&command) }),
                Output::Gmcp { package, data } => json!({ "t": "gmcp", "package": package, "data": data }),
                Output::Received(text) => json!({ "t": "received", "text": text }),
                Output::Sent(text) => json!({ "t": "sent", "text": text }),
                Output::Agent(args) => {
                    let (shown_now, more) = self.agent_command(id, &args);
                    shown.extend(shown_now);
                    queue.extend(more);
                    continue;
                }
                Output::AgentTools(_) => {
                    self.agents.tools_changed(id);
                    continue;
                }
                Output::AgentResult { id: call, ok, content } => {
                    self.agents.result(id, call, ok, content);
                    continue;
                }
            };
            shown.push(v);
        }
        if config_changed {
            if let Err(e) = self.save() {
                shown.push(json!({ "t": "notice", "text": e }));
            }
            shown.push(json!({ "t": "config_changed" }));
        }
        self.schedule_timer(id);
        shown
    }

    /// Makes sure we're woken when the session's next script timer or /delay is due.
    fn schedule_timer(&mut self, id: u64) {
        let Some(session) = self.sessions.get_mut(&id) else { return };
        let deadline = session.engine.next_deadline();
        if session.timer.as_ref().map(|(_, at)| *at) == deadline {
            return;
        }
        if let Some((task, _)) = session.timer.take() {
            task.abort();
        }
        let Some(deadline) = deadline else { return };
        let inbox = session.inbox.clone();
        let task = runtime().spawn(async move {
            tokio::time::sleep_until(deadline.into()).await;
            inbox.wake(id);
        });
        session.timer = Some((task, deadline));
    }

    /// Keeps a line of the world's window for agents, as plain text.
    fn remember(&mut self, id: u64, text: &str) {
        let Some(session) = self.sessions.get_mut(&id) else { return };
        for line in text.lines() {
            if session.recent.len() == familiar_agent::MAX_OUTPUT_LINES {
                session.recent.pop_front();
            }
            session.recent.push_back(line.to_string());
        }
    }

    /// `/agent [name]`, `/agent stop`, `/agent list`: what to show, and outputs to run through.
    /// Running an agent is the app's job: it gets an agent_start with the program to run in a
    /// terminal, and calls agent.ended when the program ends.
    fn agent_command(&mut self, id: u64, args: &str) -> (Vec<Value>, Vec<Output>) {
        let notice = |text: String| (Vec::new(), vec![Output::Notice(text)]);
        let Some(session) = self.sessions.get_mut(&id) else { return (Vec::new(), Vec::new()) };
        match args.trim() {
            "list" => notice(format!("Agents: {} ([[settings.agent]] in the config adds more)", familiar_agent::names(&self.config.settings))),
            "stop" => match session.agent.take() {
                Some((name, token)) => {
                    session.tty = None;
                    self.agents.close(&token);
                    (vec![json!({ "t": "agent_stop", "token": token })], vec![Output::Notice(format!("Stopped {name}"))])
                }
                None => notice("There's no agent working on this world".into()),
            },
            wanted => {
                if session.agent.is_some() {
                    return (vec![json!({ "t": "agent_focus" })], vec![Output::Notice("This world has an agent already (/agent stop stops it)".into())]);
                }
                // A Mac app opened from the Finder doesn't have the user's PATH.
                if cfg!(target_os = "macos") && !self.search_path_set {
                    self.agents.set_search_path(familiar_agent::login_shell_path());
                    self.search_path_set = true;
                }
                let agent = match self.agents.choose(&self.config.settings, wanted) {
                    Ok(agent) => agent,
                    Err(e) => return notice(e),
                };
                let Some(session) = self.sessions.get_mut(&id) else { return (Vec::new(), Vec::new()) };
                let world = session.engine.world().name.clone();
                let world = if world.is_empty() { session.engine.title() } else { world };
                let launch = match self.agents.launch(&agent, id, &world) {
                    Ok(launch) => launch,
                    Err(e) => return notice(e),
                };
                if session.run_agents {
                    let inbox = session.inbox.clone();
                    // The app resizes it to fit as soon as it's on screen.
                    match tty::Tty::spawn(&launch, 80, 24, move || inbox.wake(id)) {
                        Ok(tty) => session.tty = Some(tty),
                        Err(e) => {
                            self.agents.close(&launch.token);
                            return notice(format!("Couldn't run {}: {e}", launch.program));
                        }
                    }
                }
                session.agent = Some((launch.name.clone(), launch.token.clone()));
                let mut more = Vec::new();
                if session.engine.agent_tools().is_none() {
                    more.push(Output::Notice("This world offers no tools (its MOO needs the dns-com-familiar-agent package), so the agent can only read its output".into()));
                }
                let env: serde_json::Map<String, Value> = launch.env.iter().map(|(k, v)| (k.clone(), Value::from(v.as_str()))).collect();
                if session.run_agents {
                    return (vec![json!({ "t": "agent_start", "name": launch.name, "token": launch.token, "pty": true })], more);
                }
                let start = json!({
                    "t": "agent_start",
                    "name": launch.name,
                    "token": launch.token,
                    "program": launch.program,
                    "args": launch.args,
                    "dir": launch.dir.to_string_lossy(),
                    "env": env,
                });
                (vec![start], more)
            }
        }
    }

    fn outputs(&mut self, id: u64, outputs: Vec<Output>) -> Reply {
        Ok(json!({ "outputs": self.finish(id, outputs) }))
    }

    fn poll(&mut self, args: &Value) -> Reply {
        let (id, _) = self.session(args)?;
        let Self { sessions, agents, .. } = self;
        let Some(session) = sessions.get_mut(&id) else { return Err(format!("no session {id}")) };
        session.inbox.woken.store(false, Ordering::SeqCst);
        let mut out = std::mem::take(&mut session.pending);
        let events: Vec<SessionEvent> = {
            let mut queue = session.inbox.events.lock().unwrap_or_else(|e| e.into_inner());
            let n = queue.len().min(BATCH_LIMIT);
            queue.drain(..n).filter(|(generation, _)| *generation == session.generation).map(|(_, event)| event).collect()
        };
        for event in events {
            out.extend(session.engine.handle_event(event));
        }
        let requests: Vec<AgentRequest> = session.inbox.agent.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
        for request in requests {
            agents.answer(request, Some(&mut AgentWorld(session)));
        }
        // What the agent we run printed, and whether it has stopped.
        let mut shown = Vec::new();
        if let Some(printed) = session.tty.as_ref().map(tty::Tty::take_output) {
            if !printed.bytes.is_empty() {
                shown.push(json!({ "t": "agent_output", "data": tty::base64(&printed.bytes) }));
            }
            if printed.exited
                && session.tty.take().is_some()
                && let Some((name, token)) = session.agent.take()
            {
                agents.close(&token);
                shown.push(json!({ "t": "agent_stop", "token": token }));
                out.push(Output::Notice(format!("{name} stopped")));
            }
        }
        let now = Instant::now();
        if session.engine.next_deadline().is_some_and(|due| due <= now) {
            // This timer has fired; a new one is set below.
            session.timer = None;
            out.extend(session.engine.tick(now));
        }
        let more = !session.inbox.events.lock().unwrap_or_else(|e| e.into_inner()).is_empty();
        shown.extend(self.finish(id, out));
        Ok(json!({ "outputs": shown, "more": more }))
    }

    fn open(&mut self, args: &Value) -> Reply {
        let world: World = serde_json::from_value(args.get("world").cloned().unwrap_or_default()).map_err(|e| format!("bad world: {e}"))?;
        let character = args.get("character").and_then(Value::as_str).and_then(|name| world.characters.iter().find(|c| c.name == name).cloned());
        let (mut engine, mut out) = Engine::new(&self.config, world, character);
        engine.set_webviews_supported(true);
        let id = self.next_id;
        self.next_id += 1;
        // Only apps that can run agents (in a terminal) say so: the engine then offers MOOs the
        // agent package and passes /agent on.
        // "pty" also asks us to run them, in a pseudo-terminal, and pass on what they print.
        let run_agents = args.get("agents").and_then(Value::as_str) == Some("pty");
        engine.set_agents_supported(flag(args, "agents") || run_agents);
        let inbox = Arc::new(Inbox { events: Mutex::default(), woken: AtomicBool::new(false), agent: Mutex::default() });
        self.inboxes.lock().unwrap_or_else(|e| e.into_inner()).insert(id, inbox.clone());
        let session = Session { engine, generation: 0, inbox, pending: Vec::new(), timer: None, agent: None, run_agents, tty: None, recent: VecDeque::new() };
        self.sessions.insert(id, session);
        out.extend(self.start_scripting(id));
        let outputs = self.finish(id, out);
        let mut info = self.info(id)?;
        info["outputs"] = outputs.into();
        Ok(info)
    }

    fn info(&mut self, id: u64) -> Reply {
        let session = self.sessions.get_mut(&id).ok_or_else(|| format!("no session {id}"))?;
        let engine = &session.engine;
        let world = engine.world();
        Ok(json!({
            "id": id,
            "title": engine.title(),
            "world": serde_json::to_value(world).map_err(|e| e.to_string())?,
            "saved": self.config.world(&world.name).is_some(),
            "character": engine.character_name(),
            "connected": engine.is_connected(),
            "logging": engine.is_logging(),
            "has_map": !engine.map().is_empty(),
            "scopes": engine.scopes().iter().map(json::scope).collect::<Vec<_>>(),
            "puppets": engine.puppets().map(|p| p.name.clone()).collect::<Vec<_>>(),
        }))
    }

    fn close(&mut self, args: &Value) -> Reply {
        let (id, session) = self.session(args)?;
        session.engine.disconnect();
        let saved = session.engine.save_map();
        if let Some((task, _)) = session.timer.take() {
            task.abort();
        }
        // The app stops the agent's program with the tab, or we do.
        session.tty = None;
        if let Some((_, token)) = session.agent.take() {
            self.agents.close(&token);
        }
        self.agents.forget(id);
        self.inboxes.lock().unwrap().remove(&id);
        self.sessions.remove(&id);
        saved.map(|_| Value::Null)
    }

    fn map(&mut self, args: &Value) -> Reply {
        let (_, session) = self.session(args)?;
        let map = session.engine.map();
        let rooms: Vec<Value> = map.rooms().filter_map(|r| serde_json::to_value(r).ok()).collect();
        Ok(json!({
            "rooms": rooms,
            "areas": map.areas(),
            "current": session.engine.current_room(),
            "room_w": familiar_map::ROOM_W,
            "room_h": familiar_map::ROOM_H,
            "grid_x": familiar_map::GRID_X,
            "grid_y": familiar_map::GRID_Y,
        }))
    }

    fn set_config(&mut self, args: &Value) -> Reply {
        let config: Config = serde_json::from_value(args.get("config").cloned().unwrap_or_default()).map_err(|e| format!("bad config: {e}"))?;
        self.config = config;
        let saved = self.save();
        self.reload_sessions();
        saved.map(|_| Value::Null)
    }

    /// Imports a BeipMU Config.txt: worlds, characters, rules and (if we have none) its
    /// startup script. Returns a heading, a summary and what couldn't be carried over.
    fn import_beipmu(&mut self, args: &Value) -> Reply {
        let path = PathBuf::from(text(args, "path")?);
        let reply = |heading: &str, body: String, warnings: &[String]| Ok(json!({ "heading": heading, "body": body, "warnings": warnings }));
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) => return reply("Couldn’t Read the File", format!("{}: {e}", path.display()), &[]),
        };
        // BeipMU writes UTF-8; very old configs may be Windows-1252.
        let contents = String::from_utf8(bytes).unwrap_or_else(|e| e.into_bytes().iter().map(|&b| b as char).collect());
        let imported = familiar_import::config::import(&contents);
        let summary = familiar_import::config::merge(&mut self.config, &imported);
        let mut body = summary.describe();
        body.push('.');
        if let Some(script) = &imported.startup_script {
            body.push_str("\n\n");
            body.push_str(&copy_startup_script(script, &path));
        }
        let saved = self.save();
        self.reload_sessions();
        match saved {
            Ok(()) => reply("BeipMU Configuration Imported", body, &imported.warnings),
            Err(e) => reply("Couldn’t Save", e, &imported.warnings),
        }
    }

    fn add_examples(&mut self, args: &Value) -> Reply {
        let scope = args.get("scope").and_then(json::parse_scope).ok_or("bad \"scope\"")?;
        let rules = self.config.rules_mut(&scope).ok_or_else(|| format!("{} isn't saved, so it has no rules of its own", scope.label()))?;
        let added = rules.add_examples();
        if added.total() > 0 {
            self.save()?;
            self.reload_sessions();
        }
        Ok(json!({ "added": added.total(), "message": added.describe() }))
    }

    fn call(&mut self, method: &str, args: &Value) -> Reply {
        match method {
            // What the app needs to start: the config, where things are, the colours.
            "start" => Ok(json!({
                "config": serde_json::to_value(&self.config).map_err(|e| e.to_string())?,
                "load_error": self.load_error,
                "migrated": familiar_config::migrate_old_dirs(),
                "palette": json::palette(&self.palette),
                "config_path": self.config_path.to_string_lossy(),
                "config_dir": familiar_config::config_dir().to_string_lossy(),
                "log_dir": familiar_config::log_dir().to_string_lossy(),
                "startup_script": familiar_config::startup_script_path().to_string_lossy(),
                "app_name": familiar_config::APP_NAME,
                "version": env!("CARGO_PKG_VERSION"),
                // The script every web view runs first, shared with the GTK client.
                "webview_bridge_js": include_str!("../../gtk/src/webview_bridge.js"),
                // New, empty ones of each thing the app can add to the config.
                "defaults": {
                    "world": serde_json::to_value(World::default()).unwrap_or_default(),
                    "puppet": serde_json::to_value(familiar_config::Puppet::default()).unwrap_or_default(),
                    "trigger": serde_json::to_value(familiar_config::Trigger::default()).unwrap_or_default(),
                    "alias": serde_json::to_value(familiar_config::Alias::default()).unwrap_or_default(),
                    "settings": serde_json::to_value(familiar_config::Settings::default()).unwrap_or_default(),
                },
                "commands": familiar_engine::COMMANDS.iter().map(|(usage, about)| json!([usage, about])).collect::<Vec<_>>(),
            })),
            "config.get" => serde_json::to_value(&self.config).map_err(|e| e.to_string()),
            // Keyboard shortcuts. "keys.actions": every action this app has, in order, with its
            // label, group, scope ("window" or "editor"), keys, defaults, whether the user changed
            // it, and for each key the other actions and the keyboard macros on it.
            "keys.actions" => serde_json::to_value(self.keymap().describe(&self.config)).map_err(|e| e.to_string()),
            // The action a key press runs: {"scope": "window"|"editor", "key", "ctrl", "alt",
            // "shift", "cmd"} → its id, or null. Ask before running a macro: shortcuts come first.
            "keys.match" => {
                let chord = KeyChord::new(text(args, "key")?, flag(args, "ctrl"), flag(args, "alt"), flag(args, "shift")).with_cmd(flag(args, "cmd"));
                let scope = if args.get("scope").and_then(Value::as_str) == Some("editor") { keymap::Scope::Editor } else { keymap::Scope::Window };
                Ok(self.keymap().lookup(scope, &chord).into())
            }
            // Sets an action's keys ({"id", "keys": ["Ctrl+K", ...]}; [] for none) or with "keys"
            // null puts back its defaults. Saves, and returns what "keys.actions" does.
            "keys.set" => {
                let id = text(args, "id")?.to_string();
                let keys: Option<Vec<String>> = match args.get("keys") {
                    None | Some(Value::Null) => None,
                    Some(v) => Some(serde_json::from_value(v.clone()).map_err(|e| format!("\"keys\" should be a list of keys: {e}"))?),
                };
                keymap::set_keys(&mut self.config.settings, keymap::Platform::desktop(), &id, keys.as_deref())?;
                self.save()?;
                self.reload_sessions();
                serde_json::to_value(self.keymap().describe(&self.config)).map_err(|e| e.to_string())
            }
            // Saves the config and has every open session pick it up.
            "config.set" => self.set_config(args),
            "import.beipmu" => self.import_beipmu(args),
            // Adds the example triggers, aliases, macros and variable that a set of rules doesn't
            // have yet. "scope" says whose: {"kind": "global"}, or "world" with "world", or
            // "character" with "world" and "character".
            "rules.add_examples" => self.add_examples(args),

            "session.open" => self.open(args),
            "session.info" => {
                let (id, _) = self.session(args)?;
                self.info(id)
            }
            "session.close" => self.close(args),
            "session.poll" => self.poll(args),
            "session.connect" => {
                let (id, _) = self.session(args)?;
                let out = self.connect(id);
                self.outputs(id, out)
            }
            "session.disconnect" => {
                let (_, session) = self.session(args)?;
                session.engine.disconnect();
                Ok(Value::Null)
            }
            "session.submit" => {
                let (id, session) = self.session(args)?;
                let out = session.engine.submit(text(args, "text")?, flag(args, "secret"));
                self.outputs(id, out)
            }
            // A key press. "handled" says whether a macro ran.
            "session.macro" => {
                let (id, session) = self.session(args)?;
                let chord = KeyChord::new(text(args, "key")?, flag(args, "ctrl"), flag(args, "alt"), flag(args, "shift")).with_cmd(flag(args, "cmd"));
                match session.engine.run_macro(&chord) {
                    Some(out) => Ok(json!({ "handled": true, "outputs": self.finish(id, out) })),
                    None => Ok(json!({ "handled": false, "outputs": [] })),
                }
            }
            "session.link" => {
                let (id, session) = self.session(args)?;
                let out = session.engine.follow_link(text(args, "uri")?);
                self.outputs(id, out)
            }
            "session.toggle_log" => {
                let (id, session) = self.session(args)?;
                let out = session.engine.toggle_log();
                self.outputs(id, out)
            }
            "session.resize" => {
                let (_, session) = self.session(args)?;
                let cols = number(args, "cols")?.clamp(1.0, u16::MAX as f64) as u16;
                let rows = number(args, "rows")?.clamp(1.0, u16::MAX as f64) as u16;
                session.engine.set_window_size(cols, rows);
                Ok(Value::Null)
            }
            "session.gmcp" => {
                let (id, session) = self.session(args)?;
                let out = session.engine.send_gmcp(text(args, "package")?, args.get("data").and_then(Value::as_str));
                self.outputs(id, out)
            }
            "session.send_text" => {
                let (id, session) = self.session(args)?;
                let out = session.engine.send_text(text(args, "text")?, flag(args, "aliases"));
                self.outputs(id, out)
            }
            // Handles a line as if the server had sent it.
            "session.inject" => {
                let (id, session) = self.session(args)?;
                let out = session.engine.inject_line(text(args, "text")?);
                self.outputs(id, out)
            }
            "session.save_edit" => {
                let (id, session) = self.session(args)?;
                let request = args.get("request").and_then(json::parse_edit_request).ok_or("bad \"request\"")?;
                let out = session.engine.save_edit(&request, text(args, "content")?);
                self.outputs(id, out)
            }
            "session.puppet_submit" => {
                let (id, session) = self.session(args)?;
                let out = session.engine.puppet_submit(text(args, "name")?, text(args, "text")?);
                self.outputs(id, out)
            }
            "session.puppet_open" => {
                let (_, session) = self.session(args)?;
                session.engine.set_puppet_open(text(args, "name")?, flag(args, "open"));
                Ok(Value::Null)
            }
            // Typing into the agent we run ("pty"): "data" is the text.
            "agent.write" => {
                let (_, session) = self.session(args)?;
                if let Some(tty) = &mut session.tty {
                    tty.write(text(args, "data")?.as_bytes());
                }
                Ok(Value::Null)
            }
            "agent.resize" => {
                let (_, session) = self.session(args)?;
                if let Some(tty) = &session.tty {
                    tty.resize(number(args, "cols")?.clamp(2.0, 1000.0) as u16, number(args, "rows")?.clamp(2.0, 1000.0) as u16);
                }
                Ok(Value::Null)
            }
            // /agent stop, from the app (closing the agent's tab, say).
            "agent.stop" => {
                let (id, _) = self.session(args)?;
                self.outputs(id, vec![Output::Agent("stop".into())])
            }
            // The agent the app was running for a world ended (or never started, with "error").
            // "token" is the one from its agent_start, so a late report is ignored.
            "agent.ended" => {
                let (id, session) = self.session(args)?;
                let token = text(args, "token")?;
                let Some((name, _)) = session.agent.take_if(|(_, t)| t == token) else { return self.outputs(id, Vec::new()) };
                self.agents.close(token);
                let message = match args.get("error").and_then(Value::as_str) {
                    Some(error) => format!("Couldn't run {name}: {error}"),
                    None => format!("{name} stopped"),
                };
                self.outputs(id, vec![Output::Notice(message)])
            }
            "session.restart_scripts" => {
                let (id, _) = self.session(args)?;
                self.outputs(id, vec![Output::ReloadScripts])
            }

            "map.get" => self.map(args),
            "map.save" => {
                let (_, session) = self.session(args)?;
                session.engine.save_map().map(|_| Value::Null)
            }
            "map.search" => {
                let (_, session) = self.session(args)?;
                Ok(json!(session.engine.map().search(text(args, "text")?)))
            }
            "map.walk" => {
                let (id, session) = self.session(args)?;
                let out = session.engine.map_walk_to(room(args, "room")?);
                self.outputs(id, out)
            }
            "map.set_current" => {
                let (id, session) = self.session(args)?;
                let out = session.engine.map_set_current(args.get("room").and_then(Value::as_u64));
                self.outputs(id, out)
            }
            "map.move" => {
                let (id, session) = self.session(args)?;
                let out = session.engine.map_move_room(room(args, "room")?, number(args, "x")?, number(args, "y")?);
                self.outputs(id, out)
            }
            "map.rename" => {
                let (id, session) = self.session(args)?;
                let out = session.engine.map_rename_room(room(args, "room")?, text(args, "name")?);
                self.outputs(id, out)
            }
            "map.delete" => {
                let (id, session) = self.session(args)?;
                let out = session.engine.map_delete_room(room(args, "room")?);
                self.outputs(id, out)
            }
            "map.add" => {
                let (id, session) = self.session(args)?;
                let (room, out) = session.engine.map_add_room_at(text(args, "name")?, text(args, "area")?, number(args, "x")?, number(args, "y")?);
                Ok(json!({ "room": room, "outputs": self.finish(id, out) }))
            }
            "map.import" => {
                let (id, session) = self.session(args)?;
                let out = session.engine.map_import(Path::new(text(args, "path")?));
                self.outputs(id, out)
            }
            _ => Err(format!("unknown method {method}")),
        }
    }
}

/// Copies BeipMU's startup script to ours when it can be found next to Config.txt (or at its
/// path) and we don't have one yet. Returns a sentence about what happened.
fn copy_startup_script(written: &str, config: &Path) -> String {
    let name = written.rsplit(['\\', '/']).next().unwrap_or(written);
    let candidates = [Some(Path::new(written).to_path_buf()), config.parent().map(|dir| dir.join(name))];
    let found = candidates.into_iter().flatten().find(|p| p.is_file());
    let ours = familiar_config::startup_script_path();
    match found {
        Some(_) if ours.exists() => {
            format!("BeipMU ran {name} at startup. You already have {}, so it was left alone; add BeipMU's code there if you want it.", ours.display())
        }
        Some(source) => {
            let copied = ours.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::copy(&source, &ours));
            match copied {
                Ok(_) => format!("BeipMU's startup script was copied to {}; it runs when a world opens.", ours.display()),
                Err(e) => format!("Couldn't copy the startup script {}: {e}", source.display()),
            }
        }
        None => format!("BeipMU ran a startup script ({written}). Copy it to {} to use it here.", ours.display()),
    }
}

fn rule<T: serde::de::DeserializeOwned>(args: &Value) -> Result<T, String> {
    serde_json::from_value(args.get("rule").cloned().unwrap_or_default()).map_err(|e| format!("bad \"rule\": {e}"))
}

/// Methods that don't touch the sessions or the config, so any thread may call them.
fn call_stateless(method: &str, args: &Value) -> Option<Reply> {
    Some(match method {
        // (any thread) A local file for a sound's URL, downloaded if need be. Blocks.
        "media.fetch" => text(args, "url").and_then(familiar_engine::media::fetch).map(|path| json!({ "path": path.to_string_lossy() })),
        // (any thread) The built-in directory of public worlds.
        "directory" => Ok(familiar_config::directory::entries()
            .iter()
            .map(|e| {
                json!({
                    "name": e.name,
                    "host": e.host,
                    "port": e.port,
                    "tls": e.tls,
                    "keywords": e.keywords,
                    "description": e.description,
                    "website": e.website,
                    "world": serde_json::to_value(e.to_world()).unwrap_or_default(),
                })
            })
            .collect()),
        // (any thread) "host:port", "host port tls" and so on as a world, or null.
        "address.parse" => text(args, "text").map(|t| Address::parse(t).and_then(|a| serde_json::to_value(a.into_world(World::default().port)).ok()).unwrap_or_default()),
        // (any thread) A line of text with ANSI codes, as a styled line.
        "ansi.parse" => text(args, "text").map(|t| json::line(&AnsiParser::new().parse_line(t), &Palette::default())),
        // (any thread) The indentation for the line being finished and the one after it.
        "code.on_enter" => text(args, "before").map(|before| match familiar_code::Lang::from_id(args.get("language").and_then(Value::as_str).unwrap_or("")) {
            Some(lang) => {
                let (current, next) = familiar_code::on_enter(before, lang);
                json!({ "current": current, "next": next })
            }
            None => Value::Null,
        }),
        // (any thread) The whole text re-indented, or null for a language without rules.
        "code.reindent" => text(args, "text").map(|t| match familiar_code::Lang::from_id(args.get("language").and_then(Value::as_str).unwrap_or("")) {
            Some(lang) => familiar_code::reindent(t, lang, args.get("unit").and_then(Value::as_str).unwrap_or("  ")).into(),
            None => Value::Null,
        }),
        // (any thread) Whether a trigger's or alias's pattern is usable: null, or what's wrong.
        // "rule" is the trigger or alias as it is in the config.
        "rules.check_pattern" => rule::<familiar_config::Trigger>(args).map(|t| familiar_engine::rules::compile_pattern(&t.matcher).err().into()),
        // (any thread) What a trigger does with a line: {"matched": bool, "result": text}.
        "rules.test_trigger" => rule::<familiar_config::Trigger>(args).and_then(|t| {
            let result = familiar_engine::rules::test_trigger(&t, text(args, "text")?)?;
            Ok(json!({ "matched": result.is_some(), "result": result }))
        }),
        // (any thread) What an alias sends for some input: {"matched": bool, "lines": [...]}.
        "rules.test_alias" => rule::<familiar_config::Alias>(args).and_then(|a| {
            let result = familiar_engine::rules::test_alias(&a, text(args, "text")?)?;
            Ok(json!({ "matched": result.is_some(), "lines": result.unwrap_or_default() }))
        }),
        // (any thread) Whether a key chord like "Ctrl+F5" is one the engine understands.
        "keys.parse" => text(args, "text").map(|t| KeyChord::parse(t).map(|c| c.to_string()).into()),
        _ => return None,
    })
}

/// Runs a method. Public so the crate's tests (and any Rust embedder) can skip the C layer.
pub fn call(method: &str, args: &Value) -> Reply {
    if let Some(reply) = call_stateless(method, args) {
        return reply;
    }
    if MAIN_THREAD.get_or_init(|| std::thread::current().id()) != &std::thread::current().id() {
        return Err(format!("{method} must be called on the main thread"));
    }
    #[cfg(windows)]
    if let Some(reply) = windows::call(method, args) {
        return reply;
    }
    STATE.with(|state| {
        let mut state = state.try_borrow_mut().map_err(|_| format!("{method} was called from inside another call"))?;
        state.get_or_insert_with(State::new).call(method, args)
    })
}

/// # Safety
/// Call once, before anything else, on the thread that will make the other calls.
#[unsafe(no_mangle)]
pub extern "C" fn familiar_init(wake: Option<WakeFn>) {
    if let Some(wake) = wake {
        let _ = WAKE.set(wake);
    }
    let _ = MAIN_THREAD.set(std::thread::current().id());
}

/// # Safety
/// `method` and `args_json` must be NUL-terminated strings (or `args_json` null). The reply
/// must be freed with [`familiar_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn familiar_call(method: *const c_char, args_json: *const c_char) -> *mut c_char {
    let reply = std::panic::catch_unwind(|| {
        if method.is_null() {
            return json!({ "error": "null method" });
        }
        let method = unsafe { CStr::from_ptr(method) }.to_string_lossy();
        let args = if args_json.is_null() {
            Value::Null
        } else {
            match serde_json::from_slice(unsafe { CStr::from_ptr(args_json) }.to_bytes()) {
                Ok(args) => args,
                Err(e) => return json!({ "error": format!("bad arguments for {method}: {e}") }),
            }
        };
        match call(&method, &args) {
            Ok(value) => json!({ "ok": value }),
            Err(e) => json!({ "error": e }),
        }
    })
    .unwrap_or_else(|_| json!({ "error": "Familiar's engine hit an internal error (a panic); see the console for details." }));
    // JSON text never contains a NUL byte, so this can't fail.
    CString::new(reply.to_string()).unwrap_or_default().into_raw()
}

/// # Safety
/// `reply` must have come from [`familiar_call`], and must not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn familiar_free(reply: *mut c_char) {
    if !reply.is_null() {
        drop(unsafe { CString::from_raw(reply) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_read_back_as_written() {
        use familiar_config::Scope;
        for scope in [Scope::Global, Scope::World { world: "Vale".into() }, Scope::Character { world: "Vale".into(), character: "Mara".into() }] {
            assert_eq!(json::parse_scope(&json::scope(&scope)), Some(scope));
        }
        assert_eq!(json::parse_scope(&json!({ "kind": "world" })), None);
    }

    #[test]
    fn stateless_methods() {
        let world = call("address.parse", &json!({ "text": "example.org 7777 tls" })).unwrap();
        assert_eq!(world["host"], "example.org");
        assert_eq!(world["port"], 7777);
        assert_eq!(world["tls"], true);
        assert_eq!(call("address.parse", &json!({ "text": "" })).unwrap(), Value::Null);
        assert!(!call("directory", &Value::Null).unwrap().as_array().unwrap().is_empty());
        assert_eq!(call("keys.parse", &json!({ "text": "control+k" })).unwrap(), "Ctrl+K");
        assert!(call("no.such", &Value::Null).is_err());
        let trigger = json!({ "pattern": "(\\w+) waves", "regex": true, "send": "wave $1" });
        assert_eq!(call("rules.check_pattern", &json!({ "rule": trigger })).unwrap(), Value::Null);
        assert!(call("rules.check_pattern", &json!({ "rule": { "pattern": "(", "regex": true } })).unwrap().is_string());
        let tested = call("rules.test_trigger", &json!({ "rule": trigger, "text": "Bob waves" })).unwrap();
        assert_eq!(tested["matched"], true);
        assert!(tested["result"].as_str().unwrap().contains("wave Bob"), "{tested}");
        let alias = json!({ "pattern": "k", "replacement": "kill $1" });
        assert_eq!(call("rules.test_alias", &json!({ "rule": alias, "text": "k rat" })).unwrap()["lines"], json!(["kill rat"]));
        assert_eq!(call("rules.test_alias", &json!({ "rule": alias, "text": "look" })).unwrap()["matched"], false);
    }

    #[test]
    fn lines_count_utf16_and_find_links() {
        let line = call("ansi.parse", &json!({ "text": "\u{1b}[31m😀é\u{1b}[0m see https://example.org now" })).unwrap();
        assert_eq!(line["text"], "😀é see https://example.org now");
        let spans = line["spans"].as_array().unwrap();
        // The emoji is two UTF-16 units and the é one.
        assert_eq!(spans[0]["len"], 3);
        assert_eq!(spans[0]["fg"], "#cd3131");
        let link = spans.iter().find(|s| s.get("link").is_some()).expect("a link span");
        assert_eq!(link["link"], "https://example.org");
        let total: u64 = spans.iter().map(|s| s["len"].as_u64().unwrap()).sum();
        assert_eq!(total as usize, "😀é see https://example.org now".encode_utf16().count());
    }

    // The only test that uses sessions: they belong to the first thread that asks for one.
    #[test]
    fn a_session_runs_commands_and_closes() {
        let opened = call("session.open", &json!({ "world": { "name": "", "host": "127.0.0.1", "port": 9 } })).unwrap();
        let id = opened["id"].as_u64().unwrap();
        assert_eq!(opened["title"], "127.0.0.1:9");
        let reply = call("session.submit", &json!({ "id": id, "text": "/echo hello" })).unwrap();
        let outputs = reply["outputs"].as_array().unwrap();
        assert!(outputs.iter().any(|o| o["t"] == "line" && o["plain"] == "hello" || o["t"] == "notice" && o["text"] == "hello" || o["t"] == "echo"), "{outputs:?}");
        let key = call("session.macro", &json!({ "id": id, "key": "F5" })).unwrap();
        assert_eq!(key["handled"], false);
        // Shortcuts (read only: "keys.set" would save the user's config).
        let actions = call("keys.actions", &Value::Null).unwrap();
        let find = actions.as_array().unwrap().iter().find(|a| a["id"] == "find").expect("find");
        assert_eq!(find["scope"], "window");
        let key = if cfg!(target_os = "macos") { json!({ "key": "f", "cmd": true }) } else { json!({ "key": "f", "ctrl": true }) };
        if find["changed"] == false {
            assert_eq!(call("keys.match", &key).unwrap(), "find");
        }
        let mut editor = key.clone();
        editor["scope"] = "editor".into();
        assert!(call("keys.match", &editor).unwrap() != "find");
        assert_eq!(call("session.poll", &json!({ "id": id })).unwrap()["more"], false);
        // Without "agents", /agent is an unknown command.
        let reply = call("session.submit", &json!({ "id": id, "text": "/agent list" })).unwrap();
        assert!(reply["outputs"][0]["text"].as_str().unwrap().starts_with("Unknown command /agent"), "{reply}");
        call("session.close", &json!({ "id": id })).unwrap();
        assert!(call("session.poll", &json!({ "id": id })).is_err());

        // With it, the bridge handles /agent; starting one is the app's job (agent_start).
        let opened = call("session.open", &json!({ "world": { "name": "", "host": "127.0.0.1", "port": 9 }, "agents": true })).unwrap();
        let id = opened["id"].as_u64().unwrap();
        let notice = |text: &str| {
            let reply = call("session.submit", &json!({ "id": id, "text": text })).unwrap();
            reply["outputs"].as_array().unwrap().iter().filter(|o| o["t"] == "notice").map(|o| o["text"].as_str().unwrap().to_string()).collect::<Vec<_>>().join("\n")
        };
        assert!(notice("/agent list").starts_with("Agents: claude, codex, agy, grok"));
        assert_eq!(notice("/agent stop"), "There's no agent working on this world");
        assert!(notice("/agent nobody").starts_with("There's no agent called nobody"));
        // A report about an agent that isn't running is ignored.
        let ended = call("agent.ended", &json!({ "id": id, "token": "old" })).unwrap();
        assert_eq!(ended["outputs"], json!([]));
        call("session.close", &json!({ "id": id })).unwrap();
    }

    #[test]
    fn ffi_handles_null_method() {
        let ptr = unsafe { familiar_call(std::ptr::null(), std::ptr::null()) };
        assert!(!ptr.is_null());
        let text = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
        unsafe { familiar_free(ptr) };
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["error"], "null method");
    }
}
