//! Triggers, aliases, keyboard macros and variables. Each can be set globally, per world and per
//! character.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Rules {
    #[serde(rename = "trigger", skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<Trigger>,
    #[serde(rename = "alias", skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<Alias>,
    #[serde(rename = "macro", skip_serializing_if = "Vec::is_empty")]
    pub macros: Vec<Macro>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub variables: BTreeMap<String, String>,
}

impl Rules {
    pub fn is_empty(&self) -> bool {
        self.triggers.is_empty() && self.aliases.is_empty() && self.macros.is_empty() && self.variables.is_empty()
    }
}

/// How a trigger or alias pattern is matched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Pattern {
    pub pattern: String,
    /// Treat `pattern` as a regular expression (PCRE-like syntax, with lookaround).
    #[serde(skip_serializing_if = "is_false")]
    pub regex: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub case_sensitive: bool,
    /// For plain-text patterns: only match whole words.
    #[serde(skip_serializing_if = "is_false")]
    pub whole_word: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Trigger {
    /// Optional label shown in the editor.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub enabled: bool,
    #[serde(flatten)]
    pub matcher: Pattern,
    /// Don't run any later triggers on a line this one matched.
    #[serde(skip_serializing_if = "is_false")]
    pub stop: bool,

    /// Hide the line from the output window.
    #[serde(skip_serializing_if = "is_false")]
    pub gag: bool,
    /// Also leave the line out of the log.
    #[serde(skip_serializing_if = "is_false")]
    pub gag_log: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub highlight: Option<Highlight>,
    /// Commands to send, one per line. `$1`, `${name}` and `%var%` are expanded.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub send: String,
    /// Copy the line to the spawn window with this name.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub spawn: String,
    /// Sound file to play.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sound: String,
    /// Show a desktop notification with the line.
    #[serde(skip_serializing_if = "is_false")]
    pub notify: bool,
    /// Mark the tab as needing attention.
    #[serde(skip_serializing_if = "is_false")]
    pub important: bool,
    /// Text to speak, with $1 and %var% expanded; "$0" speaks the whole line.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub speak: String,
    /// Name of a script function to call with the line and the regex groups. A `beipmu:` prefix
    /// calls it the way BeipMU did: `(ranges, line, window)`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub script: String,
    /// Replace each match with this (`$1` and `%var%` expanded); an empty string deletes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replace: Option<String>,
}

impl Default for Trigger {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: true,
            matcher: Pattern::default(),
            stop: false,
            gag: false,
            gag_log: false,
            highlight: None,
            send: String::new(),
            spawn: String::new(),
            sound: String::new(),
            notify: false,
            important: false,
            speak: String::new(),
            script: String::new(),
            replace: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Highlight {
    /// Colours as "#rrggbb".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bg: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub italic: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub underline: bool,
    /// Style the whole line rather than just the matched text.
    #[serde(skip_serializing_if = "is_false")]
    pub whole_line: bool,
    /// Style only the regex's groups (when it has any), as BeipMU does.
    #[serde(skip_serializing_if = "is_false")]
    pub groups: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Alias {
    pub enabled: bool,
    /// Plain text matches the first word of the input. A regex can match anywhere in the input;
    /// anchor it with ^ and $ to match the whole line.
    #[serde(flatten)]
    pub matcher: Pattern,
    /// What to send instead, one command per line. `$1`..`$9` are the arguments (plain) or
    /// captures (regex), `$*` is everything after the first word, and `%var%` is a variable.
    pub replacement: String,
    /// Replace only the matched text, everywhere it appears in the input, then carry on with
    /// later aliases (BeipMU's behaviour). Plain patterns then match anywhere, not just the first
    /// word.
    #[serde(skip_serializing_if = "is_false")]
    pub substitute: bool,
    /// With `substitute`: don't apply any later aliases after this one matches.
    #[serde(skip_serializing_if = "is_false")]
    pub stop: bool,
}

impl Default for Alias {
    fn default() -> Self {
        Self { enabled: true, matcher: Pattern::default(), replacement: String::new(), substitute: false, stop: false }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Macro {
    /// A key chord like "F5", "Ctrl+K" or "Shift+KP_8". See `mu_engine::KeyChord`.
    pub key: String,
    /// Sent as if typed, so aliases apply.
    pub command: String,
    /// Put the command in the input box instead of sending it.
    #[serde(skip_serializing_if = "is_false")]
    pub insert: bool,
}

/// Where a set of rules lives.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Scope {
    Global,
    World { world: String },
    Character { world: String, character: String },
}

impl Scope {
    pub fn label(&self) -> String {
        match self {
            Scope::Global => "All worlds".into(),
            Scope::World { world } => world.clone(),
            Scope::Character { world, character } => format!("{character} @ {world}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_toml_is_flat_and_tidy() {
        let rules = Rules {
            triggers: vec![Trigger {
                matcher: Pattern { pattern: "^You are hungry".into(), regex: true, ..Pattern::default() },
                highlight: Some(Highlight { fg: Some("#ff0000".into()), ..Highlight::default() }),
                send: "eat bread".into(),
                ..Trigger::default()
            }],
            ..Rules::default()
        };
        let text = toml::to_string(&rules).unwrap();
        assert!(text.contains("pattern = \"^You are hungry\""), "{text}");
        assert!(!text.contains("gag"), "{text}");
        assert_eq!(toml::from_str::<Rules>(&text).unwrap(), rules);
    }

    #[test]
    fn enabled_defaults_to_true() {
        let rules: Rules = toml::from_str("[[trigger]]\npattern = \"x\"\n[[alias]]\npattern = \"k\"").unwrap();
        assert!(rules.triggers[0].enabled);
        assert!(rules.aliases[0].enabled);
    }
}
