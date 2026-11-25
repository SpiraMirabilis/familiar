//! LLM agents (Claude Code, Codex, Antigravity, Grok Build, ...) working on a world beside it,
//! for every Familiar frontend.
//!
//! - [`AgentServer`] serves the world's tools to agents over the Model Context Protocol.
//! - [`choose`] picks the agent `/agent` means, and [`Agents::launch`] prepares its folder and
//!   says how to run it. Running it, in a terminal, is the frontend's job.
//! - [`Agents`] answers the agents' requests from the worlds, which the frontend owns: it
//!   implements [`World`] for its sessions and passes each [`AgentRequest`] to
//!   [`Agents::answer`], and each `Output::AgentResult` and `Output::AgentTools` to
//!   [`Agents::result`] and [`Agents::tools_changed`].

mod server;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use familiar_config::{Agent, Settings};
use familiar_engine::AgentTool;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

pub use server::{AgentRequest, AgentServer, Ask, WORLD_OUTPUT};

/// The most lines `world_output` returns.
pub const MAX_OUTPUT_LINES: usize = 1000;

/// Written into a new agent's folder, for it to read and keep notes in.
const NOTES: &str = "# {world}

You are working on the MUD/MOO world \"{world}\" from inside the Familiar client, using the tools of the `familiar` MCP server. \
They act as the character logged in there, with that character's permissions, on the live world: changes take effect at once, for everyone there.

- Read before you write: look at a verb's code, and at what it inherits, before changing it.
- `world_output` shows what the player sees in the world's window.

Keep notes about this world below: its objects, its conventions, and what you learn about it.
";

/// A world an agent works on, as the frontend has it.
pub trait World {
    /// The tools the world offers, or `None` if it doesn't speak dns-com-familiar-agent.
    fn agent_tools(&self) -> Option<&[AgentTool]>;
    /// Sends a call to the world: its id, which the result comes back with.
    fn agent_call(&mut self, tool: &str, args: &Value) -> Result<u64, String>;
    /// The last `n` lines of the world's window, as plain text, oldest first.
    fn last_lines(&self, n: usize) -> Vec<String>;
}

/// The agent `/agent <wanted>` means: the one with that name, or for no name, the first
/// installed one in the order the settings give.
pub fn choose(settings: &Settings, wanted: &str) -> Result<Agent, String> {
    choose_in(settings, wanted, None)
}

/// [`choose`], looking for programs in `path` (as in PATH) instead of this process's PATH.
fn choose_in(settings: &Settings, wanted: &str, path: Option<&str>) -> Result<Agent, String> {
    let agents = settings.agents();
    let wanted = wanted.trim();
    if wanted.is_empty() {
        return first_installed(&agents, &settings.agent_order, path).cloned().ok_or_else(|| {
            format!("None of the agents is installed ({}); /agent <name> runs one by name", settings.agent_order.join(", "))
        });
    }
    agents
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case(wanted))
        .cloned()
        .ok_or_else(|| format!("There's no agent called {wanted}; there's {}", names(settings)))
}

/// The names of the agents there are, for messages.
pub fn names(settings: &Settings) -> String {
    settings.agents().iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ")
}

/// How to run an agent: its program, in its folder, with the address of its MCP endpoint.
#[derive(Debug, Clone)]
pub struct Launch {
    /// The agent's name, for a title.
    pub name: String,
    /// Its endpoint's token: pass it to [`Agents::close`] when the agent stops.
    pub token: String,
    pub url: String,
    pub program: String,
    pub args: Vec<String>,
    /// The folder it runs in, with its notes about the world.
    pub dir: PathBuf,
    /// Set in its environment, besides the frontend's own.
    pub env: Vec<(String, String)>,
}

impl Launch {
    pub fn command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(&self.program);
        command.args(&self.args).current_dir(&self.dir);
        for (key, value) in &self.env {
            command.env(key, value);
        }
        command
    }
}

/// The agents of one frontend: the MCP server, started with the first one, and the calls they
/// are waiting on.
pub struct Agents {
    requests: mpsc::UnboundedSender<AgentRequest>,
    runtime: Option<tokio::runtime::Handle>,
    server: Option<AgentServer>,
    /// Calls sent to a world, by session and call id.
    calls: HashMap<(u64, u64), oneshot::Sender<(bool, String)>>,
    /// Where to look for agents' programs, if not this process's PATH.
    search_path: Option<String>,
}

