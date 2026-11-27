//! MCP 2.1, the MUD Client Protocol used by MOOs and MUCKs, carried in lines that start with
//! `#$#`. Spec: <https://www.moo.mud.org/mcp/mcp2.html>.
//!
//! Supported packages: mcp-negotiate 2.0, dns-org-mud-moo-simpleedit 1.0 (edit server text in
//! the client), dns-com-awns-status 1.0 (status line), dns-com-awns-ping 1.0 and
//! dns-com-vmoo-client 1.0, and dns-com-familiar-agent 1.0 (tools a MOO offers to an LLM agent;
//! see docs/agent-protocol.md).

use std::collections::HashMap;

const PACKAGES: [(&str, &str, &str); 6] = [
    ("mcp-negotiate", "2.0", "2.0"),
    ("dns-org-mud-moo-simpleedit", "1.0", "1.0"),
    ("dns-com-awns-status", "1.0", "1.0"),
    ("dns-com-awns-ping", "1.0", "1.0"),
    ("dns-com-vmoo-client", "1.0", "1.0"),
    (AGENT, "1.0", "1.0"),
];

/// The package a MOO offers tools to an LLM agent with.
pub const AGENT: &str = "dns-com-familiar-agent";

/// The longest piece of an agent call's arguments sent on one line.
const ARGS_CHUNK: usize = 1000;

/// How a tool call shows in the world's window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Show {
    /// Nothing.
    None,
    /// A line saying which tool was called and how it went.
    #[default]
    Summary,
    /// That, and what the tool returned.
    Full,
}

/// A tool the server offers through dns-com-familiar-agent.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentTool {
    pub name: String,
    pub description: String,
    /// A JSON Schema object for the tool's arguments.
    pub input_schema: serde_json::Value,
    pub show: Show,
    /// Seconds to wait for its result.
    pub timeout: u64,
}

impl AgentTool {
    pub const DEFAULT_TIMEOUT: u64 = 30;

