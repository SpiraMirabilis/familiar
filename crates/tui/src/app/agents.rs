//! LLM agents (Claude Code, Codex, ...) running beside a world, in a terminal pane, and the
//! tool calls they make on it through the MCP server in `familiar_agent`.

use super::*;

impl App {
    /// `/agent [name]`, `/agent stop`, `/agent list`.
    pub(super) fn agent_command(&mut self, args: &str) {
        match args.trim() {
            "list" => {
                let names = familiar_agent::names(&self.config.settings);
                self.toast(&format!("Agents: {names} ([[settings.agent]] in the config adds more)"));
            }
            "stop" => self.stop_agent(),
            wanted => match familiar_agent::choose(&self.config.settings, wanted) {
                Ok(agent) => self.start_agent(agent),
                Err(e) => self.toast(&e),
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
        let world = self.sessions[idx].engine.world().name.clone();
        let world = if world.is_empty() { self.sessions[idx].title() } else { world };
        let launch = match self.agents.launch(&agent, session, &world) {
            Ok(launch) => launch,
            Err(e) => return self.toast(&e),
        };
        let job = External::Agent { session };
        if let Err((e, _)) = self.open_pane(&job, &launch.command(), launch.name.clone(), None, Some(launch.token.clone()), true) {
            self.agents.close(&launch.token);
            return self.toast(&format!("Couldn't run {}: {e}", launch.program));
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
        let world = idx.map(|i| &mut self.sessions[i]);
        self.agents.answer(request, world);
    }
}
