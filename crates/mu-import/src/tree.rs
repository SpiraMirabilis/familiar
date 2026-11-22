//! BeipMU's Config.txt syntax, read into a plain tree. The format (from BeipMU's
//! ConfigParser.cpp):
//!
//! ```text
//! ; comment
//! Key=Value
//! Scope.Key=Value
//! Scope
//! {
//!   Key=Value
//!   { Unnamed=Scope }
//! }
//! "Quoted Scope"
//! {
//! }
//! ```
//!
//! Values run to the end of the line; quoted strings use C-style escapes. Named scopes with the
//! same name are merged, as BeipMU does; unnamed scopes (list items) never are.

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scope {
    /// Empty for list items.
    pub name: String,
    pub values: Vec<(String, String)>,
    pub scopes: Vec<Scope>,
}

impl Scope {
    /// The raw text of a value (still quoted, if it was), matching the key without case.
    pub fn raw(&self, key: &str) -> Option<&str> {
        self.values.iter().rev().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }

    /// A string value, with its quotes and escapes removed.
    pub fn string(&self, key: &str) -> Option<String> {
        self.raw(key).map(unquote)
    }

    pub fn str_or_empty(&self, key: &str) -> String {
        self.string(key).unwrap_or_default()
    }

    pub fn flag(&self, key: &str) -> Option<bool> {
        match self.raw(key)?.trim().to_ascii_lowercase().as_str() {
            "true" | "t" => Some(true),
            "false" | "f" => Some(false),
            _ => None,
        }
    }

    pub fn flag_or(&self, key: &str, default: bool) -> bool {
        self.flag(key).unwrap_or(default)
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        self.raw(key)?.trim().parse().ok()
    }

    /// A named child scope.
    pub fn scope(&self, name: &str) -> Option<&Scope> {
        self.scopes.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Unnamed child scopes: the items of a list.
    pub fn items(&self) -> impl Iterator<Item = &Scope> {
        self.scopes.iter().filter(|s| s.name.is_empty())
    }

    fn child(&mut self, name: &str) -> &mut Scope {
        let existing = if name.is_empty() { None } else { self.scopes.iter().position(|s| s.name == name) };
        let i = existing.unwrap_or_else(|| {
            self.scopes.push(Scope { name: name.to_string(), ..Scope::default() });
            self.scopes.len() - 1
        });
        &mut self.scopes[i]
    }
}

/// Removes quotes and escapes from a value. Unquoted values are trimmed and returned as they are.
pub fn unquote(raw: &str) -> String {
    let raw = raw.trim();
    let Some(body) = raw.strip_prefix('"') else { return raw.to_string() };
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('0') => out.push('\0'),
                Some('x') => {
                    let hex: String = chars.by_ref().take(2).collect();
                    if let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(c);
                    }
                }
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(c) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(c);
                    }
                }
                Some(other) => out.push(other),
                None => break,
            },
            c => out.push(c),
        }
    }
    out
}

