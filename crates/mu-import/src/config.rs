//! Converts a BeipMU `Config.txt` into our worlds, characters, puppets and rules.
//!
//! BeipMU's model is richer in places (nested triggers, per-puppet rules, paragraph styling), so
//! the conversion is lossy. Everything that can't be carried over is listed in
//! [`Imported::warnings`].

use std::collections::BTreeMap;

use mu_config::{Alias, Character, Config, Highlight, Macro, Pattern, Puppet, Rules, Trigger, World};

use crate::tree::{self, Scope};

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Imported {
    /// Rules for every world (BeipMU's Connections level).
    pub global: Rules,
    pub worlds: Vec<World>,
    /// BeipMU's startup script, as the path written in the config (usually a Windows path).
    pub startup_script: Option<String>,
    /// Problems reading the file, and things that couldn't be carried over.
    pub warnings: Vec<String>,
}

/// Reads a BeipMU config file's text.
pub fn import(text: &str) -> Imported {
    let (root, errors) = tree::parse(text);
    let mut ctx = Ctx { warnings: errors.into_iter().map(|e| format!("Config.txt {e}")).collect() };
    let mut imported = Imported { startup_script: root.string("ScriptStartup").filter(|s| !s.trim().is_empty()), ..Imported::default() };

    let Some(connections) = root.scope("Connections") else {
        ctx.warn("No Connections section: is this a BeipMU Config.txt?".to_string());
        imported.warnings = ctx.warnings;
        return imported;
    };
    // Aliases are switched on and off as a whole, at the top level (off by default).
    let aliases_on = connections.scope("Aliases").is_some_and(|a| a.flag_or("Active", false));
    let has_aliases = |s: &Scope| s.scope("Aliases").is_some_and(|a| a.items().next().is_some());
    let any_aliases = has_aliases(connections) || servers(connections).any(|s| has_aliases(s) || characters(s).any(has_aliases));
    if any_aliases && !aliases_on {
        ctx.warn("Aliases were switched off in BeipMU (Process Aliases unchecked), so they're imported disabled".into());
    }

    imported.global = ctx.rules(connections, "All worlds", aliases_on);
    for server in servers(connections) {
        if let Some(world) = ctx.world(server, aliases_on) {
            imported.worlds.push(world);
        }
    }
    imported.warnings = ctx.warnings;
    imported
}

/// Servers are named scopes under `Servers` (written as `Shortcuts` by recent builds).
fn servers(connections: &Scope) -> impl Iterator<Item = &Scope> {
    ["Shortcuts", "Servers"].into_iter().filter_map(|n| connections.scope(n)).flat_map(|s| s.scopes.iter())
}

fn characters(server: &Scope) -> impl Iterator<Item = &Scope> {
    server.scope("Characters").into_iter().flat_map(|c| c.scopes.iter())
}

/// A scope's name: the scope's own name, or an old-style `Name=` value.
fn name_of(scope: &Scope) -> String {
    if scope.name.is_empty() { scope.str_or_empty("Name") } else { scope.name.clone() }
}

struct Ctx {
    warnings: Vec<String>,
}

impl Ctx {
    fn warn(&mut self, message: String) {
        if !self.warnings.contains(&message) {
            self.warnings.push(message);
        }
    }

    fn world(&mut self, server: &Scope, aliases_on: bool) -> Option<World> {
        let name = name_of(server);
        let host_port = server.string("Host").unwrap_or_default();
        if host_port.trim().is_empty() {
            self.warn(format!("World \"{name}\" has no host, so it was skipped"));
            return None;
        }
        let (host, port) = match host_port.rsplit_once(':').map(|(h, p)| (h, p.trim().parse::<u16>())) {
            Some((h, Ok(p))) => (h.trim().to_string(), p),
            _ => (host_port.trim().to_string(), server.int("Port").and_then(|p| u16::try_from(p).ok()).unwrap_or(23)),
        };
        let encoding = match server.string("Encoding").unwrap_or_default().trim() {
            "UTF8" => "UTF-8",
            "CP437" => "CP437",
            // BeipMU's default, CP1252. Auto reads it too, and also reads UTF-8.
            _ => "Auto",
        };
        let label = format!("World \"{name}\"");
        let characters_out = characters(server).map(|c| self.character(c, &name, aliases_on)).collect();
        Some(World {
            name: name.clone(),
            host,
            port,
            tls: server.flag_or("TLS", false),
            // BeipMU didn't check certificates unless asked.
            verify_certificate: server.flag_or("VerifyCertificate", false),
            encoding: encoding.to_string(),
            allow_webviews: match server.int("GMCP_WebView") {
                Some(0) => Some(false),
                Some(1) => Some(true),
                _ => None,
            },
            rules: self.rules(server, &label, aliases_on),
            characters: characters_out,
            ..World::default()
        })
    }

