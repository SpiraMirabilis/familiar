//! Rules compiled for a session: regexes built, layers flattened in priority order.

use mu_config::{Alias, Macro, Pattern, Rules, Trigger};

use crate::KeyChord;
use crate::expand::Captures;

/// Turns a pattern into a regex. Plain text is escaped, so it matches literally.
pub fn compile_pattern(p: &Pattern) -> Result<fancy_regex::Regex, String> {
    if p.pattern.is_empty() {
        return Err("the pattern is empty".into());
    }
    let mut source = if p.regex { p.pattern.clone() } else { fancy_regex::escape(&p.pattern).into_owned() };
    if p.whole_word && !p.regex {
        source = format!(r"\b{source}\b");
    }
    if !p.case_sensitive {
        source = format!("(?i){source}");
    }
    fancy_regex::Regex::new(&source).map_err(|e| e.to_string())
}

pub struct CompiledTrigger {
    pub trigger: Trigger,
    pub regex: fancy_regex::Regex,
    /// Turned off for this session because it fired too often.
    pub suspended: bool,
    pub recent_sends: std::collections::VecDeque<std::time::Instant>,
}

impl CompiledTrigger {
    pub fn label(&self) -> &str {
        if self.trigger.name.is_empty() { &self.trigger.matcher.pattern } else { &self.trigger.name }
    }
}

pub struct CompiledAlias {
    pub alias: Alias,
    /// Only for regex aliases. Plain aliases compare the first word.
    pub regex: Option<fancy_regex::Regex>,
}

impl CompiledAlias {
    /// Returns the captures if the input matches.
    pub fn matches(&self, input: &str) -> Option<Captures> {
        match &self.regex {
            Some(re) => re.captures(input).ok().flatten().map(|c| Captures::from_regex(re, &c)),
            None => {
                let first = input.split_whitespace().next()?;
                let pattern = self.alias.matcher.pattern.trim();
                let hit = if self.alias.matcher.case_sensitive {
                    first == pattern
                } else {
                    first.to_lowercase() == pattern.to_lowercase()
                };
                hit.then(|| Captures::from_words(input))
            }
        }
    }
}

#[derive(Default)]
pub struct CompiledRules {
    pub triggers: Vec<CompiledTrigger>,
    pub aliases: Vec<CompiledAlias>,
    pub macros: Vec<(KeyChord, Macro)>,
}

impl CompiledRules {
    /// Compiles rule layers, most specific first. Disabled rules are skipped. Rules that fail
    /// to compile are skipped too, and described in the returned errors.
    pub fn compile<'a>(layers: impl IntoIterator<Item = &'a Rules>) -> (Self, Vec<String>) {
        let mut compiled = Self::default();
        let mut errors = Vec::new();
        for rules in layers {
            for trigger in rules.triggers.iter().filter(|t| t.enabled) {
                match compile_pattern(&trigger.matcher) {
                    Ok(regex) => compiled.triggers.push(CompiledTrigger {
                        trigger: trigger.clone(),
                        regex,
                        suspended: false,
                        recent_sends: Default::default(),
                    }),
                    Err(e) => errors.push(format!("Trigger \"{}\" was skipped: {e}", trigger.matcher.pattern)),
                }
            }
            for alias in rules.aliases.iter().filter(|a| a.enabled) {
                if alias.matcher.pattern.trim().is_empty() {
                    errors.push("An alias with an empty pattern was skipped".into());
                    continue;
                }
                let regex = if alias.matcher.regex {
                    match compile_pattern(&alias.matcher) {
                        Ok(re) => Some(re),
                        Err(e) => {
                            errors.push(format!("Alias \"{}\" was skipped: {e}", alias.matcher.pattern));
                            continue;
                        }
                    }
                } else {
                    None
                };
                compiled.aliases.push(CompiledAlias { alias: alias.clone(), regex });
            }
            for m in &rules.macros {
                match KeyChord::parse(&m.key) {
                    Some(chord) => compiled.macros.push((chord, m.clone())),
                    None => errors.push(format!("Macro with key \"{}\" was skipped: unknown key", m.key)),
                }
            }
        }
        (compiled, errors)
    }

    pub fn macro_for(&self, chord: &KeyChord) -> Option<&Macro> {
        self.macros.iter().find(|(c, _)| c.matches(chord)).map(|(_, m)| m)
    }
}