    /// Reads one line of a dns-com-familiar-agent-tools message.
    pub fn parse(line: &str) -> Result<Self, String> {
        let v: serde_json::Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
        let name = v.get("name").and_then(|n| n.as_str()).filter(|n| !n.is_empty()).ok_or("no name")?;
        let mut input_schema = v.get("input_schema").cloned().unwrap_or_else(|| serde_json::json!({}));
        let Some(schema) = input_schema.as_object_mut() else { return Err(format!("{name}: input_schema isn't an object")) };
        schema.entry("type").or_insert_with(|| "object".into());
        let show = match v.get("show").and_then(|s| s.as_str()).unwrap_or("summary") {
            "none" => Show::None,
            "full" => Show::Full,
            _ => Show::Summary,
        };
        Ok(Self {
            name: name.to_string(),
            description: v.get("description").and_then(|d| d.as_str()).unwrap_or("").to_string(),
            input_schema,
            show,
            timeout: v
                .get("timeout")
                .and_then(|t| t.as_u64())
                .filter(|t| *t > 0)
                .map(|t| t.min(3600))
                .unwrap_or(Self::DEFAULT_TIMEOUT),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Message {
    /// Lowercase, e.g. "dns-org-mud-moo-simpleedit-content".
    pub name: String,
    pub values: Vec<(String, String)>,
    pub lists: Vec<(String, Vec<String>)>,
}

impl Message {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }

    pub fn list(&self, key: &str) -> Option<&[String]> {
        self.lists.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_slice())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum McpEvent {
    /// dns-com-awns-status: text for a status line.
    Status(String),
    /// dns-org-mud-moo-simpleedit: let the user edit text, then call [`Mcp::simpleedit_set`].
    Edit { reference: String, name: String, kind: String, content: String },
    Notice(String),
    /// dns-com-familiar-agent-tools: the tools the server offers now.
    AgentTools(Vec<AgentTool>),
    /// dns-com-familiar-agent-result: the answer to a call.
    AgentResult { id: u64, ok: bool, content: String },
}

/// What to do with a line after MCP has looked at it.
#[derive(Debug, Clone, PartialEq)]
pub enum Disposition {
    /// Not MCP. Display it.
    Display,
    /// An MCP message. Don't display it.
    Hide,
    /// A `#$"` quoted line. Display this text instead.
    Quoted(String),
}

const MAX_PENDING_MESSAGES: usize = 64;
const MAX_PENDING_LINES: usize = 50_000;

#[derive(Debug, Default)]
pub struct Mcp {
    /// Our authentication key, once the server has started MCP.
    key: Option<String>,
    /// True once the server has sent at least one authenticated message with our key.
    confirmed: bool,
    /// Multiline messages waiting for their end, by data tag.
    pending: HashMap<String, Message>,
    /// Packages the server says it supports, with their max version.
    server_packages: HashMap<String, String>,
    next_tag: u32,
    /// Whether to offer dns-com-familiar-agent: only frontends that can run agents do.
    agents: bool,
}

impl Mcp {
    /// A new MCP state; `agents` offers dns-com-familiar-agent.
    pub fn new(agents: bool) -> Self {
        Self { agents, ..Self::default() }
    }
}

fn random_key() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    format!("{:08x}", hasher.finish() as u32)
}

/// Parses "2.1" into (2, 1).
fn version(s: &str) -> Option<(u32, u32)> {
    let (major, minor) = s.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

fn is_simple(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || c == '_'
        || matches!(c, '!' | '#'..=')' | '+' | ',' | '-'..='/' | ';'..='@' | '[' | ']' | '^' | '`' | '{'..='~')
}

/// Splits `key: value key: "quoted value" list*: ""` into (key, value, is_list) triples.
fn parse_keyvals(mut s: &str) -> Option<Vec<(String, String, bool)>> {
    let mut out = Vec::new();
    loop {
        s = s.trim_start_matches(' ');
        if s.is_empty() {
            return Some(out);
        }
        let key_len = s.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))?;
        let mut key = &s[..key_len];
        s = &s[key_len..];
        let is_list = s.starts_with('*');
        if is_list {
            s = &s[1..];
        }
        s = s.strip_prefix(':')?;
        if !s.starts_with(' ') {
            return None;
        }
        s = s.trim_start_matches(' ');
        if key.is_empty() {
            return None;
        }
        let value = if let Some(rest) = s.strip_prefix('"') {
            let mut value = String::new();
            let mut chars = rest.char_indices();
            let end = loop {
                match chars.next()? {
                    (i, '"') => break i,
                    (_, '\\') => value.push(chars.next()?.1),
                    (_, c) => value.push(c),
                }
            };
            s = &rest[end + 1..];
            value
        } else {
            let len = s.find(|c: char| !is_simple(c)).unwrap_or(s.len());
            if len == 0 {
                return None;
            }
            let value = s[..len].to_string();
            s = &s[len..];
            value
        };
        key = key.trim();
        out.push((key.to_ascii_lowercase(), value, is_list));
    }
}

fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        if matches!(c, '\r' | '\n') {
            continue;
        }
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

impl Mcp {
    pub fn is_active(&self) -> bool {
        self.key.is_some()
    }

    /// The server has sent something with our key, so it received our answer.
    pub fn server_confirmed(&self) -> bool {
        self.confirmed || !self.server_packages.is_empty()
    }

    pub fn server_supports(&self, package: &str) -> bool {
        self.server_packages.contains_key(package)
    }

    /// Looks at a line from the server. Returns what to do with it and any lines to send back.
    pub fn on_line(&mut self, line: &str, events: &mut Vec<McpEvent>) -> (Disposition, Vec<String>) {
        if let Some(quoted) = line.strip_prefix("#$\"") {
            return if self.is_active() { (Disposition::Quoted(quoted.to_string()), Vec::new()) } else { (Disposition::Display, Vec::new()) };
        }
        let Some(body) = line.strip_prefix("#$#") else { return (Disposition::Display, Vec::new()) };

        // The server starts MCP with "#$#mcp version: 2.1 to: 2.1". Sent again, it's a new
        // session (the MOO restarted MCP, e.g. after a reconnection was redirected), so answer
        // again with a fresh key.
        if let Some(rest) = body.strip_prefix("mcp ")
            && !rest.contains("authentication-key")
        {
            let Some(kv) = parse_keyvals(rest) else { return (Disposition::Display, Vec::new()) };
            let get = |k: &str| kv.iter().find(|(key, _, _)| key == k).map(|(_, v, _)| v.as_str());
            let ours = (2, 1);
            let supported = get("version").and_then(version).is_some_and(|v| v <= ours)
                && get("to").and_then(version).is_some_and(|v| v >= ours);
            if !supported {
                events.push(McpEvent::Notice("The server offered an MCP version this client doesn't support".into()));
                return (Disposition::Hide, Vec::new());
            }
            let key = random_key();
            let mut out = vec![format!("#$#mcp authentication-key: {key} version: 2.1 to: 2.1")];
            self.key = Some(key);
            self.confirmed = false;
            self.pending.clear();
            self.server_packages.clear();
            for (name, min, max) in PACKAGES {
                if name == AGENT && !self.agents {
                    continue;
                }
                out.extend(self.format("mcp-negotiate-can", &[("package", name), ("min-version", min), ("max-version", max)], &[]));
            }
            out.extend(self.format("mcp-negotiate-end", &[], &[]));
            return (Disposition::Hide, out);
        }
        if self.key.is_none() {
            // Not started, so this is just text that happens to look like MCP.
            return (Disposition::Display, Vec::new());
        }

        let mut sends = Vec::new();
        if let Some(rest) = body.strip_prefix("* ") {
            // Multiline continuation: "#$#* <tag> <key>: <value>"
            if let Some((tag, rest)) = rest.split_once(' ') {
                let rest = rest.trim_start_matches(' ');
                // An empty value is "key: ", but the space may not survive the trip: take "key:" too.
                if let Some((key, value)) = rest.split_once(": ").or_else(|| Some((rest.strip_suffix(':')?, "")))
                    && let Some(message) = self.pending.get_mut(tag)
                    && let Some((_, list)) = message.lists.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key))
                    && list.len() < MAX_PENDING_LINES
                {
                    list.push(value.to_string());
                }
            }
        } else if let Some(tag) = body.strip_prefix(": ") {
            // Multiline end: "#$#: <tag>"
            if let Some(message) = self.pending.remove(tag.trim()) {
                self.dispatch(&message, events, &mut sends);
            }
        } else if let Some((name, rest)) = body.split_once(' ') {
            let (key, rest) = rest.split_once(' ').unwrap_or((rest, ""));
            if Some(key) != self.key.as_deref() {
                // Wrong key: someone (maybe another player) is trying to spoof the server.
                return (Disposition::Hide, Vec::new());
            }
            self.confirmed = true;
            let Some(kv) = parse_keyvals(rest) else {
                events.push(McpEvent::Notice(format!("Ignored an MCP message the client couldn't read: {name} {rest}")));
                return (Disposition::Hide, Vec::new());
            };
            let mut message = Message { name: name.to_ascii_lowercase(), ..Message::default() };
            for (k, v, is_list) in kv {
                if is_list {
                    message.lists.push((k, Vec::new()));
                } else {
                    message.values.push((k, v));
                }
            }
            if message.lists.is_empty() {
                self.dispatch(&message, events, &mut sends);
            } else if let Some(tag) = message.get("_data-tag").map(str::to_string)
                && (self.pending.contains_key(&tag) || self.pending.len() < MAX_PENDING_MESSAGES)
            {
                self.pending.insert(tag, message);
            }
        }
        (Disposition::Hide, sends)
    }