    fn character(&mut self, scope: &Scope, world: &str, aliases_on: bool) -> Character {
        let name = name_of(scope);
        let password = scope.str_or_empty("Password");
        let mut login = scope.str_or_empty("Connect").replace("%NAME%", &name);
        if !password.is_empty() {
            login = login.replace("%PASSWORD%", &password);
        }
        let login = login.lines().map(str::trim_end).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n");
        let label = format!("{name} @ {world}");
        let mut rules = self.rules(scope, &label, aliases_on);
        if let Some(vars) = scope.scope("Variables") {
            for v in vars.items() {
                let key = v.str_or_empty("Name");
                if !key.is_empty() {
                    rules.variables.insert(key, v.str_or_empty("Value"));
                }
            }
        }
        let mut puppets = Vec::new();
        for p in scope.scope("Puppets").into_iter().flat_map(|p| p.scopes.iter()) {
            let puppet_name = name_of(p);
            let has_rules = ["Triggers", "Aliases", "KeyboardMacros2", "KeyboardMacros"]
                .iter()
                .any(|s| p.scope(s).is_some_and(|s| !s.scopes.is_empty() || !s.values.is_empty()));
            if has_rules {
                self.warn(format!(
                    "Puppet \"{puppet_name}\" ({label}) had its own triggers, aliases or macros, which puppets don't have here"
                ));
            }
            puppets.push(Puppet {
                name: puppet_name,
                receive_prefix: p.str_or_empty("ReceivePrefix"),
                regex: p.flag_or("RegularExpression", false),
                hide_prefix: p.flag_or("HideReceivePrefix", true),
                send_prefix: p.str_or_empty("SendPrefix"),
                auto_open: p.flag_or("AutoConnect", true),
                open_on_connect: p.flag_or("ConnectWithPlayer", false),
                remove_accidental_prefix: p.flag_or("RemoveAccidentalPrefix", false),
            });
        }
        Character { name, login, rules, puppets }
    }

    /// The triggers, aliases and macros in a Connections, server or character scope.
    fn rules(&mut self, scope: &Scope, label: &str, aliases_on: bool) -> Rules {
        let mut rules = Rules::default();
        if let Some(triggers) = scope.scope("Triggers") {
            let active = triggers.flag_or("Active", true);
            if !active && triggers.items().next().is_some() {
                self.warn(format!("Triggers for {label} were switched off, so they're imported disabled"));
            }
            self.triggers(triggers, label, !active, None, &mut rules.triggers);
        }
        if let Some(aliases) = scope.scope("Aliases") {
            self.aliases(aliases, label, aliases_on, None, &mut rules.aliases);
        }
        if let Some(macros) = scope.scope("KeyboardMacros2") {
            self.macros(macros, label, &mut rules.macros);
        }
        if let Some(old) = scope.scope("KeyboardMacros") {
            for (key, raw) in &old.values {
                if key.eq_ignore_ascii_case("Active") {
                    continue;
                }
                match old_macro(raw) {
                    Some(m) => rules.macros.push(m),
                    None => self.warn(format!("A keyboard macro for {label} couldn't be read: {raw}")),
                }
            }
        }
        rules
    }

