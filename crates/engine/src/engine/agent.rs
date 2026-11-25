//! Tools a MOO offers an LLM agent (dns-com-familiar-agent): calling them, and their results.

use super::*;
use crate::mcp::{AGENT, AgentTool, Show};

/// A call waiting for its result.
#[derive(Debug)]
pub(super) struct AgentCall {
    tool: String,
    /// What it was called with, for the line in the world's window.
    summary: String,
    show: Show,
    deadline: Instant,
    timeout: u64,
}

/// A short description of a call's arguments: `obj=#123 verb=look code=(12 lines)`.
fn summarize(args: &serde_json::Value) -> String {
    let Some(args) = args.as_object() else { return String::new() };
    let mut parts = Vec::new();
    for (key, value) in args {
        let text = match value {
            serde_json::Value::String(s) if s.contains('\n') => format!("({} lines)", s.lines().count()),
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let text = if text.chars().count() > 40 { format!("{}…", text.chars().take(39).collect::<String>()) } else { text };
        parts.push(format!("{key}={text}"));
    }
    parts.join(" ")
}

impl Engine {
    /// The tools the server offers, or None if it doesn't speak dns-com-familiar-agent.
    pub fn agent_tools(&self) -> Option<&[AgentTool]> {
        (self.world.mcp && self.mcp.server_supports(AGENT)).then_some(self.agent_tools.as_slice())
    }

    /// Calls a tool on the server. Its result comes later as [`Output::AgentResult`] with the
    /// returned id, or an error result when it times out or the connection drops.
    pub fn agent_call(&mut self, tool: &str, args: &serde_json::Value) -> Result<u64, String> {
        if self.handle.is_none() {
            return Err("Not connected to the world".into());
        }
        let Some(tools) = self.agent_tools() else {
            return Err(format!("This world doesn't offer agent tools (it doesn't support the {AGENT} MCP package)"));
        };
        let Some(spec) = tools.iter().find(|t| t.name == tool) else {
            return Err(format!("The world offers no tool called {tool}"));
        };
        let (show, timeout) = (spec.show, spec.timeout);
        let id = self.next_agent_call;
        self.next_agent_call += 1;
        let json = serde_json::to_string(args).map_err(|e| e.to_string())?;
        for line in self.mcp.agent_call(id, tool, &json) {
            if !self.send_raw(&line) {
                return Err("Couldn't send to the world".into());
            }
        }
        self.agent_calls.insert(
            id,
            AgentCall {
                tool: tool.to_string(),
                summary: summarize(args),
                show,
                deadline: Instant::now() + Duration::from_secs(timeout),
                timeout,
            },
        );
        Ok(id)
    }

    /// Reports a call's result, if it's still waiting for one.
    pub(super) fn agent_result(&mut self, id: u64, ok: bool, content: String, out: &mut Vec<Output>) {
        let Some(call) = self.agent_calls.remove(&id) else { return };
        if call.show != Show::None {
            let outcome = if ok {
                "ok".to_string()
            } else {
                format!("failed: {}", content.lines().find(|l| !l.trim().is_empty()).unwrap_or("no reason given"))
            };
            let args = if call.summary.is_empty() { String::new() } else { format!(" {}", call.summary) };
            out.push(Output::Notice(format!("[agent] {}{args} → {outcome}", call.tool)));
            if call.show == Show::Full {
                out.extend(content.lines().map(|l| Output::Notice(format!("[agent]   {l}"))));
            }
        }
        out.push(Output::AgentResult { id, ok, content });
    }

    /// Fails calls whose time is up.
    pub(super) fn agent_timeouts(&mut self, now: Instant, out: &mut Vec<Output>) {
        let due: Vec<u64> = self.agent_calls.iter().filter(|(_, c)| c.deadline <= now).map(|(id, _)| *id).collect();
        for id in due {
            let timeout = self.agent_calls[&id].timeout;
            self.agent_result(id, false, format!("The world didn't answer within {timeout} seconds"), out);
        }
    }

    pub(super) fn agent_deadline(&self) -> Option<Instant> {
        self.agent_calls.values().map(|c| c.deadline).min()
    }

    /// The connection dropped: no results are coming, and the tools may differ next time.
    pub(super) fn agent_disconnected(&mut self, out: &mut Vec<Output>) {
        let ids: Vec<u64> = self.agent_calls.keys().copied().collect();
        for id in ids {
            self.agent_result(id, false, "The connection to the world closed".into(), out);
        }
        if !self.agent_tools.is_empty() {
            self.agent_tools.clear();
            out.push(Output::AgentTools(Vec::new()));
        }
    }
}
