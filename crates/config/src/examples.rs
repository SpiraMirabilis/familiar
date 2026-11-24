//! A starter set of triggers, aliases, keyboard macros and a variable, for the rules editors'
//! "Add Examples" button. They aren't written for any one game: each is there to show one
//! thing the rules can do, and is meant to be read, changed and thrown away.
//!
//! Triggers and aliases are added switched off, so nothing happens in a real world until the
//! user turns one on. Macros have no off switch, so they're on keys that are rarely used.

use crate::rules::{Alias, Highlight, Macro, Pattern, Rules, Trigger};

/// What every example trigger's name starts with, so they're easy to tell from the user's own.
pub const PREFIX: &str = "Example: ";

fn plain(pattern: &str) -> Pattern {
    Pattern { pattern: pattern.into(), ..Pattern::default() }
}

fn regex(pattern: &str) -> Pattern {
    Pattern { pattern: pattern.into(), regex: true, ..Pattern::default() }
}

fn trigger(name: &str, matcher: Pattern) -> Trigger {
    Trigger { name: format!("{PREFIX}{name}"), enabled: false, matcher, ..Trigger::default() }
}

fn alias(matcher: Pattern, replacement: &str) -> Alias {
    Alias { enabled: false, matcher, replacement: replacement.into(), ..Alias::default() }
}

fn key(key: &str, command: &str) -> Macro {
    Macro { key: key.into(), command: command.into(), insert: false }
}

/// The examples, simplest first within each kind.
pub fn examples() -> Rules {
    let triggers = vec![
        // Plain text, matched as a whole word, and a highlight on just that word.
        Trigger {
            highlight: Some(Highlight { fg: Some("#ffd75f".into()), bold: true, ..Highlight::default() }),
            ..trigger("highlight a word", Pattern { whole_word: true, ..plain("treasure") })
        },
        // The highlight can cover the whole line, and the tab can be marked.
        Trigger {
            highlight: Some(Highlight { fg: Some("#7ee787".into()), whole_line: true, ..Highlight::default() }),
            important: true,
            ..trigger("colour the whole line and mark the tab", plain("tells you"))
        },
        // A line can be hidden.
        Trigger { gag: true, ..trigger("hide a line", plain("The wind howls.")) },
        // A regular expression: ^ is the start of the line, \w+ a word, (?:a|b) either of two.
        Trigger { notify: true, ..trigger("notify me, with a regular expression", regex(r"^\w+ (?:pages|whispers to you)[:,]")) },
        // What the brackets match is $1 in what's sent. Each line sent is its own command.
        Trigger { send: "wave $1\nsay Hello, $1!".into(), ..trigger("answer, using what matched", regex(r"^(\w+) waves at you\.$")) },
        // Part of a line can be rewritten: "<Public> hi" becomes "[Public] hi".
        Trigger { replace: Some("[$1] ".into()), ..trigger("rewrite part of a line", regex(r"^<(\w+)> ")) },
        // A copy of the line goes to a window at the side.
        Trigger { spawn: "Channels".into(), ..trigger("copy a channel to a side window", regex(r"^\[(?:Public|Newbie|Chat)\] ")) },
        // %target% is a variable: set it on the Variables page, or with /set in the world.
        Trigger { send: "poke %target%".into(), ..trigger("send a command that uses a variable", plain("You feel playful.")) },
    ];
    let aliases = vec![
        // Plain text is the first word of what's typed.
        alias(plain("hi"), "say Hello, everyone!"),
        // $1 and $2 are the words after it: "gt lamp Mara" sends "give lamp to Mara".
        alias(plain("gt"), "give $1 to $2"),
        // $* is everything after it.
        alias(plain("ooc"), "say (OOC) $*"),
        // Several lines send several commands, and %target% is a variable.
        alias(plain("greet"), "wave %target%\nsay Good to see you, %target%!"),
        // A regular expression can match the whole of what's typed: 'hello sends "say hello".
        alias(regex(r"^'(.+)$"), "say $1"),
        // Replacing only what matched, wherever it is: "say brb, tea" sends "say be right back, tea".
        Alias { substitute: true, ..alias(plain("brb"), "be right back") },
    ];
    let macros = vec![
        key("F2", "look"),
        // This one types into the input box and leaves the rest to you.
        Macro { insert: true, ..key("Shift+F2", "say ") },
        key("Alt+Up", "north"),
        key("Alt+Down", "south"),
        key("Alt+Left", "west"),
        key("Alt+Right", "east"),
    ];
    Rules { triggers, aliases, macros, variables: [("target".to_string(), "everyone".to_string())].into() }
}