    fn triggers(&mut self, list: &Scope, label: &str, disabled: bool, parent: Option<&str>, out: &mut Vec<Trigger>) {
        for item in list.items() {
            let off = disabled || item.flag_or("Disabled", false);
            let Some(matcher) = find_string(item.scope("FindString").unwrap_or(item)) else {
                // A folder: its children apply to every line.
                if let Some(children) = item.scope("Triggers") {
                    self.triggers(children, label, off || !children.flag_or("Active", true), parent, out);
                }
                continue;
            };
            let name = item.string("Description").filter(|d| !d.is_empty()).unwrap_or_else(|| matcher.pattern.clone());
            let what = format!("Trigger \"{name}\" ({label})");
            if let Some(parent) = parent {
                self.warn(format!("{what} only ran on lines that \"{parent}\" matched; here it runs on every line"));
            }
            let mut t = Trigger { name: name.clone(), enabled: !off, stop: item.flag_or("StopProcessing", false), ..Trigger::default() };
            self.trigger_actions(item, &matcher, &what, &mut t);
            if item.flag_or("AwayPresent", false) {
                t.enabled = false;
                self.warn(format!("{what} only fired while you were away or present; it's imported disabled"));
            }
            if item.flag_or("Multiline", false) {
                self.warn(format!("{what} matched across several lines, which isn't supported"));
            }
            if item.flag_or("Cooldown", false) {
                self.warn(format!("{what} had a cooldown, which isn't supported"));
            }
            t.matcher = matcher;
            out.push(t);
            if let Some(children) = item.scope("Triggers") {
                self.triggers(children, label, off || !children.flag_or("Active", true), Some(&name), out);
            }
        }
    }

    fn trigger_actions(&mut self, item: &Scope, matcher: &Pattern, what: &str, t: &mut Trigger) {
        let mut h = Highlight::default();
        let mut styled = false;
        if let Some(c) = item.scope("Color") {
            if c.flag_or("UseForeColor", false) && !c.flag_or("ForeHash", false) {
                h.fg = Some(color(c.raw("ForeColor").unwrap_or("RGB(255,255,255)")).unwrap_or_else(|| "#ffffff".into()));
                styled = true;
            }
            if c.flag_or("UseBackColor", false) && !c.flag_or("BackHash", false) {
                h.bg = Some(color(c.raw("BackColor").unwrap_or("RGB(0,0,0)")).unwrap_or_else(|| "#000000".into()));
                styled = true;
            }
            if c.flag_or("ForeHash", false) || c.flag_or("BackHash", false) {
                self.warn(format!("{what}: colours picked by hashing the text aren't supported"));
            }
            if c.string("FontFace").is_some_and(|f| !f.is_empty()) || c.int("FontSize").is_some_and(|s| s != 0) {
                self.warn(format!("{what}: font changes aren't supported"));
            }
            h.whole_line |= c.flag_or("WholeLine", false);
        }
        if let Some(s) = item.scope("Style") {
            for (set, value, field) in
                [("SetBold", "Bold", &mut h.bold), ("SetItalic", "Italic", &mut h.italic), ("SetUnderline", "Underline", &mut h.underline)]
            {
                if s.flag_or(set, false) && s.flag_or(value, false) {
                    *field = true;
                    styled = true;
                }
            }
            if s.flag_or("SetStrikeout", false) || s.flag_or("Flash", false) {
                self.warn(format!("{what}: strikeout and flashing aren't supported"));
            }
            h.whole_line |= s.flag_or("WholeLine", false);
        }
        if styled {
            // BeipMU styles a regex's groups, if it has any, rather than the whole match.
            h.groups = matcher.regex;
            t.highlight = Some(h);
        }
        if let Some(g) = item.scope("Gag") {
            t.gag = g.flag_or("Active", false);
            t.gag_log = g.flag_or("Log", false);
        }
        if let Some(s) = item.scope("Spawn").filter(|s| s.flag_or("Active", false)) {
            t.spawn = template(&s.str_or_empty("Title"));
            if t.spawn.is_empty() {
                t.spawn = "Spawn".into();
            }
            if !s.str_or_empty("CaptureUntil").is_empty() {
                self.warn(format!(
                    "{what}: capturing the lines that follow into the spawn window isn't supported (only the matching line goes there)"
                ));
            }
            if !s.flag_or("Copy", false) {
                // BeipMU moved the line rather than copying it.
                t.gag = true;
            }
        }
        if let Some(s) = item.scope("Sound").filter(|s| s.flag_or("Active", false)) {
            let path = s.str_or_empty("Sound");
            if path.contains('\\') {
                self.warn(format!("{what}: its sound is a Windows path ({path}); choose a Linux one in the trigger editor"));
            }
            t.sound = path;
        }
        if let Some(a) = item.scope("Activate") {
            t.important = a.flag_or("ImportantActivity", false);
        }
        if let Some(s) = item.scope("Send").filter(|s| s.flag_or("Active", false)) {
            if s.flag_or("SendOnClick", false) {
                self.warn(format!("{what}: sending when the text is clicked isn't supported; it sends straight away"));
            }
            t.send = template(&s.str_or_empty("Send")).replace("\r\n", "\n");
        }
        if let Some(f) = item.scope("Filter").filter(|f| f.flag_or("Active", false)) {
            let mut replace = f.str_or_empty("Replace");
            if f.flag_or("HTML", false) {
                // Keep simple bold/italic/underline tags as styling; drop other markup.
                let (text, tags) = strip_html(&replace);
                replace = text;
                let style = |names: &[&str]| tags.iter().any(|t| names.contains(&t.as_str()));
                let (bold, italic, underline) = (style(&["b", "strong"]), style(&["i", "em"]), style(&["u"]));
                if bold || italic || underline {
                    let h = t.highlight.get_or_insert_with(Highlight::default);
                    h.bold |= bold;
                    h.italic |= italic;
                    h.underline |= underline;
                    // The replacement takes the style of the start of the match.
                    h.groups = false;
                }
                if tags.iter().any(|t| !["b", "strong", "i", "em", "u"].contains(&t.as_str())) {
                    self.warn(format!("{what}: its HTML replacement used markup other than bold, italic and underline, which was dropped"));
                }
            }
            t.replace = Some(template(&replace));
        }
        if item.scope("Toast").is_some_and(|s| s.flag_or("Active", false)) {
            t.notify = true;
        }
        if let Some(s) = item.scope("Script").filter(|s| s.flag_or("Active", false)) {
            let function = s.str_or_empty("Function");
            if !function.is_empty() {
                t.script = format!("beipmu:{function}");
            }
        }
        if let Some(s) = item.scope("Speech").filter(|s| s.flag_or("Active", false)) {
            let say = s.str_or_empty("Say");
            t.speak = if s.flag_or("WholeLine", false) || say.is_empty() { "$0".into() } else { template(&say) };
        }
        if item.scope("Avatar").is_some_and(|a| !a.str_or_empty("URL").is_empty()) {
            self.warn(format!("{what}: trigger avatars aren't supported"));
        }
        if item.scope("Stat").is_some() {
            self.warn(format!("{what}: stat window updates aren't supported"));
        }
        if item.scope("Paragraph").is_some_and(|p| p.values.iter().any(|(k, v)| k.starts_with("Use") && v.trim() == "true")) {
            self.warn(format!("{what}: paragraph styling (alignment, borders, indents) isn't supported"));
        }
    }