impl Agents {
    /// Requests from agents go to `requests`, for the frontend to pass to [`Agents::answer`].
    /// The server runs on `runtime`, or if that's `None`, on the runtime the first
    /// [`Agents::launch`] is called in.
    pub fn new(requests: mpsc::UnboundedSender<AgentRequest>, runtime: Option<tokio::runtime::Handle>) -> Self {
        Self { requests, runtime, server: None, calls: HashMap::new(), search_path: None }
    }

    /// Looks for agents' programs in `path` (as in PATH), and runs them with it as their PATH.
    /// For apps that don't inherit the user's shell's PATH, such as a Mac app opened from the
    /// Finder: see [`login_shell_path`].
    pub fn set_search_path(&mut self, path: Option<String>) {
        self.search_path = path;
    }

    /// [`choose`], looking for programs where [`Agents::set_search_path`] said.
    pub fn choose(&self, settings: &Settings, wanted: &str) -> Result<Agent, String> {
        choose_in(settings, wanted, self.search_path.as_deref())
    }

    /// Gets `agent` ready to work on `world` (session `session`): starts the server if it
    /// isn't running, opens an endpoint, and makes the agent's folder. On success the caller
    /// runs the [`Launch`], and closes its token if that fails.
    pub fn launch(&mut self, agent: &Agent, session: u64, world: &str) -> Result<Launch, String> {
        let Some(program) = agent.command.first().filter(|p| !p.is_empty()) else {
            return Err(format!("The agent {} has no command", agent.name));
        };
        // A full path, for frontends that start programs without searching PATH.
        let program = find_program(program, self.search_path.as_deref()).map_or_else(|| program.clone(), |p| p.to_string_lossy().into_owned());
        if self.server.is_none() {
            let runtime = match &self.runtime {
                Some(runtime) => runtime.clone(),
                None => tokio::runtime::Handle::try_current().map_err(|_| "There's no runtime to run the MCP server for agents on".to_string())?,
            };
            let server = AgentServer::start(self.requests.clone(), &runtime).map_err(|e| format!("Couldn't start the MCP server for agents: {e}"))?;
            self.server = Some(server);
        }
        let dir = familiar_config::agent_dir(world);
        prepare_folder(&dir, world).map_err(|e| format!("Couldn't make the agent's folder {}: {e}", dir.display()))?;
        let Some(server) = &self.server else { unreachable!("started above") };
        let (token, url) = server.open(session);
        if let Err(e) = write_files(&dir, &agent.files_for(&url, world)) {
            server.close(&token);
            return Err(format!("Couldn't write {}'s files in {}: {e}", agent.name, dir.display()));
        }
        let args = agent.command_for(&url, world).split_off(1);
        let mut env = vec![("FAMILIAR_MCP_URL".to_string(), url.clone()), ("FAMILIAR_WORLD".to_string(), world.to_string())];
        if let Some(path) = &self.search_path {
            env.push(("PATH".to_string(), path.clone()));
        }
        Ok(Launch { name: agent.name.clone(), token, url, program, args, dir, env })
    }

    /// Stops serving an agent's endpoint, when the agent stops.
    pub fn close(&self, token: &str) {
        if let Some(server) = &self.server {
            server.close(token);
        }
    }

    /// Answers an agent's request of `world`, the session `request.session` (`None` if it's
    /// gone). A call's answer waits for [`Agents::result`].
    pub fn answer<W: World + ?Sized>(&mut self, request: AgentRequest, world: Option<&mut W>) {
        match request.ask {
            Ask::Tools(reply) => {
                let tools = world.and_then(|w| w.agent_tools().map(<[_]>::to_vec)).unwrap_or_default();
                let _ = reply.send(tools);
            }
            Ask::Call { tool, args, reply } => {
                let Some(world) = world else {
                    let _ = reply.send((false, "The world's tab was closed".into()));
                    return;
                };
                if tool == WORLD_OUTPUT {
                    let n = args.get("lines").and_then(Value::as_u64).map_or(50, |n| n as usize).clamp(1, MAX_OUTPUT_LINES);
                    let _ = reply.send((true, world.last_lines(n).join("\n")));
                    return;
                }
                match world.agent_call(&tool, &args) {
                    Ok(call) => {
                        self.calls.insert((request.session, call), reply);
                    }
                    Err(e) => {
                        let _ = reply.send((false, e));
                    }
                }
            }
        }
    }

    /// A world answered call `call` (`Output::AgentResult`).
    pub fn result(&mut self, session: u64, call: u64, ok: bool, content: String) {
        if let Some(reply) = self.calls.remove(&(session, call)) {
            let _ = reply.send((ok, content));
        }
    }

