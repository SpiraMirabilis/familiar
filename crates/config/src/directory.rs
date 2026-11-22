//! A built-in directory of public worlds, taken from BeipMU's `Worlds.xml` (MIT licensed).

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::{Character, World};

const WORLDS_XML: &str = include_str!("../Worlds.xml");

#[derive(Debug, Clone, PartialEq)]
pub struct DirectoryEntry {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub encoding: String,
    pub keywords: Vec<String>,
    /// Plain-text description with HTML stripped.
    pub description: String,
    pub website: Option<String>,
    /// Login command for a guest account, if the world has one.
    pub guest_login: Option<String>,
}

impl DirectoryEntry {
    pub fn to_world(&self) -> World {
        World {
            name: self.name.clone(),
            host: self.host.clone(),
            port: self.port,
            tls: self.tls,
            // BeipMU marks every world as CP1252 by default. Auto reads that too, and also shows
            // UTF-8 from servers that have since moved to it.
            encoding: if self.encoding.eq_ignore_ascii_case("CP1252") { "Auto".into() } else { self.encoding.clone() },
            characters: self
                .guest_login
                .iter()
                .map(|login| Character { name: "Guest".into(), login: login.clone(), ..Character::default() })
                .collect(),
            ..World::default()
        }
    }
}

pub fn entries() -> Vec<DirectoryEntry> {
    parse(WORLDS_XML)
}

/// Splits BeipMU's host field. Both "host:1234" and "host port:1234" occur in the file.
fn split_host(s: &str) -> Option<(String, u16)> {
    let (host, port) = s.rsplit_once(':')?;
    let host = host.trim().trim_end_matches(" port").trim();
    Some((host.to_string(), port.trim().parse().ok()?))
}

fn parse(xml: &str) -> Vec<DirectoryEntry> {
    let mut reader = Reader::from_str(xml.trim_start_matches('\u{feff}'));
    let mut entries = Vec::new();
    let mut current: Option<DirectoryEntry> = None;
    let mut in_description = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.name().as_ref() {
                b"world" => {
                    let attr = |name: &str| {
                        e.try_get_attribute(name)
                            .ok()
                            .flatten()
                            .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
                    };
                    let Some((host, port)) = attr("host").as_deref().and_then(split_host) else { continue };
                    current = Some(DirectoryEntry {
                        name: attr("name").unwrap_or_default(),
                        host,
                        port,
                        tls: attr("tls").as_deref() == Some("true"),
                        encoding: attr("encoding").unwrap_or_else(|| "UTF-8".into()),
                        keywords: attr("keywords")
                            .unwrap_or_default()
                            .split('|')
                            .filter(|k| !k.is_empty())
                            .map(|k| k.to_lowercase())
                            .collect(),
                        description: String::new(),
                        website: None,
                        guest_login: None,
                    });
                }
                b"description" => in_description = true,
                b"guest" => {
                    if let Some(entry) = current.as_mut() {
                        entry.guest_login = e
                            .try_get_attribute("connect")
                            .ok()
                            .flatten()
                            .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()));
                    }
                }
                _ => {}
            },
            Ok(Event::CData(text)) if in_description => {
                if let Some(entry) = current.as_mut() {
                    let html = String::from_utf8_lossy(&text);
                    entry.website = first_href(&html);
                    entry.description = strip_tags(&html).trim().to_string();
                }
            }
            Ok(Event::End(e)) => match e.name().as_ref() {
                b"description" => in_description = false,
                b"world" => entries.extend(current.take()),
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    entries
}

fn first_href(html: &str) -> Option<String> {
    let start = html.find("href=\"")? + 6;
    let end = html[start..].find('"')? + start;
    Some(html[start..end].to_string())
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("Website", "").trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bundled_directory() {
        let entries = entries();
        assert_eq!(entries.len(), 40);
        let lambda = &entries[0];
        assert_eq!((lambda.name.as_str(), lambda.host.as_str(), lambda.port), ("LambdaMOO", "lambda.moo.mud.org", 8888));
        assert_eq!(lambda.guest_login.as_deref(), Some("connect guest"));
        assert!(lambda.website.as_deref().unwrap().starts_with("https://"));
        assert!(!lambda.description.contains('<'));

        let spin = entries.iter().find(|e| e.name == "SpinDizzy Muck").unwrap();
        assert!(spin.tls);
        let arx = entries.iter().find(|e| e.name == "Arx").unwrap();
        assert_eq!((arx.host.as_str(), arx.port), ("play.arxmush.org", 3000));
    }

    #[test]
    fn host_formats() {
        assert_eq!(split_host("a.org:23"), Some(("a.org".into(), 23)));
        assert_eq!(split_host("a.org port:23"), Some(("a.org".into(), 23)));
        assert_eq!(split_host("a.org"), None);
    }
}
