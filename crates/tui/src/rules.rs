//! The window for triggers, aliases, keyboard macros and variables, at any scope: a list for
//! each kind, and a form for the one being edited.

use familiar_config::{Alias, Config, Highlight, Macro, Rules, Scope, Trigger};
use familiar_engine::KeyChord;

use crate::form::{Field, FieldKind};

pub const TRIGGERS: usize = 0;
pub const ALIASES: usize = 1;
pub const MACROS: usize = 2;
pub const VARIABLES: usize = 3;

/// Each section's name, what it's for, and what one of its entries is called.
pub const SECTIONS: [(&str, &str, &str); 4] = [
    ("Triggers", "Run on each line from the server. Character triggers run first, then world, then all-world ones.", "trigger"),
    ("Aliases", "Replace what you type before it is sent. The first matching alias wins.", "alias"),
    ("Macros", "Send a command when you press a key. Aliases apply to it.", "macro"),
    ("Variables", "Use as %name% in trigger sends and alias replacements. You can also type /set name value.", "variable"),
];

/// A line of a section's list.
pub struct Row {
    pub title: String,
    pub detail: String,
    /// None for things that can't be switched off.
    pub enabled: Option<bool>,
}

pub struct RulesView {
    /// Every scope in the config: all worlds, then each world followed by its characters.
    pub scopes: Vec<Scope>,
    pub scope: usize,
    pub section: usize,
    pub selected: usize,
    pub form: Option<RuleForm>,
    /// Delete was pressed once on the selected row.
    pub delete_armed: bool,
    /// What the last key did, said in place of the key hints until the next key.
    pub notice: Option<String>,
}

fn trigger_summary(t: &Trigger) -> String {
    let mut parts = Vec::new();
    if !t.name.is_empty() {
        parts.push(t.matcher.pattern.clone());
    }
    if t.matcher.regex {
        parts.push("regex".to_string());
    }
    if t.gag {
        parts.push("hide".into());
    }
    if t.highlight.is_some() {
        parts.push("highlight".into());
    }
    if t.replace.is_some() {
        parts.push("replace".into());
    }
    if !t.send.is_empty() {
        parts.push(format!("send \"{}\"", t.send.lines().next().unwrap_or("")));
    }
    if !t.spawn.is_empty() {
        parts.push(format!("→ {}", t.spawn));
    }
    if !t.sound.is_empty() {
        parts.push("sound".into());
    }
    if t.notify {
        parts.push("notify".into());
    }
    if !t.script.is_empty() {
        parts.push(format!("{}()", t.script));
    }
    if !t.speak.is_empty() {
        parts.push("speak".into());
    }
    if t.stop {
        parts.push("stop".into());
    }
    parts.join(" · ")
}

impl RulesView {
    /// Opens on `preferred` (or the rules for all worlds).
    pub fn new(config: &Config, preferred: Option<&Scope>) -> Self {
        let mut scopes = vec![Scope::Global];
        for world in &config.worlds {
            scopes.push(Scope::World { world: world.name.clone() });
            for c in &world.characters {
                scopes.push(Scope::Character { world: world.name.clone(), character: c.name.clone() });
            }
        }
        let scope = preferred.and_then(|p| scopes.iter().position(|s| s == p)).unwrap_or(0);
        Self { scopes, scope, section: TRIGGERS, selected: 0, form: None, delete_armed: false, notice: None }
    }

    pub fn scope(&self) -> &Scope {
        &self.scopes[self.scope]
    }

