//! Puppets: second characters played through this connection, each with a tab of its own.

use super::*;

impl Engine {
    pub fn puppets(&self) -> impl Iterator<Item = &Puppet> {
        self.puppets.iter().map(|(p, _)| p)
    }

    /// The UI opened or closed a puppet's tab.
    pub fn set_puppet_open(&mut self, name: &str, open: bool) {
        if open {
            self.open_puppets.insert(name.to_string());
        } else {
            self.open_puppets.remove(name);
        }
    }

    /// Which puppet a line belongs to, and the byte range of its prefix.
    pub(super) fn match_puppet(&self, plain: &str) -> Option<(usize, usize, usize)> {
        self.puppets.iter().enumerate().find_map(|(i, (p, regex))| match regex {
            Some(re) => {
                let caps = re.captures(plain).ok().flatten()?;
                // With a group, the group is the part to hide; otherwise the whole match.
                let m = caps.get(1).or_else(|| caps.get(0))?;
                Some((i, m.start(), m.end()))
            }
            None => {
                let prefix = &p.receive_prefix;
                (plain.len() >= prefix.len() && plain.is_char_boundary(prefix.len()) && plain[..prefix.len()].eq_ignore_ascii_case(prefix))
                    .then_some((i, 0, prefix.len()))
            }
        })
    }

    /// Sends text typed in a puppet's tab, with the puppet's send prefix.
    pub fn puppet_submit(&mut self, name: &str, text: &str) -> Vec<Output> {
        let mut out = Vec::new();
        let Some((puppet, _)) = self.puppets.iter().find(|(p, _)| p.name == name).cloned() else {
            out.push(Output::Notice(format!("There's no puppet called {name}")));
            return out;
        };
        let lines: Vec<&str> = if text.is_empty() { vec![""] } else { text.lines().collect() };
        for line in lines {
            let line = match line.strip_prefix(puppet.send_prefix.as_str()) {
                Some(rest) if puppet.remove_accidental_prefix && !puppet.send_prefix.is_empty() => rest,
                _ => line,
            };
            let command = format!("{}{line}", puppet.send_prefix);
            if self.send_raw(&command) {
                out.push(Output::Sent(command));
                if self.settings.local_echo {
                    out.push(Output::PuppetEcho { name: name.to_string(), text: line.to_string() });
                }
            } else {
                out.push(Output::Notice(format!("Not connected. {}", self.reconnect_hint())));
                break;
            }
        }
        out
    }

    pub(super) fn open_puppet(&mut self, name: &str, focus: bool, out: &mut Vec<Output>) {
        self.open_puppets.insert(name.to_string());
        out.push(Output::OpenPuppet { name: name.to_string(), focus });
    }
}