    fn dispatch(&mut self, m: &Message, events: &mut Vec<McpEvent>, sends: &mut Vec<String>) {
        match m.name.as_str() {
            "mcp-negotiate-can" => {
                if let Some(package) = m.get("package") {
                    self.server_packages.insert(package.to_ascii_lowercase(), m.get("max-version").unwrap_or("").to_string());
                }
            }
            "mcp-negotiate-end" => {
                if self.agents && self.server_supports(AGENT) {
                    sends.extend(self.format("dns-com-familiar-agent-list", &[], &[]));
                }
                if self.server_supports("dns-com-vmoo-client") {
                    let version = env!("CARGO_PKG_VERSION");
                    sends.extend(self.format(
                        "dns-com-vmoo-client-info",
                        &[("name", familiar_config::APP_NAME), ("text-version", version), ("internal-version", version)],
                        &[],
                    ));
                }
            }
            "dns-com-awns-status" => events.push(McpEvent::Status(m.get("text").unwrap_or("").to_string())),
            "dns-com-awns-ping" => {
                if let Some(id) = m.get("id") {
                    sends.extend(self.format("dns-com-awns-ping-reply", &[("id", id)], &[]));
                }
            }
            "dns-org-mud-moo-simpleedit-content" => {
                let content = match m.list("content") {
                    Some(lines) => lines.join("\n"),
                    None => m.get("content").unwrap_or("").to_string(),
                };
                events.push(McpEvent::Edit {
                    reference: m.get("reference").unwrap_or("").to_string(),
                    name: m.get("name").unwrap_or("").to_string(),
                    kind: m.get("type").unwrap_or("string-list").to_string(),
                    content,
                });
            }
            "dns-com-familiar-agent-tools" => {
                let mut tools = Vec::new();
                for line in m.list("tools").unwrap_or_default() {
                    match AgentTool::parse(line) {
                        Ok(tool) => tools.push(tool),
                        Err(e) => events.push(McpEvent::Notice(format!("Ignored an agent tool the server offered ({e})"))),
                    }
                }
                events.push(McpEvent::AgentTools(tools));
            }
            "dns-com-familiar-agent-result" => {
                if let Some(id) = m.get("id").and_then(|id| id.parse().ok()) {
                    let content = match m.list("content") {
                        Some(lines) => lines.join("\n"),
                        None => m.get("content").unwrap_or("").to_string(),
                    };
                    events.push(McpEvent::AgentResult { id, ok: m.get("ok").is_some_and(|ok| ok != "0" && !ok.is_empty()), content });
                }
            }
            _ => {}
        }
    }