    fn aliases(&mut self, list: &Scope, label: &str, on: bool, parent: Option<&str>, out: &mut Vec<Alias>) {
        for item in list.items() {
            let children = item.scope("Aliases");
            if item.flag_or("Folder", false) {
                if let Some(children) = children {
                    self.aliases(children, label, on, parent, out);
                }
                continue;
            }
            let Some(matcher) = find_string(item.scope("FindString").unwrap_or(item)) else { continue };
            let name = item.string("Description").filter(|d| !d.is_empty()).unwrap_or_else(|| matcher.pattern.clone());
            if let Some(parent) = parent {
                self.warn(format!("Alias \"{name}\" ({label}) only ran after \"{parent}\" matched; here it always runs"));
            }
            out.push(Alias {
                enabled: on,
                matcher,
                replacement: template(&item.str_or_empty("Replace")),
                substitute: true,
                stop: item.flag_or("StopProcessing", false),
            });
            if let Some(children) = children {
                self.aliases(children, label, on, Some(&name), out);
            }
        }
    }

    fn macros(&mut self, list: &Scope, label: &str, out: &mut Vec<Macro>) {
        for item in list.items() {
            if let Some(children) = item.scope("KeyboardMacros2") {
                self.macros(children, label, out);
            }
            if item.flag_or("Folder", false) {
                continue;
            }
            let raw = item.str_or_empty("key");
            match key_chord(&raw) {
                Some(key) => out.push(Macro { key, command: item.str_or_empty("Macro"), insert: item.flag_or("Type", false) }),
                None => self.warn(format!("A keyboard macro for {label} uses a key that can't be mapped: {raw}")),
            }
        }
    }
}