    pub fn rows(&self, config: &Config) -> Vec<Row> {
        let Some(rules) = config.rules(self.scope()) else { return Vec::new() };
        match self.section {
            TRIGGERS => rules
                .triggers
                .iter()
                .map(|t| Row { title: if t.name.is_empty() { t.matcher.pattern.clone() } else { t.name.clone() }, detail: trigger_summary(t), enabled: Some(t.enabled) })
                .collect(),
            ALIASES => rules.aliases.iter().map(|a| Row { title: a.matcher.pattern.clone(), detail: a.replacement.replace('\n', " ⏎ "), enabled: Some(a.enabled) }).collect(),
            MACROS => rules.macros.iter().map(|m| Row { title: m.key.clone(), detail: if m.insert { format!("{}  (into the input)", m.command) } else { m.command.clone() }, enabled: None }).collect(),
            _ => rules.variables.iter().map(|(name, value)| Row { title: format!("%{name}%"), detail: value.clone(), enabled: None }).collect(),
        }
    }

    /// How many entries each section has at this scope.
    pub fn counts(&self, config: &Config) -> [usize; 4] {
        config.rules(self.scope()).map_or([0; 4], |r| [r.triggers.len(), r.aliases.len(), r.macros.len(), r.variables.len()])
    }

    /// Moves the selection, staying inside a list of `count` rows.
    pub fn select(&mut self, delta: isize, count: usize) {
        self.selected = (self.selected as isize + delta).clamp(0, count.saturating_sub(1) as isize) as usize;
        self.delete_armed = false;
    }

    pub fn change_section(&mut self, delta: isize) {
        self.section = (self.section as isize + delta).rem_euclid(SECTIONS.len() as isize) as usize;
        self.selected = 0;
        self.delete_armed = false;
    }

    pub fn change_scope(&mut self, delta: isize) {
        self.scope = (self.scope as isize + delta).rem_euclid(self.scopes.len() as isize) as usize;
        self.selected = 0;
        self.delete_armed = false;
    }

    /// Switches the selected trigger or alias on or off. Returns whether anything changed.
    pub fn toggle(&self, rules: &mut Rules) -> bool {
        let enabled = match self.section {
            TRIGGERS => rules.triggers.get_mut(self.selected).map(|t| &mut t.enabled),
            ALIASES => rules.aliases.get_mut(self.selected).map(|a| &mut a.enabled),
            _ => None,
        };
        enabled.map(|e| *e = !*e).is_some()
    }

    /// Deletes the selected entry. Returns whether anything changed.
    pub fn delete(&mut self, rules: &mut Rules) -> bool {
        let i = self.selected;
        match self.section {
            TRIGGERS if i < rules.triggers.len() => drop(rules.triggers.remove(i)),
            ALIASES if i < rules.aliases.len() => drop(rules.aliases.remove(i)),
            MACROS if i < rules.macros.len() => drop(rules.macros.remove(i)),
            VARIABLES => match rules.variables.keys().nth(i).cloned() {
                Some(name) => drop(rules.variables.remove(&name)),
                None => return false,
            },
            _ => return false,
        }
        self.selected = i.saturating_sub(1);
        self.delete_armed = false;
        true
    }

    /// Moves the selected entry up or down its list (their order is the order they're tried
    /// in). Returns whether anything changed.
    pub fn move_by(&mut self, rules: &mut Rules, delta: isize) -> bool {
        let len = match self.section {
            TRIGGERS => rules.triggers.len(),
            ALIASES => rules.aliases.len(),
            MACROS => rules.macros.len(),
            _ => 0,
        };
        let (from, to) = (self.selected, self.selected as isize + delta);
        if from >= len || to < 0 || to as usize >= len {
            return false;
        }
        let to = to as usize;
        match self.section {
            TRIGGERS => rules.triggers.swap(from, to),
            ALIASES => rules.aliases.swap(from, to),
            _ => rules.macros.swap(from, to),
        }
        self.selected = to;
        true
    }

    /// A form for the selected entry, or for one to add (`new`, or nothing is selected).
    pub fn open_form(&mut self, config: &Config, new: bool) {
        let rules = config.rules(self.scope()).cloned().unwrap_or_default();
        let index = (!new).then_some(self.selected);
        let form = match self.section {
            TRIGGERS => RuleForm::trigger(index.and_then(|i| rules.triggers.get(i).cloned()), index),
            ALIASES => RuleForm::alias(index.and_then(|i| rules.aliases.get(i).cloned()), index),
            MACROS => RuleForm::keyboard_macro(index.and_then(|i| rules.macros.get(i).cloned()), index),
            _ => RuleForm::variable(index.and_then(|i| rules.variables.iter().nth(i)).map(|(k, v)| (k.clone(), v.clone()))),
        };
        self.form = Some(form);
        self.delete_armed = false;
    }