    /// The lines that call an agent tool. `args` is JSON, sent in pieces the server joins.
    pub fn agent_call(&mut self, id: u64, tool: &str, args: &str) -> Vec<String> {
        let mut pieces = Vec::new();
        let mut rest = args;
        while !rest.is_empty() {
            let mut end = ARGS_CHUNK.min(rest.len());
            while !rest.is_char_boundary(end) {
                end -= 1;
            }
            // A piece ending in a space would lose it to a server that trims lines; back up to
            // a non-space if one exists in this chunk.
            if end < rest.len() && rest.as_bytes()[end - 1] == b' ' {
                let mut trimmed = end;
                while trimmed > 0 && rest.as_bytes()[trimmed - 1] == b' ' {
                    trimmed -= 1;
                }
                if trimmed > 0 {
                    end = trimmed;
                }
            }
            pieces.push(rest[..end].to_string());
            rest = &rest[end..];
        }
        self.format("dns-com-familiar-agent-call", &[("id", &id.to_string()), ("tool", tool)], &[("args", &pieces)])
    }

    /// Formats an outgoing message as one or more lines.
    pub fn format(&mut self, name: &str, values: &[(&str, &str)], lists: &[(&str, &[String])]) -> Vec<String> {
        let Some(key) = &self.key else { return Vec::new() };
        let mut first = format!("#$#{name} {key}");
        for (k, v) in values {
            first.push_str(&format!(" {k}: {}", quote(v)));
        }
        let mut lines = Vec::new();
        let mut tail = Vec::new();
        if !lists.is_empty() {
            self.next_tag = self.next_tag.wrapping_add(1);
            let tag = format!("{:x}{}", self.next_tag, &key[..4.min(key.len())]);
            for (k, items) in lists {
                first.push_str(&format!(" {k}*: \"\""));
                for item in *items {
                    let clean = item.replace(['\r', '\n'], "");
                    tail.push(format!("#$#* {tag} {k}: {clean}"));
                }
            }
            first.push_str(&format!(" _data-tag: {tag}"));
            tail.push(format!("#$#: {tag}"));
        }
        lines.push(first);
        lines.extend(tail);
        lines
    }

