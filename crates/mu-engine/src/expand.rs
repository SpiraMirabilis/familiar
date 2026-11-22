//! Expands `$1`, `${name}`, `$*` and `%var%` in trigger sends and alias replacements.

use std::collections::HashMap;

/// Values available to a template: regex captures or alias arguments.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Captures {
    /// `$0`..`$9` and beyond. `$0` is the whole match.
    pub numbered: Vec<Option<String>>,
    pub named: HashMap<String, String>,
    /// `$*`: everything after the first word (aliases only).
    pub rest: Option<String>,
}

impl Captures {
    pub fn from_regex(regex: &fancy_regex::Regex, caps: &fancy_regex::Captures<str>) -> Self {
        let numbered = (0..caps.len()).map(|i| caps.get(i).map(|m| m.as_str().to_string())).collect();
        let named = regex
            .capture_names()
            .flatten()
            .filter_map(|name| caps.name(name).map(|m| (name.to_string(), m.as_str().to_string())))
            .collect();
        Self { numbered, named, rest: None }
    }

    /// For plain aliases: `$0` is the whole input, `$1`.. are the words after the first, and
    /// `$*` is everything after the first word.
    pub fn from_words(input: &str) -> Self {
        let trimmed = input.trim_start();
        let rest = trimmed.split_once(char::is_whitespace).map(|(_, r)| r.trim_start().to_string());
        let mut numbered = vec![Some(input.to_string())];
        numbered.extend(rest.as_deref().unwrap_or("").split_whitespace().map(|w| Some(w.to_string())));
        Self { numbered, named: HashMap::new(), rest: Some(rest.unwrap_or_default()) }
    }

    fn group(&self, i: usize) -> &str {
        self.numbered.get(i).and_then(|g| g.as_deref()).unwrap_or("")
    }
}

fn is_var_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Expands a template. Unknown `%var%` names are left as they are, so a literal `%` rarely needs
/// escaping. `$$` and `%%` produce a single `$` or `%`.
pub fn expand(template: &str, caps: &Captures, var: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find(['$', '%']) {
        out.push_str(&rest[..i]);
        let sigil = rest.as_bytes()[i] as char;
        let after = &rest[i + 1..];
        if sigil == '$' {
            let mut chars = after.chars();
            match chars.next() {
                Some('$') => {
                    out.push('$');
                    rest = &after[1..];
                }
                Some('*') => {
                    out.push_str(caps.rest.as_deref().unwrap_or(""));
                    rest = &after[1..];
                }
                Some(d) if d.is_ascii_digit() => {
                    out.push_str(caps.group(d as usize - '0' as usize));
                    rest = &after[1..];
                }
                Some('{') => match after.find('}') {
                    Some(end) => {
                        let name = &after[1..end];
                        match name.parse::<usize>() {
                            Ok(n) => out.push_str(caps.group(n)),
                            Err(_) => out.push_str(caps.named.get(name).map(String::as_str).unwrap_or("")),
                        }
                        rest = &after[end + 1..];
                    }
                    None => {
                        out.push('$');
                        rest = after;
                    }
                },
                _ => {
                    out.push('$');
                    rest = after;
                }
            }
        } else {
            if let Some(stripped) = after.strip_prefix('%') {
                out.push('%');
                rest = stripped;
                continue;
            }
            let name_len = after.find(|c: char| !is_var_char(c)).unwrap_or(after.len());
            let value = (name_len > 0 && after[name_len..].starts_with('%'))
                .then(|| var(&after[..name_len]))
                .flatten();
            match value {
                Some(value) => {
                    out.push_str(&value);
                    rest = &after[name_len + 1..];
                }
                None => {
                    out.push('%');
                    rest = after;
                }
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(name: &str) -> Option<String> {
        match name {
            "target" => Some("orc".into()),
            "hp_max" => Some("100".into()),
            _ => None,
        }
    }

    #[test]
    fn numbered_named_and_vars() {
        let re = fancy_regex::Regex::new(r"(?<who>\w+) says (.*)").unwrap();
        let caps = re.captures("Bob says hi there").unwrap().unwrap();
        let caps = Captures::from_regex(&re, &caps);
        assert_eq!(expand("tell ${who} you said: $2", &caps, vars), "tell Bob you said: hi there");
        assert_eq!(expand("$1/$9/${7}", &caps, vars), "Bob//");
        assert_eq!(expand("kill %target% at %hp_max%%%", &caps, vars), "kill orc at 100%");
        assert_eq!(expand("100% sure %unknown% $$5", &caps, vars), "100% sure %unknown% $5");
        assert_eq!(expand("trailing $ and %", &caps, vars), "trailing $ and %");
        assert_eq!(expand("open ${who", &caps, vars), "open ${who");
    }

    #[test]
    fn alias_words() {
        let caps = Captures::from_words("k  orc  big");
        assert_eq!(expand("kill $1 then $2 [$*] {$0}", &caps, vars), "kill orc then big [orc  big] {k  orc  big}");
        let caps = Captures::from_words("look");
        assert_eq!(expand("l $1|$*", &caps, vars), "l |");
    }

    #[test]
    fn unicode_is_preserved() {
        let caps = Captures::from_words("say héllo wörld");
        assert_eq!(expand("« $* » ✓", &caps, vars), "« héllo wörld » ✓");
    }
}