/// Describes what a trigger does with a test line, for the editor.
pub fn test_trigger(trigger: &Trigger, line: &str) -> Result<Option<String>, String> {
    let regex = compile_pattern(&trigger.matcher)?;
    let Some(caps) = regex.captures(line).map_err(|e| e.to_string())? else { return Ok(None) };
    let mut parts = vec![format!("Matched \"{}\"", caps.get(0).map_or("", |m| m.as_str()))];
    for i in 1..caps.len() {
        if let Some(m) = caps.get(i) {
            parts.push(format!("${i} = \"{}\"", m.as_str()));
        }
    }
    if !trigger.send.is_empty() {
        let caps = Captures::from_regex(&regex, &caps);
        let sent = crate::expand::expand(&trigger.send, &caps, |_| None);
        parts.push(format!("sends \"{}\"", sent.replace('\n', "; ")));
    }
    Ok(Some(parts.join(", ")))
}

/// What an alias would send for a test input, for the editor. Ok(None) means it doesn't match.
pub fn test_alias(alias: &Alias, input: &str) -> Result<Option<Vec<String>>, String> {
    if alias.matcher.pattern.trim().is_empty() {
        return Err("the pattern is empty".into());
    }
    let regex = if alias.matcher.regex { Some(compile_pattern(&alias.matcher)?) } else { None };
    let compiled = CompiledAlias { alias: alias.clone(), regex };
    Ok(compiled.matches(input).map(|caps| {
        crate::expand::expand(&alias.replacement, &caps, |_| None).lines().map(str::to_string).collect()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(p: &str) -> Pattern {
        Pattern { pattern: p.into(), ..Pattern::default() }
    }

    #[test]
    fn plain_patterns_are_literal_and_case_insensitive() {
        let re = compile_pattern(&pattern("a.b (c)")).unwrap();
        assert!(re.is_match("xx A.B (C) yy").unwrap());
        assert!(!re.is_match("aXb (c)").unwrap());
        let re = compile_pattern(&Pattern { case_sensitive: true, ..pattern("Bob") }).unwrap();
        assert!(!re.is_match("bob").unwrap());
    }

    #[test]
    fn whole_word_and_lookaround() {
        let re = compile_pattern(&Pattern { whole_word: true, ..pattern("cat") }).unwrap();
        assert!(re.is_match("a cat sat").unwrap());
        assert!(!re.is_match("concatenate").unwrap());
        let re = compile_pattern(&Pattern { regex: true, ..pattern(r"(?<!not )ready") }).unwrap();
        assert!(re.is_match("ready").unwrap());
        assert!(!re.is_match("not ready").unwrap());
    }

    #[test]
    fn compile_reports_errors_and_skips_disabled() {
        let rules = Rules {
            triggers: vec![
                Trigger { matcher: Pattern { regex: true, ..pattern("(") }, ..Trigger::default() },
                Trigger { enabled: false, matcher: pattern("x"), ..Trigger::default() },
                Trigger { matcher: pattern("ok"), ..Trigger::default() },
            ],
            macros: vec![Macro { key: "Bogus+K".into(), command: "x".into() }],
            ..Rules::default()
        };
        let (compiled, errors) = CompiledRules::compile([&rules]);
        assert_eq!(compiled.triggers.len(), 1);
        assert_eq!(errors.len(), 2, "{errors:?}");
    }

    #[test]
    fn plain_alias_matches_first_word_only() {
        let alias = CompiledAlias { alias: Alias { matcher: pattern("k"), ..Alias::default() }, regex: None };
        assert!(alias.matches("k orc").is_some());
        assert!(alias.matches("K").is_some());
        assert!(alias.matches("kick orc").is_none());
        assert!(alias.matches("").is_none());
    }

    #[test]
    fn alias_test() {
        let alias = Alias { matcher: pattern("k"), replacement: "kill $1\nloot".into(), ..Alias::default() };
        assert_eq!(test_alias(&alias, "k orc").unwrap(), Some(vec!["kill orc".into(), "loot".into()]));
        assert_eq!(test_alias(&alias, "look").unwrap(), None);
        let broken = Alias { matcher: Pattern { regex: true, ..pattern("(") }, ..Alias::default() };
        assert!(test_alias(&broken, "x").is_err());
    }

    #[test]
    fn trigger_test_description() {
        let t = Trigger {
            matcher: Pattern { regex: true, ..pattern(r"(\w+) attacks you") },
            send: "kill $1".into(),
            ..Trigger::default()
        };
        assert_eq!(
            test_trigger(&t, "The orc attacks you!").unwrap().unwrap(),
            "Matched \"orc attacks you\", $1 = \"orc\", sends \"kill orc\""
        );
        assert_eq!(test_trigger(&t, "nothing").unwrap(), None);
    }
}