    pub fn paste(&mut self, text: &str) {
        if let Some(form) = &mut self.form {
            let field = &mut form.fields[form.focus];
            match field.kind {
                FieldKind::Toggle => {}
                FieldKind::Multi => field.input.insert(&text.replace("\r\n", "\n").replace('\r', "\n")),
                _ => field.input.insert(&text.replace(['\r', '\n'], " ")),
            }
        }
    }
}

/// What a form edits; the original is kept for the parts the form doesn't show.
enum Subject {
    Trigger(Box<Trigger>),
    Alias(Alias),
    Macro,
    /// The variable's name before editing.
    Variable(Option<String>),
}

pub struct RuleForm {
    subject: Subject,
    /// Its place in the list, or None for a new one.
    pub index: Option<usize>,
    pub fields: Vec<Field>,
    pub focus: usize,
    pub error: Option<String>,
    /// The next key pressed becomes the macro's key.
    pub recording: bool,
}

const TEST: &str = "Try it on";
const KEY: &str = "Key";

pub fn is_color(text: &str) -> bool {
    text.len() == 7 && text.starts_with('#') && text[1..].chars().all(|c| c.is_ascii_hexdigit())
}

impl RuleForm {
    fn new(subject: Subject, index: Option<usize>, fields: Vec<Field>) -> Self {
        Self { subject, index, fields, focus: 0, error: None, recording: false }
    }

    fn trigger(trigger: Option<Trigger>, index: Option<usize>) -> Self {
        let index = index.filter(|_| trigger.is_some());
        let t = trigger.unwrap_or_default();
        let h = t.highlight.clone().unwrap_or_default();
        let fields = vec![
            Field::text("Name", "Optional: shown in the list instead of the pattern", &t.name).group("Match"),
            Field::text("Pattern", "Text to look for in each line from the server", &t.matcher.pattern),
            Field::toggle("Regular expression", "PCRE-like syntax, with lookaround; groups are $1…$9 and ${name}", t.matcher.regex),
            Field::toggle("Match case", "", t.matcher.case_sensitive),
            Field::toggle("Whole words only", "For plain-text patterns", t.matcher.whole_word),
            Field::toggle("Enabled", "", t.enabled),
            Field::toggle("Stop here", "Don't run later triggers on lines this one matches", t.stop),
            Field::text(TEST, "A line from the game, to see what this trigger would do (not saved)", ""),
            Field::toggle("Hide the line", "", t.gag).group("Display"),
            Field::toggle("Leave it out of the log", "", t.gag_log),
            Field::toggle("Highlight", "Style the matching text with what follows", t.highlight.is_some()),
            Field::text("Text colour", "As #rrggbb; empty leaves it alone", h.fg.as_deref().unwrap_or("")),
            Field::text("Background", "As #rrggbb; empty leaves it alone", h.bg.as_deref().unwrap_or("")),
            Field::toggle("Bold", "", h.bold),
            Field::toggle("Italic", "", h.italic),
            Field::toggle("Underline", "", h.underline),
            Field::toggle("Whole line", "Otherwise only the matching text", h.whole_line),
            Field::toggle("Only the groups", "With a regular expression that has groups, style just those", h.groups),
            Field::toggle("Replace the match", "For example to censor words", t.replace.is_some()),
            Field::text("Replace with", "Empty removes the match; $1 and %name% work", t.replace.as_deref().unwrap_or("")),
            Field::multi("Send", "Commands to send, one per line (Alt+Enter for a new line). $1…$9, ${name} and %variable% work", &t.send).group("Actions"),
            Field::text("Copy to spawn window", "The name of a sidebar window to copy the line to", &t.spawn),
            Field::text("Play sound file", "", &t.sound),
            Field::text("Call script function", "", &t.script),
            Field::text("Speak aloud", "$0 is the whole line", &t.speak),
            Field::toggle("Desktop notification", "", t.notify),
            Field::toggle("Mark the tab important", "Shows a star on the tab until you look at it", t.important),
        ];
        Self::new(Subject::Trigger(Box::new(t)), index, fields)
    }