    /// A world's tools changed (`Output::AgentTools`): tells its agents.
    pub fn tools_changed(&self, session: u64) {
        if let Some(server) = &self.server {
            server.tools_changed(session);
        }
    }

    /// A world was closed: the calls still waiting on it fail.
    pub fn forget(&mut self, session: u64) {
        self.calls.retain(|(s, _), _| *s != session);
    }
}

/// Makes the folder an agent runs in, with notes about the world if it has none yet.
fn prepare_folder(dir: &Path, world: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let notes = dir.join("AGENTS.md");
    if !notes.exists() {
        std::fs::write(&notes, NOTES.replace("{world}", world))?;
    }
    // Claude Code reads CLAUDE.md, not AGENTS.md.
    let claude = dir.join("CLAUDE.md");
    if !claude.exists() {
        std::fs::write(&claude, "@AGENTS.md\n")?;
    }
    Ok(())
}

/// The first agent in `order` whose program is installed.
fn first_installed<'a>(agents: &'a [Agent], order: &[String], path: Option<&str>) -> Option<&'a Agent> {
    order
        .iter()
        .filter_map(|name| agents.iter().find(|a| a.name.eq_ignore_ascii_case(name)))
        .find(|a| a.command.first().is_some_and(|program| find_program(program, path).is_some()))
}

/// Whether `program` can be run: a path to a file, or the name of one on the PATH.
pub fn installed(program: &str) -> bool {
    find_program(program, None).is_some()
}

/// Where `program` is: itself if it's a path to a file, or the first file of that name in
/// `path` (as in PATH; this process's PATH if `None`).
fn find_program(program: &str, path: Option<&str>) -> Option<PathBuf> {
    let program = Path::new(program);
    if program.components().count() > 1 {
        return program.is_file().then(|| program.to_path_buf());
    }
    let path = match path {
        Some(path) => path.into(),
        None => std::env::var_os("PATH")?,
    };
    std::env::split_paths(&path).find_map(|dir| {
        let file = dir.join(program);
        if file.is_file() {
            return Some(file);
        }
        if cfg!(windows) {
            return ["exe", "cmd", "bat"].iter().map(|ext| file.with_extension(ext)).find(|f| f.is_file());
        }
        None
    })
}

/// The PATH the user's shell sets up, which a Mac app opened from the Finder doesn't get. The
/// shell runs as an interactive login shell, so it reads the files where installers add to
/// PATH (zsh's .zshrc is read only by interactive shells). `None` if the shell can't be run or
/// takes longer than a few seconds. Unix only.
pub fn login_shell_path() -> Option<String> {
    #[cfg(unix)]
    {
        use std::io::Read;
        use std::process::{Command, Stdio};
        const MARK: &str = "__FAMILIAR_PATH__";
        let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into());
        let mut child = Command::new(shell)
            .args(["-i", "-l", "-c", &format!("printf '{MARK}%s{MARK}' \"$PATH\"")])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        // Interactive shells can be slow to start (plugins, prompts).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while child.try_wait().ok()?.is_none() {
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let mut out = String::new();
        child.stdout.take()?.read_to_string(&mut out).ok()?;
        let path = out.split(MARK).nth(1)?;
        (!path.is_empty()).then(|| path.to_string())
    }
    #[cfg(not(unix))]
    None
}

