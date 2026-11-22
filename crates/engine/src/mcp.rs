//! MCP 2.1, the MUD Client Protocol used by MOOs and MUCKs, carried in lines that start with
//! `#$#`. Spec: <https://www.moo.mud.org/mcp/mcp2.html>.
//!
//! Supported packages: mcp-negotiate 2.0, dns-org-mud-moo-simpleedit 1.0 (edit server text in
//! the client), dns-com-awns-status 1.0 (status line), dns-com-awns-ping 1.0 and
//! dns-com-vmoo-client 1.0.

use std::collections::HashMap;

const PACKAGES: [(&str, &str, &str); 5] = [
    ("mcp-negotiate", "2.0", "2.0"),
    ("dns-org-mud-moo-simpleedit", "1.0", "1.0"),
    ("dns-com-awns-status", "1.0", "1.0"),
    ("dns-com-awns-ping", "1.0", "1.0"),
    ("dns-com-vmoo-client", "1.0", "1.0"),
];

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

#[derive(Debug, Default)]
pub struct Mcp {
    /// Our authentication key, once the server has started MCP.
    key: Option<String>,
    /// Multiline messages waiting for their end, by data tag.
    pending: HashMap<String, Message>,
    /// Packages the server says it supports, with their max version.
    server_packages: HashMap<String, String>,
    next_tag: u32,
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
        s = s.strip_prefix(": ")?;
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
        !self.server_packages.is_empty()
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
            self.pending.clear();
            self.server_packages.clear();
            for (name, min, max) in PACKAGES {
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
            if let Some((tag, rest)) = rest.split_once(' ')
                && let Some((key, value)) = rest.split_once(": ")
                && let Some(message) = self.pending.get_mut(tag)
                && let Some((_, list)) = message.lists.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key))
            {
                list.push(value.to_string());
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
            } else if let Some(tag) = message.get("_data-tag").map(str::to_string) {
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
            _ => {}
        }
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
        for (k, items) in lists {
            self.next_tag += 1;
            let tag = format!("{:x}{}", self.next_tag, &key[..4.min(key.len())]);
            first.push_str(&format!(" {k}*: \"\" _data-tag: {tag}"));
            tail.extend(items.iter().map(|item| format!("#$#* {tag} {k}: {item}")));
            tail.push(format!("#$#: {tag}"));
        }
        lines.push(first);
        lines.extend(tail);
        lines
    }

    /// The lines that send edited text back for a simpleedit `reference`.
    pub fn simpleedit_set(&mut self, reference: &str, kind: &str, content: &str) -> Vec<String> {
        let lines: Vec<String> = content.lines().map(str::to_string).collect();
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
        mcp.on_line("#$#* 99 content:   indented: with \"quotes\"", &mut ev);
        assert!(ev.is_empty());
        mcp.on_line("#$#: 99", &mut ev);
        assert_eq!(
            ev,
            vec![McpEvent::Edit {
                reference: "#123.desc".into(),
                name: "My description".into(),
                kind: "string-list".into(),
                content: "First line\n  indented: with \"quotes\"".into(),
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