/// How many of each kind [`Rules::add_examples`] added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Added {
    pub triggers: usize,
    pub aliases: usize,
    pub macros: usize,
    pub variables: usize,
}

impl Added {
    pub fn total(&self) -> usize {
        self.triggers + self.aliases + self.macros + self.variables
    }

    /// A sentence about what was added, for the editors to show.
    pub fn describe(&self) -> String {
        if self.total() == 0 {
            return "The examples are all here already.".into();
        }
        let count = |n: usize, one: &str, many: &str| (n > 0).then(|| format!("{n} {}", if n == 1 { one } else { many }));
        let parts: Vec<String> = [
            count(self.triggers, "trigger", "triggers"),
            count(self.aliases, "alias", "aliases"),
            count(self.macros, "macro", "macros"),
            count(self.variables, "variable", "variables"),
        ]
        .into_iter()
        .flatten()
        .collect();
        let list = match parts.as_slice() {
            [one] => one.clone(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
            [] => String::new(),
        };
        let off = if self.triggers + self.aliases > 0 { " Triggers and aliases start switched off." } else { "" };
        format!("Added {list}.{off}")
    }
}

impl Rules {
    /// Adds the examples that aren't here yet, after what is. One counts as here if there's a
    /// trigger with its name, an alias with its pattern, a macro on its key or a variable with
    /// its name, so pressing the button twice adds nothing, and nothing of the user's is
    /// replaced.
    pub fn add_examples(&mut self) -> Added {
        let examples = examples();
        let mut added = Added::default();
        for trigger in examples.triggers {
            if !self.triggers.iter().any(|t| t.name == trigger.name) {
                self.triggers.push(trigger);
                added.triggers += 1;
            }
        }
        for alias in examples.aliases {
            if !self.aliases.iter().any(|a| a.matcher.pattern == alias.matcher.pattern) {
                self.aliases.push(alias);
                added.aliases += 1;
            }
        }
        for m in examples.macros {
            if !self.macros.iter().any(|existing| existing.key.eq_ignore_ascii_case(&m.key)) {
                self.macros.push(m);
                added.macros += 1;
            }
        }
        for (name, value) in examples.variables {
            if let std::collections::btree_map::Entry::Vacant(slot) = self.variables.entry(name) {
                slot.insert(value);
                added.variables += 1;
            }
        }
        added
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn examples_start_switched_off_and_are_named() {
        let examples = examples();
        assert!(examples.triggers.iter().all(|t| !t.enabled && t.name.starts_with(PREFIX)));
        assert!(examples.aliases.iter().all(|a| !a.enabled));
        assert!(!examples.macros.is_empty() && !examples.variables.is_empty());
    }

    #[test]
    fn adding_twice_adds_once_and_keeps_what_was_there() {
        let mut rules = Rules::default();
        rules.macros.push(key("f2", "inventory"));
        rules.variables.insert("target".into(), "Mara".into());
        let first = rules.add_examples();
        let all = examples();
        assert_eq!(first, Added { triggers: all.triggers.len(), aliases: all.aliases.len(), macros: all.macros.len() - 1, variables: 0 });
        // The user's own macro on F2 and their variable are left alone.
        assert_eq!(rules.macros[0].command, "inventory");
        assert_eq!(rules.variables["target"], "Mara");
        let again = rules.add_examples();
        assert_eq!(again.total(), 0);
        assert_eq!(again.describe(), "The examples are all here already.");
    }

    #[test]
    fn describes_what_was_added() {
        let added = Added { triggers: 8, aliases: 1, macros: 0, variables: 1 };
        assert_eq!(added.describe(), "Added 8 triggers, 1 alias and 1 variable. Triggers and aliases start switched off.");
        assert_eq!(Added { macros: 2, ..Added::default() }.describe(), "Added 2 macros.");
    }
}