    fn alias(alias: Option<Alias>, index: Option<usize>) -> Self {
        let index = index.filter(|_| alias.is_some());
        let a = alias.unwrap_or_default();
        let fields = vec![
            Field::text("Pattern", "Plain text matches the first word you type; a regular expression can match anywhere", &a.matcher.pattern).group("Match"),
            Field::toggle("Regular expression", "Anchor it with ^ and $ to match the whole line", a.matcher.regex),
            Field::toggle("Match case", "", a.matcher.case_sensitive),
            Field::toggle("Replace just the match", "Everywhere it appears, then carry on with later aliases (as BeipMU does)", a.substitute),
            Field::toggle("Then stop", "Skip later aliases when this one matches", a.stop),
            Field::toggle("Enabled", "", a.enabled),
            Field::text(TEST, "Something you'd type, to see what would be sent (not saved)", ""),
            Field::multi("Send instead", "One command per line (Alt+Enter for a new line). $1…$9 are the arguments or groups, $* the rest, %name% a variable", &a.replacement)
                .group("Replacement"),
        ];
        Self::new(Subject::Alias(a), index, fields)
    }

    fn keyboard_macro(m: Option<Macro>, index: Option<usize>) -> Self {
        let index = index.filter(|_| m.is_some());
        let m = m.unwrap_or_default();
        let fields = vec![
            Field::text(KEY, "Press Ctrl+T and then the key, or type its name: F5, Ctrl+K, Shift+KP_8", &m.key),
            Field::text("Command", "Sent as if you'd typed it, so aliases apply", &m.command),
            Field::toggle("Put it in the input", "Instead of sending it", m.insert),
        ];
        Self::new(Subject::Macro, index, fields)
    }

    fn variable(variable: Option<(String, String)>) -> Self {
        let (name, value) = variable.clone().unwrap_or_default();
        let fields = vec![Field::text("Name", "Letters, digits and _", &name), Field::text("Value", "", &value)];
        Self::new(Subject::Variable(variable.map(|(name, _)| name)), None, fields)
    }

    /// What's being edited, for the window's title.
    pub fn title(&self) -> String {
        let (kind, new) = match &self.subject {
            Subject::Trigger(_) => ("trigger", self.index.is_none()),
            Subject::Alias(_) => ("alias", self.index.is_none()),
            Subject::Macro => ("macro", self.index.is_none()),
            Subject::Variable(old) => ("variable", old.is_none()),
        };
        format!("{} {kind}", if new { "New" } else { "Edit" })
    }

    pub fn focused(&mut self) -> &mut Field {
        &mut self.fields[self.focus]
    }

    pub fn next(&mut self) {
        self.focus = (self.focus + 1) % self.fields.len();
    }

    pub fn prev(&mut self) {
        self.focus = (self.focus + self.fields.len() - 1) % self.fields.len();
    }

    pub fn on_key_field(&self) -> bool {
        self.fields[self.focus].label == KEY
    }

    fn field(&self, label: &str) -> &Field {
        self.fields.iter().find(|f| f.label == label).expect("a field of this form")
    }

    fn text(&self, label: &str) -> String {
        self.field(label).input.text.trim().to_string()
    }

    fn on(&self, label: &str) -> bool {
        self.field(label).on
    }

    fn focus_on(&mut self, label: &str) {
        self.focus = self.fields.iter().position(|f| f.label == label).unwrap_or(0);
    }