/// A FindString scope (or a trigger with the old inline fields) as a pattern. None when it has
/// no text, which BeipMU uses for folders.
fn find_string(s: &Scope) -> Option<Pattern> {
    let text = s.string("MatchText")?;
    if text.is_empty() {
        return None;
    }
    let case_sensitive = s.flag_or("MatchCase", false);
    if s.flag_or("RegularExpression", false) {
        // Regexes honour Match Case but none of the other FindString options.
        return Some(Pattern { pattern: text, regex: true, case_sensitive, whole_word: false });
    }
    let (starts, ends, whole) = (s.flag_or("StartsWith", false), s.flag_or("EndsWith", false), s.flag_or("WholeWord", false));
    if starts || ends {
        let mut pattern = escape_regex(&text);
        if whole {
            pattern = format!(r"\b{pattern}\b");
        }
        if starts {
            pattern.insert(0, '^');
        }
        if ends {
            pattern.push('$');
        }
        return Some(Pattern { pattern, regex: true, case_sensitive, whole_word: false });
    }
    Some(Pattern { pattern: text, regex: false, case_sensitive, whole_word: whole })
}

fn escape_regex(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if r"\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Turns BeipMU's `\1` (and `\a12`) capture references into ours (`$1`, `${12}`), escaping `$`.
pub fn template(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '$' => out.push_str("$$"),
            '\\' if chars.get(i + 1).is_some_and(char::is_ascii_digit) => {
                out.push('$');
                out.push(chars[i + 1]);
                i += 1;
            }
            '\\' if chars.get(i + 1) == Some(&'a') && chars.get(i + 2..i + 4).is_some_and(|d| d.iter().all(char::is_ascii_digit)) => {
                let n = chars[i + 2].to_digit(10).unwrap() * 10 + chars[i + 3].to_digit(10).unwrap();
                out.push_str(&format!("${{{n}}}"));
                i += 3;
            }
            c => out.push(c),
        }
        i += 1;
    }
    out
}

/// Removes HTML tags and decodes common entities. Returns the text and the tag names seen.
fn strip_html(html: &str) -> (String, Vec<String>) {
    let mut text = String::with_capacity(html.len());
    let mut tags = Vec::new();
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        text.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('>') else {
            text.push_str(&rest[open..]);
            rest = "";
            break;
        };
        let tag = &rest[open + 1..open + close];
        let name: String = tag.trim_start_matches('/').chars().take_while(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase();
        if !name.is_empty() && !tags.contains(&name) {
            tags.push(name);
        }
        rest = &rest[open + close + 1..];
    }
    text.push_str(rest);
    let text = text.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&nbsp;", " ").replace("&amp;", "&");
    (text, tags)
}

/// `RGB(r,g,b)` as `#rrggbb`.
fn color(raw: &str) -> Option<String> {
    let inner = raw.trim().strip_prefix("RGB(")?.strip_suffix(')')?;
    let parts: Vec<u8> = inner.split(',').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    match parts.as_slice() {
        [r, g, b] => Some(format!("#{r:02x}{g:02x}{b:02x}")),
        _ => None,
    }
}

/// A BeipMU key like `Control+Alt+A`, `NumPad8` or `F5` in our form (GDK key names).
pub fn key_chord(raw: &str) -> Option<String> {
    const MODIFIERS: [(&str, &str); 6] =
        [("control+", "Ctrl+"), ("alt+", "Alt+"), ("shift+", "Shift+"), ("?control+", ""), ("?alt+", ""), ("?shift+", "")];
    let mut rest = raw.trim();
    let mut mods = String::new();
    while let Some((prefix, m)) = MODIFIERS.iter().find(|(p, _)| rest.to_ascii_lowercase().starts_with(p)) {
        mods.push_str(m);
        rest = &rest[prefix.len()..];
    }
    if rest.is_empty() {
        return None;
    }
    let names: BTreeMap<&str, &str> = [
        ("add", "KP_Add"),
        ("subtract", "KP_Subtract"),
        ("multiply", "KP_Multiply"),
        ("divide", "KP_Divide"),
        ("decimal", "KP_Decimal"),
        ("enter", "Return"),
        ("return", "Return"),
        ("escape", "Escape"),
        ("esc", "Escape"),
        ("space", "space"),
        ("tab", "Tab"),
        ("backspace", "BackSpace"),
        ("back", "BackSpace"),
        ("delete", "Delete"),
        ("del", "Delete"),
        ("insert", "Insert"),
        ("ins", "Insert"),
        ("home", "Home"),
        ("end", "End"),
        ("pageup", "Page_Up"),
        ("prior", "Page_Up"),
        ("pagedown", "Page_Down"),
        ("next", "Page_Down"),
        ("up", "Up"),
        ("down", "Down"),
        ("left", "Left"),
        ("right", "Right"),
        ("pause", "Pause"),
        ("clear", "KP_Begin"),
    ]
    .into_iter()
    .collect();
    let lower = rest.to_ascii_lowercase();
    let key = if let Some(n) = lower.strip_prefix("numpad").filter(|n| n.len() == 1 && n.chars().all(|c| c.is_ascii_digit())) {
        format!("KP_{n}")
    } else if let Some(name) = names.get(lower.as_str()) {
        name.to_string()
    } else if lower.len() > 1 && lower.starts_with('f') && lower[1..].parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)) {
        format!("F{}", &lower[1..])
    } else if rest.chars().count() == 1 {
        rest.to_uppercase()
    } else {
        return None;
    };
    Some(format!("{mods}{key}"))
}

