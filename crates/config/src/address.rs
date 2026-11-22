//! Addresses as people type them into a quick-connect box.

use crate::World;

/// A typed address: `host:port`, `host port`, `[::1]:4000`, optionally followed by `tls`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub host: String,
    /// None if no port was given.
    pub port: Option<u16>,
    pub tls: bool,
}

impl Address {
    pub fn parse(text: &str) -> Option<Self> {
        let mut words = text.split_whitespace().peekable();
        let first = words.next()?;
        let (host, mut port) = if let Some(rest) = first.strip_prefix('[') {
            // A bracketed IPv6 address, as in URLs.
            let (host, after) = rest.split_once(']')?;
            let port = match after.strip_prefix(':') {
                Some(p) => Some(p.parse().ok()?),
                None if after.is_empty() => None,
                None => return None,
            };
            (host, port)
        } else if first.matches(':').count() > 1 {
            // A bare IPv6 address: its colons aren't a port separator.
            (first, None)
        } else {
            match first.split_once(':') {
                Some((host, p)) => (host, Some(p.parse().ok()?)),
                None => (first, None),
            }
        };
        if host.is_empty() {
            return None;
        }
        if port.is_none()
            && let Some(p) = words.peek().and_then(|w| w.parse().ok())
        {
            port = Some(p);
            words.next();
        }
        let tls = match words.next() {
            Some(w) if w.eq_ignore_ascii_case("tls") || w.eq_ignore_ascii_case("ssl") => true,
            Some(_) => return None,
            None => false,
        };
        if words.next().is_some() {
            return None;
        }
        Some(Self { host: host.to_string(), port, tls })
    }

    /// An unsaved world for this address. `default_port` is used if none was typed.
    pub fn into_world(self, default_port: u16) -> World {
        let port = self.port.unwrap_or(default_port);
        let name = if self.host.contains(':') { format!("[{}]:{port}", self.host) } else { format!("{}:{port}", self.host) };
        World { name, host: self.host, port, tls: self.tls, ..World::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::Address;

    fn parse(text: &str) -> Option<(String, Option<u16>, bool)> {
        Address::parse(text).map(|a| (a.host, a.port, a.tls))
    }

    #[test]
    fn hosts_and_ports() {
        assert_eq!(parse("a.org:4000"), Some(("a.org".into(), Some(4000), false)));
        assert_eq!(parse(" a.org 23 "), Some(("a.org".into(), Some(23), false)));
        assert_eq!(parse("a.org"), Some(("a.org".into(), None, false)));
        assert_eq!(parse("aardmud.org 4010 tls"), Some(("aardmud.org".into(), Some(4010), true)));
        assert_eq!(parse("a.org:4000 SSL"), Some(("a.org".into(), Some(4000), true)));
        assert_eq!(parse("a.org tls"), Some(("a.org".into(), None, true)));
    }

    #[test]
    fn ipv6() {
        assert_eq!(parse("[::1]:4000"), Some(("::1".into(), Some(4000), false)));
        assert_eq!(parse("[2001:db8::1] 4000 tls"), Some(("2001:db8::1".into(), Some(4000), true)));
        assert_eq!(parse("::1"), Some(("::1".into(), None, false)));
        assert_eq!(parse("2001:db8::1 23"), Some(("2001:db8::1".into(), Some(23), false)));
        assert_eq!(Address::parse("[::1]:4000").unwrap().into_world(23).name, "[::1]:4000");
    }

    #[test]
    fn rejects_nonsense() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("a.org:x"), None);
        assert_eq!(parse(":4000"), None);
        assert_eq!(parse("a.org 4000 extra"), None);
        assert_eq!(parse("a.org 4000 tls more"), None);
        assert_eq!(parse("[::1"), None);
        assert_eq!(parse("a.org:99999"), None);
    }
}