    fn color(&mut self, label: &'static str) -> Result<Option<String>, String> {
        match self.text(label) {
            text if text.is_empty() => Ok(None),
            text if is_color(&text) => Ok(Some(text.to_lowercase())),
            _ => {
                self.focus_on(label);
                Err("Colours are written #rrggbb, like #ff8800".into())
            }
        }
    }

    fn built_trigger(&mut self) -> Result<Trigger, String> {
        let Subject::Trigger(original) = &self.subject else { unreachable!() };
        let mut t = (**original).clone();
        t.name = self.text("Name");
        t.matcher.pattern = self.field("Pattern").input.text.clone();
        t.matcher.regex = self.on("Regular expression");
        t.matcher.case_sensitive = self.on("Match case");
        t.matcher.whole_word = self.on("Whole words only");
        t.enabled = self.on("Enabled");
        t.stop = self.on("Stop here");
        t.gag = self.on("Hide the line");
        t.gag_log = self.on("Leave it out of the log");
        t.highlight = if self.on("Highlight") {
            Some(Highlight {
                fg: self.color("Text colour")?,
                bg: self.color("Background")?,
                bold: self.on("Bold"),
                italic: self.on("Italic"),
                underline: self.on("Underline"),
                whole_line: self.on("Whole line"),
                groups: self.on("Only the groups"),
            })
        } else {
            None
        };
        t.replace = self.on("Replace the match").then(|| self.field("Replace with").input.text.clone());
        t.send = self.field("Send").input.text.clone();
        t.spawn = self.text("Copy to spawn window");
        t.sound = self.text("Play sound file");
        t.script = self.text("Call script function");
        t.speak = self.text("Speak aloud");
        t.notify = self.on("Desktop notification");
        t.important = self.on("Mark the tab important");
        Ok(t)
    }

    fn built_alias(&self) -> Alias {
        let Subject::Alias(original) = &self.subject else { unreachable!() };
        let mut a = original.clone();
        a.matcher.pattern = self.field("Pattern").input.text.clone();
        a.matcher.regex = self.on("Regular expression");
        a.matcher.case_sensitive = self.on("Match case");
        a.substitute = self.on("Replace just the match");
        a.stop = self.on("Then stop");
        a.enabled = self.on("Enabled");
        a.replacement = self.field("Send instead").input.text.clone();
        a
    }

    /// What the rule would do with the text in the "try it" field, if there is any.
    pub fn test_result(&mut self) -> Option<String> {
        if !self.fields.iter().any(|f| f.label == TEST) {
            return None;
        }
        let line = self.field(TEST).input.text.clone();
        if line.is_empty() {
            return None;
        }
        let focus = self.focus;
        let result = match &self.subject {
            Subject::Trigger(_) => self.built_trigger().and_then(|t| familiar_engine::rules::test_trigger(&t, &line).map_err(|e| format!("Invalid pattern: {e}"))),
            Subject::Alias(_) => familiar_engine::rules::test_alias(&self.built_alias(), &line).map(|sent| sent.map(|s| format!("Sends: {}", s.join(" ⏎ ")))).map_err(|e| format!("Invalid pattern: {e}")),
            _ => return None,
        };
        // Checking mustn't move the focus the way a failed save does.
        self.focus = focus;
        Some(match result {
            Ok(Some(description)) => description,
            Ok(None) => "No match".into(),
            Err(e) => e,
        })
    }