/// Writes an agent's files (its MCP configuration, say) into its folder. Paths must stay
/// inside the folder.
fn write_files(dir: &Path, files: &[(String, String)]) -> std::io::Result<()> {
    use std::path::Component;
    for (path, text) in files {
        let relative = Path::new(path);
        if !relative.components().all(|c| matches!(c, Component::Normal(_))) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{path} isn't a path inside the folder")));
        }
        let file = dir.join(relative);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(file, text)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(name: &str, program: &str) -> Agent {
        Agent { name: name.into(), command: vec![program.into()], files: Default::default() }
    }

    #[test]
    fn the_first_installed_agent_in_order_runs() {
        let dir = std::env::temp_dir().join(format!("familiar-agents-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let there = dir.join("there");
        std::fs::write(&there, "").unwrap();
        let there = there.to_string_lossy().to_string();
        let missing = dir.join("missing").to_string_lossy().to_string();
        let agents = [agent("a", &missing), agent("b", &there), agent("c", &there)];
        let order = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(first_installed(&agents, &order(&["a", "b", "c"]), None).map(|a| a.name.as_str()), Some("b"));
        assert_eq!(first_installed(&agents, &order(&["C", "b"]), None).map(|a| a.name.as_str()), Some("c"));
        assert!(first_installed(&agents, &order(&["a", "nobody"]), None).is_none());

        let mut settings = Settings { agents: agents.to_vec(), agent_order: order(&["a", "c"]), ..Settings::default() };
        assert_eq!(choose(&settings, "").unwrap().name, "c");
        assert_eq!(choose(&settings, " A ").unwrap().name, "a");
        assert!(choose(&settings, "nobody").unwrap_err().contains("there's claude, codex, agy, grok, a, b, c"));
        // By name, in a search path of our own.
        let named = [agent("d", "there")];
        let dir_path = dir.to_string_lossy().to_string();
        assert!(first_installed(&named, &order(&["d"]), None).is_none());
        assert_eq!(first_installed(&named, &order(&["d"]), Some(&dir_path)).map(|a| a.name.as_str()), Some("d"));
        assert_eq!(find_program("there", Some(&dir_path)), Some(dir.join("there")));
        settings.agent_order = order(&["a"]);
        assert!(choose(&settings, "").unwrap_err().starts_with("None of the agents is installed (a)"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_login_shell_has_a_path() {
        let path = login_shell_path().expect("a PATH from the login shell");
        assert!(path.split(':').any(|dir| dir.starts_with('/')), "{path}");
    }

    #[test]
    fn agent_files_stay_in_the_folder() {
        let dir = std::env::temp_dir().join(format!("familiar-agent-files-{}", std::process::id()));
        write_files(&dir, &[(".agents/mcp_config.json".into(), "{}".into())]).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(".agents/mcp_config.json")).unwrap(), "{}");
        assert!(write_files(&dir, &[("../escape".into(), "".into())]).is_err());
        assert!(write_files(&dir, &[("/etc/escape".into(), "".into())]).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A world with one tool, that records its calls.
    struct FakeWorld {
        tools: Vec<AgentTool>,
        sent: Vec<(String, Value)>,
    }

    impl World for FakeWorld {
        fn agent_tools(&self) -> Option<&[AgentTool]> {
            Some(&self.tools)
        }
        fn agent_call(&mut self, tool: &str, args: &Value) -> Result<u64, String> {
            if tool != "look" {
                return Err(format!("No tool {tool}"));
            }
            self.sent.push((tool.into(), args.clone()));
            Ok(self.sent.len() as u64)
        }
        fn last_lines(&self, n: usize) -> Vec<String> {
            let lines: Vec<String> = (1..=3).map(|i| format!("line {i}")).collect();
            lines[lines.len().saturating_sub(n)..].to_vec()
        }
    }

    fn call(tool: &str, args: Value) -> (AgentRequest, oneshot::Receiver<(bool, String)>) {
        let (reply, answer) = oneshot::channel();
        (AgentRequest { session: 4, ask: Ask::Call { tool: tool.into(), args, reply } }, answer)
    }

    #[test]
    fn requests_are_answered_from_the_world() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut agents = Agents::new(tx, None);
        let mut world = FakeWorld { tools: vec![AgentTool::parse(r#"{"name":"look"}"#).unwrap()], sent: Vec::new() };

        let (reply, mut answer) = oneshot::channel();
        agents.answer(AgentRequest { session: 4, ask: Ask::Tools(reply) }, Some(&mut world));
        assert_eq!(answer.try_recv().unwrap()[0].name, "look");

        let (request, mut answer) = call(WORLD_OUTPUT, serde_json::json!({"lines": 2}));
        agents.answer(request, Some(&mut world));
        assert_eq!(answer.try_recv().unwrap(), (true, "line 2\nline 3".into()));

        // A call waits for the world's result.
        let (request, mut answer) = call("look", serde_json::json!({"at": "me"}));
        agents.answer(request, Some(&mut world));
        assert!(answer.try_recv().is_err());
        agents.result(4, 99, true, "not this one".into());
        agents.result(4, 1, true, "You see yourself".into());
        assert_eq!(answer.try_recv().unwrap(), (true, "You see yourself".into()));

        let (request, mut answer) = call("nope", Value::Null);
        agents.answer(request, Some(&mut world));
        assert_eq!(answer.try_recv().unwrap(), (false, "No tool nope".into()));

        // When the world goes, so do its calls; and requests of a world that's gone fail.
        let (request, mut answer) = call("look", Value::Null);
        agents.answer(request, Some(&mut world));
        agents.forget(4);
        assert!(matches!(answer.try_recv(), Err(oneshot::error::TryRecvError::Closed)));
        let (request, mut answer) = call("look", Value::Null);
        agents.answer::<FakeWorld>(request, None);
        assert_eq!(answer.try_recv().unwrap(), (false, "The world's tab was closed".into()));
    }
}