    /// The lines that send edited text back for a simpleedit `reference`.
    pub fn simpleedit_set(&mut self, reference: &str, kind: &str, content: &str) -> Vec<String> {
        let lines: Vec<String> = if content.is_empty() {
            Vec::new()
        } else {
            content.replace("\r\n", "\n").replace('\r', "\n").split('\n').map(str::to_string).collect()
        };
        self.format("dns-org-mud-moo-simpleedit-set", &[("reference", reference), ("type", kind)], &[("content", &lines)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started() -> (Mcp, String, Vec<String>) {
        let mut mcp = Mcp::default();
        let mut events = Vec::new();
        let (d, out) = mcp.on_line("#$#mcp version: 2.1 to: 2.1", &mut events);
        assert_eq!(d, Disposition::Hide);
        let key = mcp.key.clone().unwrap();
        (mcp, key, out)
    }

    #[test]
    fn handshake_and_negotiation() {
        let (_, key, out) = started();
        assert_eq!(out[0], format!("#$#mcp authentication-key: {key} version: 2.1 to: 2.1"));
        assert!(out.contains(&format!(
            "#$#mcp-negotiate-can {key} package: \"dns-org-mud-moo-simpleedit\" min-version: \"1.0\" max-version: \"1.0\""
        )));
        assert_eq!(out.last().unwrap(), &format!("#$#mcp-negotiate-end {key}"));
    }

    #[test]
    fn text_before_mcp_starts_is_displayed() {
        let mut mcp = Mcp::default();
        let mut ev = Vec::new();
        assert_eq!(mcp.on_line("#$#dns-com-awns-status x text: hi", &mut ev).0, Disposition::Display);
        assert_eq!(mcp.on_line("#$\"quoted", &mut ev).0, Disposition::Display);
        assert_eq!(mcp.on_line("plain", &mut ev).0, Disposition::Display);
    }

    #[test]
    fn status_ping_quoting_and_spoofing() {
        let (mut mcp, key, _) = started();
        let mut ev = Vec::new();
        mcp.on_line(&format!(r#"#$#dns-com-awns-status {key} text: "Ready \"set\" go""#), &mut ev);
        assert_eq!(ev, vec![McpEvent::Status("Ready \"set\" go".into())]);
        let (_, out) = mcp.on_line(&format!("#$#dns-com-awns-ping {key} id: 42"), &mut ev);
        assert_eq!(out, vec![format!("#$#dns-com-awns-ping-reply {key} id: \"42\"")]);
        // A message with the wrong key is hidden and ignored.
        ev.clear();
        let (d, _) = mcp.on_line("#$#dns-com-awns-status badkey text: spoofed", &mut ev);
        assert_eq!(d, Disposition::Hide);
        assert!(ev.is_empty());
        assert_eq!(mcp.on_line("#$\"#$#not mcp", &mut ev).0, Disposition::Quoted("#$#not mcp".into()));
    }

    #[test]
    fn multiline_simpleedit_round_trip() {
        let (mut mcp, key, _) = started();
        let mut ev = Vec::new();
        mcp.on_line(&format!(r##"#$#dns-org-mud-moo-simpleedit-content {key} reference: "#123.desc" name: "My description" type: string-list content*: "" _data-tag: 99"##), &mut ev);
        mcp.on_line("#$#* 99 content: First line", &mut ev);
        mcp.on_line("#$#* 99 content: ", &mut ev);
        mcp.on_line("#$#* 99 content:", &mut ev);
        mcp.on_line("#$#* 99 content:   indented: with \"quotes\"", &mut ev);
        assert!(ev.is_empty());
        mcp.on_line("#$#: 99", &mut ev);
        assert_eq!(
            ev,
            vec![McpEvent::Edit {
                reference: "#123.desc".into(),
                name: "My description".into(),
                kind: "string-list".into(),
                content: "First line\n\n\n  indented: with \"quotes\"".into(),
            }]
        );
        let out = mcp.simpleedit_set("#123.desc", "string-list", "New one\nNew two");
        assert!(out[0].starts_with(&format!("#$#dns-org-mud-moo-simpleedit-set {key} reference: \"#123.desc\" type: \"string-list\" content*: \"\" _data-tag: ")));
        let tag = out[0].rsplit(' ').next().unwrap();
        assert_eq!(&out[1..], [format!("#$#* {tag} content: New one"), format!("#$#* {tag} content: New two"), format!("#$#: {tag}")]);
    }

    #[test]
    fn vmoo_client_info_after_negotiation() {
        let (mut mcp, key, _) = started();
        let mut ev = Vec::new();
        mcp.on_line(&format!("#$#mcp-negotiate-can {key} package: dns-com-vmoo-client min-version: 1.0 max-version: 1.0"), &mut ev);
        let (_, out) = mcp.on_line(&format!("#$#mcp-negotiate-end {key}"), &mut ev);
        assert!(out[0].starts_with(&format!("#$#dns-com-vmoo-client-info {key} name: \"Familiar\"")));
    }

    #[test]
    fn keyval_parsing() {
        let kv = parse_keyvals(r#"a: 1 b-c: "x y" list*: "" d: #$!"#).unwrap();
        assert_eq!(
            kv,
            vec![
                ("a".into(), "1".into(), false),
                ("b-c".into(), "x y".into(), false),
                ("list".into(), "".into(), true),
                ("d".into(), "#$!".into(), false),
            ]
        );
        assert_eq!(parse_keyvals("a 1"), None);
        assert_eq!(parse_keyvals(r#"a: "unterminated"#), None);
    }
}