    /// Checks the form and puts its rule into `rules`. Returns the row to select.
    pub fn save(&mut self, rules: &mut Rules) -> Result<usize, String> {
        let index = self.index;
        match &self.subject {
            Subject::Trigger(_) => {
                let trigger = self.built_trigger()?;
                if let Err(e) = familiar_engine::rules::compile_pattern(&trigger.matcher) {
                    self.focus_on("Pattern");
                    return Err(format!("Invalid pattern: {e}"));
                }
                match index {
                    Some(i) if i < rules.triggers.len() => rules.triggers[i] = trigger,
                    _ => rules.triggers.push(trigger),
                }
                Ok(index.unwrap_or(rules.triggers.len() - 1))
            }
            Subject::Alias(_) => {
                let alias = self.built_alias();
                let problem = if alias.matcher.pattern.is_empty() {
                    Some("Enter a pattern".to_string())
                } else if alias.matcher.regex {
                    familiar_engine::rules::compile_pattern(&alias.matcher).err().map(|e| format!("Invalid pattern: {e}"))
                } else {
                    None
                };
                if let Some(problem) = problem {
                    self.focus_on("Pattern");
                    return Err(problem);
                }
                match index {
                    Some(i) if i < rules.aliases.len() => rules.aliases[i] = alias,
                    _ => rules.aliases.push(alias),
                }
                Ok(index.unwrap_or(rules.aliases.len() - 1))
            }
            Subject::Macro => {
                let Some(key) = KeyChord::parse(&self.text(KEY)) else {
                    self.focus_on(KEY);
                    return Err("Choose a key first: Ctrl+T, then press it".into());
                };
                let new = Macro { key: key.to_string(), command: self.field("Command").input.text.clone(), insert: self.on("Put it in the input") };
                // One macro per key: this one replaces any other on the same key.
                let mut keep = 0;
                let mut place = None;
                for (i, m) in std::mem::take(&mut rules.macros).into_iter().enumerate() {
                    if Some(i) == index {
                        place = Some(keep);
                        rules.macros.push(new.clone());
                        keep += 1;
                    } else if KeyChord::parse(&m.key).is_none_or(|k| !k.matches(&key)) {
                        rules.macros.push(m);
                        keep += 1;
                    }
                }
                Ok(place.unwrap_or_else(|| {
                    rules.macros.push(new);
                    rules.macros.len() - 1
                }))
            }
            Subject::Variable(old) => {
                let name = self.text("Name").trim_matches('%').to_string();
                if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    self.focus_on("Name");
                    return Err("Variable names can only contain letters, digits and _".into());
                }
                if let Some(old) = old {
                    rules.variables.remove(old);
                }
                rules.variables.insert(name.clone(), self.field("Value").input.text.clone());
                Ok(rules.variables.keys().position(|k| *k == name).unwrap_or(0))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use familiar_config::{Character, World};

    fn config() -> Config {
        let mut config = Config::default();
        config.worlds.push(World { name: "Home".into(), characters: vec![Character { name: "Bob".into(), ..Character::default() }], ..World::default() });
        config
    }

    fn set(form: &mut RuleForm, label: &str, text: &str) {
        let field = form.fields.iter_mut().find(|f| f.label == label).unwrap();
        field.input.set(text);
    }

    fn switch(form: &mut RuleForm, label: &str, on: bool) {
        form.fields.iter_mut().find(|f| f.label == label).unwrap().on = on;
    }

    #[test]
    fn scopes_and_lists() {
        let mut config = config();
        let scope = Scope::Character { world: "Home".into(), character: "Bob".into() };
        let mut view = RulesView::new(&config, Some(&scope));
        assert_eq!(view.scopes.len(), 3);
        assert_eq!(view.scope(), &scope);
        assert!(view.rows(&config).is_empty());

        view.open_form(&config, true);
        let mut form = view.form.take().unwrap();
        assert_eq!(form.title(), "New trigger");
        set(&mut form, "Pattern", "^You are (hungry|thirsty)");
        switch(&mut form, "Regular expression", true);
        set(&mut form, "Send", "eat bread\ndrink water");
        set(&mut form, TEST, "You are hungry.");
        assert!(form.test_result().is_some_and(|r| r != "No match"));
        switch(&mut form, "Highlight", true);
        set(&mut form, "Text colour", "red");
        assert!(form.save(config.rules_mut(&scope).unwrap()).is_err(), "a colour that isn't #rrggbb");
        assert_eq!(form.fields[form.focus].label, "Text colour", "the problem field gets focus");
        set(&mut form, "Text colour", "#FF0000");
        assert_eq!(form.save(config.rules_mut(&scope).unwrap()), Ok(0));
        let saved = &config.worlds[0].characters[0].rules.triggers[0];
        assert_eq!(saved.highlight.as_ref().unwrap().fg.as_deref(), Some("#ff0000"));
        assert_eq!(saved.send, "eat bread\ndrink water");
        assert!(config.rules.is_empty(), "other scopes are untouched");

        let rows = view.rows(&config);
        assert_eq!(rows[0].title, "^You are (hungry|thirsty)");
        assert!(rows[0].detail.contains("regex") && rows[0].detail.contains("highlight"));
        let rules = config.rules_mut(&scope).unwrap();
        assert!(view.toggle(rules));
        assert!(!rules.triggers[0].enabled);
        assert!(view.delete(rules) && rules.triggers.is_empty());
        assert!(!view.delete(rules));
    }

    #[test]
    fn editing_keeps_what_the_form_does_not_show() {
        let mut config = config();
        config.rules.aliases.push(Alias { matcher: familiar_config::Pattern { pattern: "k".into(), whole_word: true, ..Default::default() }, replacement: "kill $*".into(), ..Alias::default() });
        config.rules.aliases.push(Alias { matcher: familiar_config::Pattern { pattern: "l".into(), ..Default::default() }, replacement: "look".into(), ..Alias::default() });
        let mut view = RulesView::new(&config, None);
        view.change_section(1);
        view.open_form(&config, false);
        let mut form = view.form.take().unwrap();
        assert_eq!(form.title(), "Edit alias");
        set(&mut form, TEST, "k orc");
        assert_eq!(form.test_result().as_deref(), Some("Sends: kill orc"));
        set(&mut form, "Send instead", "kick $*");
        assert_eq!(form.save(&mut config.rules), Ok(0));
        assert_eq!(config.rules.aliases.len(), 2);
        assert!(config.rules.aliases[0].matcher.whole_word);
        assert_eq!(config.rules.aliases[0].replacement, "kick $*");
        // Reordering.
        assert!(view.move_by(&mut config.rules, 1));
        assert_eq!((view.selected, config.rules.aliases[1].replacement.as_str()), (1, "kick $*"));
        assert!(!view.move_by(&mut config.rules, 1), "already last");
    }

    #[test]
    fn macros_and_variables() {
        let mut config = config();
        let mut view = RulesView::new(&config, None);
        view.section = MACROS;
        view.open_form(&config, true);
        let mut form = view.form.take().unwrap();
        assert!(form.save(&mut config.rules).is_err(), "no key yet");
        set(&mut form, KEY, "ctrl+k");
        set(&mut form, "Command", "kill");
        assert_eq!(form.save(&mut config.rules), Ok(0));
        assert_eq!(config.rules.macros[0].key, "Ctrl+K");
        // The same key again replaces the first.
        view.open_form(&config, true);
        let mut form = view.form.take().unwrap();
        set(&mut form, KEY, "Ctrl+K");
        set(&mut form, "Command", "kiss");
        assert_eq!(form.save(&mut config.rules), Ok(0));
        assert_eq!(config.rules.macros.iter().map(|m| m.command.as_str()).collect::<Vec<_>>(), ["kiss"]);

        view.section = VARIABLES;
        view.open_form(&config, true);
        let mut form = view.form.take().unwrap();
        set(&mut form, "Name", "my target");
        assert!(form.save(&mut config.rules).is_err());
        set(&mut form, "Name", "%target%");
        set(&mut form, "Value", "orc");
        form.save(&mut config.rules).unwrap();
        // Renaming replaces rather than adds.
        view.open_form(&config, false);
        let mut form = view.form.take().unwrap();
        assert_eq!(form.title(), "Edit variable");
        set(&mut form, "Name", "foe");
        form.save(&mut config.rules).unwrap();
        assert_eq!(config.rules.variables.iter().collect::<Vec<_>>(), [(&"foe".to_string(), &"orc".to_string())]);
        assert_eq!(view.rows(&config)[0].title, "%foe%");
    }
}
