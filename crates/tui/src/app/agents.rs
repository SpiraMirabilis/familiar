//! LLM agents (Claude Code, Codex, ...) running beside a world, and the tool calls they make
//! on it through the MCP server in `crate::agent`.

use std::path::Path;

use super::*;
use crate::agent::{Ask, WORLD_OUTPUT};

/// The most lines `world_output` returns.
const MAX_OUTPUT_LINES: usize = 1000;

/// Written into a new agent's folder, for it to read and keep notes in.
const NOTES: &str = "# {world}

You are working on the MUD/MOO world \"{world}\" from inside the Familiar client, using the tools of the `familiar` MCP server. \
They act as the character logged in there, with that character's permissions, on the live world: changes take effect at once, for everyone there.

- Read before you write: look at a verb's code, and at what it inherits, before changing it.
- `world_output` shows what the player sees in the world's window.

Keep notes about this world below: its objects, its conventions, and what you learn about it.
";

impl App {
    /// `/agent [name]`, `/agent stop`, `/agent list`.
    pub(super) fn agent_command(&mut self, args: &str) {
        let agents = self.config.settings.agents();
        match args.trim() {
            "list" => {
                let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
                self.toast(&format!("Agents: {} ([[settings.agent]] in the config adds more)", names.join(", ")));
            }
            "stop" => self.stop_agent(),
            "" => match first_installed(&agents, &self.config.settings.agent_order) {
                Some(agent) => self.start_agent(agent.clone()),
                None => {
                    let order = self.config.settings.agent_order.join(", ");
                    self.toast(&format!("None of the agents is installed ({order}); /agent <name> runs one by name"));
                }
            },
            wanted => match agents.iter().find(|a| a.name.eq_ignore_ascii_case(wanted)) {
                Some(agent) => self.start_agent(agent.clone()),
                None => {
                    let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
                    self.toast(&format!("There's no agent called {wanted}; there's {}", names.join(", ")));
                }
            },
        }
    }

    /// The session of the world tab on screen.
    fn current_world(&self) -> Option<u64> {
        match self.tabs.get(self.active)? {
            Tab::Main(id) | Tab::Puppet(id, _) => Some(*id),
            Tab::Editor(id) => match &self.editors.iter().find(|e| e.id == *id)?.job {
                External::Agent { session, .. } => Some(*session),
                _ => None,
            },
        }
    }

    fn agent_for(&self, session: u64) -> Option<u64> {
        self.editors.iter().find(|e| matches!(&e.job, External::Agent { session: s, .. } if *s == session)).map(|e| e.id)
    }

    fn start_agent(&mut self, agent: familiar_config::Agent) {
        let Some(session) = self.current_world() else { return self.toast("Switch to a world first: an agent works on the world it's opened beside") };
        let Some(idx) = self.session_index(session) else { return };
        // One per world: go to the one that's running.
        if let Some(id) = self.agent_for(session) {
            let docked = self.editors.iter().find(|e| e.id == id).is_some_and(|e| e.docked.is_some());
            let tab = if docked { self.tabs.iter().position(|t| *t == Tab::Main(session)) } else { self.tabs.iter().position(|t| *t == Tab::Editor(id)) };
            if let Some(editor) = self.editors.iter_mut().find(|e| e.id == id) {
                editor.focused = true;
            }
            if let Some(tab) = tab {
                self.switch_to(tab);
            }
            return self.toast("This world has an agent already (/agent stop stops it)");
        }
        let Some(program) = agent.command.first().filter(|p| !p.is_empty()) else {
            return self.toast(&format!("The agent {} has no command", agent.name));
        };
        if self.agent_server.is_none() {
            match AgentServer::start(self.agent_tx.clone()) {
                Ok(server) => self.agent_server = Some(server),
                Err(e) => return self.toast(&format!("Couldn't start the MCP server for agents: {e}")),
            }
        }
        let world = self.sessions[idx].engine.world().name.clone();
        let world = if world.is_empty() { self.sessions[idx].title() } else { world };
        let dir = familiar_config::agent_dir(&world);
        if let Err(e) = prepare_folder(&dir, &world) {
            return self.toast(&format!("Couldn't make the agent's folder {}: {e}", dir.display()));
        }
        let Some(server) = &self.agent_server else { return };
        let (token, url) = server.open(session);
        if let Err(e) = write_files(&dir, &agent.files_for(&url, &world)) {
            server.close(&token);
            return self.toast(&format!("Couldn't write {}'s files in {}: {e}", agent.name, dir.display()));
        }
        let words = agent.command_for(&url, &world);
        let mut command = std::process::Command::new(program);
        command.args(&words[1..]).current_dir(&dir).env("FAMILIAR_MCP_URL", &url).env("FAMILIAR_WORLD", &world);
        let job = External::Agent { session };
        if let Err((e, _)) = self.open_pane(&job, &command, agent.name.clone(), None, Some(token.clone()), true) {
            if let Some(server) = &self.agent_server {
                server.close(&token);
            }
            return self.toast(&format!("Couldn't run {program}: {e}"));
        }
        if self.sessions[idx].engine.agent_tools().is_none() {
            self.toast("This world offers no tools (its MOO needs the dns-com-familiar-agent package), so the agent can only read its output");
        }
    }

