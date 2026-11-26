//! Tools a MOO offers an LLM agent (dns-com-familiar-agent): calling them, and their results.

use std::collections::VecDeque;

use super::*;
use crate::mcp::{AGENT, AgentTool, Show};

/// The most lines kept of what the world said, for agents' commands.
const HEARD_LINES: usize = 2000;

/// Commands longer than this many lines are shown in the world's window as their first line.
const SHOWN_LINES: usize = 3;

/// What the world said lately, as plain text, for agents that send it commands.
#[derive(Debug, Default)]
pub(super) struct Heard {
    lines: VecDeque<String>,
    /// How many lines have been heard in all: a line's number is its place in that count.
    total: u64,
    /// A prompt not yet ended by a newline.
    partial: Option<String>,
    /// What to show in the world's window about commands agents sent, at the next tick.
    shown: Vec<Output>,
    shown_since: Option<Instant>,
}

/// What the world said since a mark: see [`Engine::agent_heard`]. Lines are numbered from the
/// first the world said; a mark is the number of the next line to come.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentHeard {
    /// The lines, oldest first. The last is number `next - 1`.
    pub lines: Vec<String>,
    /// A prompt after them that hasn't ended yet.
    pub partial: Option<String>,
    /// Lines that came after the mark but are no longer kept.
    pub dropped: u64,
    /// The mark for what comes next.
    pub next: u64,
}

impl AgentHeard {
    /// The lines from number `mark` on, and the prompt after them.
    pub fn since(&self, mark: u64) -> impl Iterator<Item = &String> {
        let first = self.next - self.lines.len() as u64;
        let skip = usize::try_from(mark.saturating_sub(first)).unwrap_or(usize::MAX);
        self.lines.iter().skip(skip).chain(&self.partial)
    }
}

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
    /// The tools the server offers, or None if it doesn't speak dns-com-familiar-agent (or the
    /// frontend can't run agents, so it wasn't offered).
    pub fn agent_tools(&self) -> Option<&[AgentTool]> {
        (self.agents_supported && self.world.mcp && self.mcp.server_supports(AGENT)).then_some(self.agent_tools.as_slice())
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

    /// Sends lines to the world for an agent, as they are (no aliases, scripts or client
    /// commands). Returns a mark for [`Engine::agent_heard`], to read what the world says next.
    pub fn agent_send(&mut self, lines: &[String]) -> Result<u64, String> {
        if self.handle.is_none() {
            return Err("Not connected to the world".into());
        }
        // A prompt shown before the command isn't part of its answer.
        self.heard.partial = None;
        let mark = self.heard.total;
        for line in lines {
            if !self.send_raw(line) {
                return Err("Couldn't send to the world".into());
            }
            self.heard.shown.push(Output::Sent(line.clone()));
        }
        let shown = match lines {
            [] => return Ok(mark),
            lines if lines.len() <= SHOWN_LINES => lines.iter().map(|l| format!("[agent] → {l}")).collect(),
            [first, rest @ ..] => vec![format!("[agent] → {first} (and {} more lines)", rest.len())],
        };
        self.heard.shown.extend(shown.into_iter().map(Output::Notice));
        self.heard.shown_since.get_or_insert_with(Instant::now);
        Ok(mark)
    }

    /// What the world said in the main window since `mark` (from [`Engine::agent_send`], or
    /// [`AgentHeard::next`]). `u64::MAX` reads nothing, for the mark.
    pub fn agent_heard(&self, mark: u64) -> AgentHeard {
        let next = self.heard.total;
        let first = next - self.heard.lines.len() as u64;
        let skip = usize::try_from(mark.saturating_sub(first)).unwrap_or(usize::MAX);
        let lines = self.heard.lines.iter().skip(skip).cloned().collect();
        let partial = if mark <= next { self.heard.partial.clone() } else { None };
        AgentHeard { lines, partial, dropped: first.saturating_sub(mark.min(next)), next }
    }

    /// Keeps what the world said in the main window, while agents can be run.
    pub(super) fn hear(&mut self, out: &[Output]) {
        if !self.agents_supported {
            return;
        }
        for output in out {
            match output {
                Output::Line { line, target: Target::Main } | Output::Prompt { line, terminated: true } => {
                    self.heard.partial = None;
                    if self.heard.lines.len() == HEARD_LINES {
                        self.heard.lines.pop_front();
                    }
                    self.heard.lines.push_back(line.text.clone());
                    self.heard.total += 1;
                }
                Output::Prompt { line, terminated: false } => self.heard.partial = Some(line.text.clone()),
                _ => {}
            }
        }
    }

    /// What to show about agents' commands, at the next tick.
    pub(super) fn agent_shown(&mut self, out: &mut Vec<Output>) {
        self.heard.shown_since = None;
        out.append(&mut self.heard.shown);
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
        self.agent_calls.values().map(|c| c.deadline).chain(self.heard.shown_since).min()
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