/// The old one-line macro form: `KEY,"text"` with an optional `,T` (type into the input).
fn old_macro(raw: &str) -> Option<Macro> {
    let (key, rest) = raw.split_once(',')?;
    let rest = rest.trim();
    let insert = rest.ends_with("\",T");
    let text = if insert { &rest[..rest.len() - 2] } else { rest };
    Some(Macro { key: key_chord(key)?, command: tree::unquote(text), insert })
}

/// What [`merge`] did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MergeSummary {
    pub worlds_added: Vec<String>,
    /// Worlds that already exist (by name) and were left alone.
    pub worlds_skipped: Vec<String>,
    pub characters: usize,
    pub puppets: usize,
    pub triggers: usize,
    pub aliases: usize,
    pub macros: usize,
    pub variables: usize,
}

impl MergeSummary {
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        let mut count = |n: usize, one: &str, many: &str| {
            if n > 0 {
                parts.push(format!("{n} {}", if n == 1 { one } else { many }));
            }
        };
        count(self.worlds_added.len(), "world", "worlds");
        count(self.characters, "character", "characters");
        count(self.puppets, "puppet", "puppets");
        count(self.triggers, "trigger", "triggers");
        count(self.aliases, "alias", "aliases");
        count(self.macros, "macro", "macros");
        count(self.variables, "variable", "variables");
        let mut text = if parts.is_empty() { "Nothing new to import".to_string() } else { format!("Imported {}", parts.join(", ")) };
        if !self.worlds_skipped.is_empty() {
            text.push_str(&format!(". Already had: {}", self.worlds_skipped.join(", ")));
        }
        text
    }
}

fn count_rules(rules: &Rules, summary: &mut MergeSummary) {
    summary.triggers += rules.triggers.len();
    summary.aliases += rules.aliases.len();
    summary.macros += rules.macros.len();
    summary.variables += rules.variables.len();
}

/// Adds imported worlds and global rules to a config. Worlds that already exist (same name,
/// ignoring case) are skipped, and global rules already present aren't added twice, so
/// importing the same file again changes nothing.
pub fn merge(config: &mut Config, imported: &Imported) -> MergeSummary {
    let mut summary = MergeSummary::default();
    let global = &mut config.rules;
    for t in &imported.global.triggers {
        if !global.triggers.contains(t) {
            global.triggers.push(t.clone());
            summary.triggers += 1;
        }
    }
    for a in &imported.global.aliases {
        if !global.aliases.contains(a) {
            global.aliases.push(a.clone());
            summary.aliases += 1;
        }
    }
    for m in &imported.global.macros {
        if !global.macros.contains(m) {
            global.macros.push(m.clone());
            summary.macros += 1;
        }
    }
    for (k, v) in &imported.global.variables {
        if !global.variables.contains_key(k) {
            global.variables.insert(k.clone(), v.clone());
            summary.variables += 1;
        }
    }
    for world in &imported.worlds {
        if config.worlds.iter().any(|w| w.name.eq_ignore_ascii_case(&world.name)) {
            summary.worlds_skipped.push(world.name.clone());
            continue;
        }
        summary.worlds_added.push(world.name.clone());
        summary.characters += world.characters.len();
        count_rules(&world.rules, &mut summary);
        for c in &world.characters {
            summary.puppets += c.puppets.len();
            count_rules(&c.rules, &mut summary);
        }
        config.worlds.push(world.clone());
    }
    summary
}

#[cfg(test)]
mod tests;