    /// Stops the agent working on the world on screen.
    fn stop_agent(&mut self) {
        let Some(id) = self.current_world().and_then(|s| self.agent_for(s)) else { return self.toast("There's no agent working on this world") };
        // Dropping it stops the program.
        if let Some(editor) = self.remove_editor(id) {
            let message = editor.discard();
            self.toast(&message);
        }
        if self.tabs.is_empty() {
            self.open_connect_picker();
        }
    }

    /// Answers an agent's request of the world it works on.
    pub fn on_agent(&mut self, request: AgentRequest) {
        let idx = self.session_index(request.session);
        match request.ask {
            Ask::Tools(reply) => {
                let tools = idx.and_then(|i| self.sessions[i].engine.agent_tools()).map(<[_]>::to_vec).unwrap_or_default();
                let _ = reply.send(tools);
            }
            Ask::Call { tool, args, reply } => {
                let Some(idx) = idx else {
                    let _ = reply.send((false, "The world's tab was closed".into()));
                    return;
                };
                if tool == WORLD_OUTPUT {
                    let n = args.get("lines").and_then(serde_json::Value::as_u64).map_or(50, |n| n as usize).clamp(1, MAX_OUTPUT_LINES);
                    let _ = reply.send((true, self.sessions[idx].output.last_lines(n).join("\n")));
                    return;
                }
                match self.sessions[idx].engine.agent_call(&tool, &args) {
                    Ok(call) => {
                        self.agent_calls.insert((request.session, call), reply);
                    }
                    Err(e) => {
                        let _ = reply.send((false, e));
                    }
                }
            }
        }
    }
}

/// Makes the folder an agent runs in, with notes about the world if it has none yet.
fn prepare_folder(dir: &std::path::Path, world: &str) -> std::io::Result<()> {
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
fn first_installed<'a>(agents: &'a [familiar_config::Agent], order: &[String]) -> Option<&'a familiar_config::Agent> {
    order
        .iter()
        .filter_map(|name| agents.iter().find(|a| a.name.eq_ignore_ascii_case(name)))
        .find(|a| a.command.first().is_some_and(|program| installed(program)))
}

/// Whether `program` can be run: a path to a file, or the name of one on the PATH.
fn installed(program: &str) -> bool {
    let program = Path::new(program);
    if program.components().count() > 1 {
        return program.is_file();
    }
    let Some(path) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&path).any(|dir| {
        let file = dir.join(program);
        file.is_file() || (cfg!(windows) && ["exe", "cmd", "bat"].iter().any(|ext| file.with_extension(ext).is_file()))
    })
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

    fn agent(name: &str, program: &str) -> familiar_config::Agent {
        familiar_config::Agent { name: name.into(), command: vec![program.into()], files: Default::default() }
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
        assert_eq!(first_installed(&agents, &order(&["a", "b", "c"])).map(|a| a.name.as_str()), Some("b"));
        assert_eq!(first_installed(&agents, &order(&["C", "b"])).map(|a| a.name.as_str()), Some("c"));
        assert!(first_installed(&agents, &order(&["a", "nobody"])).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
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
}