struct Parser<'a> {
    text: &'a str,
    pos: usize,
    line: usize,
    errors: Vec<String>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        if c == '\n' {
            self.line += 1;
        }
        Some(c)
    }

    fn skip_space(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() || c == '\u{feff}' {
                self.bump();
            } else if c == ';' {
                while let Some(c) = self.bump() {
                    if c == '\n' {
                        break;
                    }
                }
            } else {
                break;
            }
        }
    }

    fn error(&mut self, message: &str) {
        self.errors.push(format!("line {}: {message}", self.line));
    }

    fn rest_of_line(&mut self) -> &str {
        let start = self.pos;
        let end = self.text[start..].find(['\r', '\n']).map_or(self.text.len(), |i| start + i);
        self.pos = end;
        &self.text[start..end]
    }

    fn key(&mut self) -> Option<String> {
        if self.peek() == Some('"') {
            let start = self.pos;
            self.bump();
            while let Some(c) = self.bump() {
                match c {
                    '\\' => {
                        self.bump();
                    }
                    '"' => break,
                    '\n' => break,
                    _ => {}
                }
            }
            return Some(unquote(&self.text[start..self.pos]));
        }
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                self.bump();
            } else {
                break;
            }
        }
        (self.pos > start).then(|| self.text[start..self.pos].to_string())
    }

    /// Parses entries into `scope` until a closing brace (consumed) or the end.
    fn entries(&mut self, scope: &mut Scope, depth: usize) {
        loop {
            self.skip_space();
            match self.peek() {
                None => {
                    if depth > 0 {
                        self.error("missing '}' at the end of the file");
                    }
                    return;
                }
                Some('}') => {
                    self.bump();
                    if depth == 0 {
                        self.error("unexpected '}'");
                        continue;
                    }
                    return;
                }
                Some('{') => {
                    self.bump();
                    let mut item = Scope::default();
                    self.entries(&mut item, depth + 1);
                    scope.scopes.push(item);
                }
                _ => self.entry(scope, depth),
            }
        }
    }

    /// `Key=Value`, `A.B.Key=Value`, or `Key { ... }`.
    fn entry(&mut self, scope: &mut Scope, depth: usize) {
        let Some(key) = self.key() else {
            let junk = self.rest_of_line().trim().to_string();
            self.error(&format!("can't read '{junk}'"));
            return;
        };
        if self.peek() == Some('.') {
            self.bump();
            let child = scope.child(&key);
            // A dotted path names one value; it never opens a block.
            self.entry(child, depth);
            return;
        }
        self.skip_inline_space();
        match self.peek() {
            Some('=') => {
                self.bump();
                self.skip_inline_space();
                let value = self.rest_of_line().trim_end().to_string();
                scope.values.push((key, value));
            }
            _ => {
                self.skip_space();
                if self.peek() == Some('{') {
                    self.bump();
                    // Parse into a fresh scope, then merge into an existing one of that name.
                    let mut block = Scope::default();
                    self.entries(&mut block, depth + 1);
                    let child = scope.child(&key);
                    child.values.extend(block.values);
                    child.scopes.extend(block.scopes);
                } else {
                    self.error(&format!("expected '=' or '{{' after '{key}'"));
                }
            }
        }
    }

    fn skip_inline_space(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.bump();
        }
    }
}

/// Parses a whole file. Problems are returned alongside whatever could be read.
pub fn parse(text: &str) -> (Scope, Vec<String>) {
    let mut parser = Parser { text, pos: 0, line: 1, errors: Vec::new() };
    let mut root = Scope::default();
    parser.entries(&mut root, 0);
    (root, parser.errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_values_and_dotted_keys() {
        let (root, errors) = parse(
            "Version=329\r\n; a comment\r\nConnections\r\n{\r\n  Logging.Size=256\r\n  \"Key Macros2\"\r\n  {\r\n    {\r\n      Macro=\"a \\\"b\\\"\"\r\n    }\r\n    {\r\n      Macro=\"c\"\r\n    }\r\n  }\r\n  Logging.Other=t\r\n}\r\n",
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(root.int("version"), Some(329));
        let c = root.scope("Connections").unwrap();
        let logging = c.scope("Logging").unwrap();
        assert_eq!(logging.int("Size"), Some(256));
        assert_eq!(logging.flag("Other"), Some(true), "dotted keys merge into one scope");
        let macros: Vec<String> = c.scope("Key Macros2").unwrap().items().map(|i| i.str_or_empty("Macro")).collect();
        assert_eq!(macros, ["a \"b\"", "c"]);
    }

    #[test]
    fn escapes_and_unquoted_values() {
        assert_eq!(unquote(r#""C:\\Windows\\chord.wav""#), r"C:\Windows\chord.wav");
        assert_eq!(unquote(r#""\r\nhttp://x/""#), "\r\nhttp://x/");
        assert_eq!(unquote("RGB(0,128,0)"), "RGB(0,128,0)");
    }

    #[test]
    fn reports_errors_but_keeps_going() {
        let (root, errors) = parse("A\n{\n  ?? junk\n  B=1\n");
        assert_eq!(root.scope("A").unwrap().int("B"), Some(1));
        assert_eq!(errors.len(), 2, "{errors:?}");
    }
}
