//! The form for adding or editing a saved world.

use familiar_config::{Character, Config, World};

use crate::input::Input;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    /// Shown as dots unless it has focus.
    Secret,
    Toggle,
    /// Text of several lines.
    Multi,
}

pub struct Field {
    pub label: &'static str,
    pub hint: &'static str,
    pub kind: FieldKind,
    pub input: Input,
    pub on: bool,
    /// A heading shown above the first field of each group.
    pub group: &'static str,
}

impl Field {
    pub fn text(label: &'static str, hint: &'static str, value: &str) -> Self {
        let mut input = Input::new(1);
        input.set(value);
        Self { label, hint, kind: FieldKind::Text, input, on: false, group: "" }
    }

    pub fn multi(label: &'static str, hint: &'static str, value: &str) -> Self {
        Self { kind: FieldKind::Multi, ..Self::text(label, hint, value) }
    }

    pub fn toggle(label: &'static str, hint: &'static str, on: bool) -> Self {
        Self { kind: FieldKind::Toggle, on, ..Self::text(label, hint, "") }
    }

    pub fn group(mut self, group: &'static str) -> Self {
        self.group = group;
        self
    }
}

pub const NAME: usize = 0;
pub const HOST: usize = 1;
pub const PORT: usize = 2;
pub const TLS: usize = 3;
pub const BOUNCER: usize = 4;
pub const CHARACTER: usize = 5;
pub const LOGIN: usize = 6;

pub struct WorldForm {
    pub fields: Vec<Field>,
    pub focus: usize,
    /// The saved world being edited (its name before any change), or None for a new one.
    pub editing: Option<String>,
    /// The character being edited, so renaming it updates rather than adds.
    pub editing_character: Option<String>,
    pub error: Option<String>,
}

impl WorldForm {
    /// A form for `world` (new if it isn't saved), showing `character` or its first one.
    pub fn new(world: Option<&World>, character: Option<&str>, saved: bool) -> Self {
        let w = world.cloned().unwrap_or_default();
        let c = character
            .and_then(|name| w.characters.iter().find(|c| c.name.eq_ignore_ascii_case(name)))
            .or_else(|| w.characters.first())
            .cloned()
            .unwrap_or_default();
        let port = if world.is_some() { w.port.to_string() } else { String::new() };
        let mut tls = Field::text("TLS", "Encrypted connection (Space to switch)", "");
        tls.kind = FieldKind::Toggle;
        tls.on = w.tls;
        let mut bouncer = Field::text("Bouncer", "Via the [bouncer] in config.toml (Space to switch)", "");
        bouncer.kind = FieldKind::Toggle;
        bouncer.on = w.bouncer;
        let mut login = Field::text("Login", "Sent after connecting, e.g. connect Bob secret", &c.login);
        login.kind = FieldKind::Secret;
        let fields = vec![
            Field::text("Name", "How it's listed (defaults to the host)", &w.name),
            Field::text("Host", "Server address; host:port works too", &w.host),
            Field::text("Port", "", &port),
            tls,
            bouncer,
            Field::text("Character", "Optional", &c.name),
            login,
        ];
        Self {
            fields,
            focus: NAME,
            editing: saved.then(|| w.name.clone()),
            editing_character: (saved && !c.name.is_empty()).then(|| c.name.clone()),
            error: None,
        }
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

    fn value(&self, field: usize) -> String {
        self.fields[field].input.text.trim().to_string()
    }

    /// Checks the fields and saves into `config`. Returns the world and character name to
    /// connect as.
    pub fn save(&mut self, config: &mut Config) -> Result<(World, Option<String>), String> {
        let mut host = self.value(HOST);
        let mut port_text = self.value(PORT);
        // "host:port" in the host field fills in the port.
        if let Some((h, p)) = host.rsplit_once(':')
            && p.parse::<u16>().is_ok()
        {
            if port_text.is_empty() {
                port_text = p.to_string();
            }
            host = h.to_string();
        }
        if host.is_empty() {
            self.focus = HOST;
            return Err("Enter the server's address".into());
        }
        let port: u16 = match port_text.parse() {
            Ok(p) if p > 0 => p,
            _ => {
                self.focus = PORT;
                return Err("The port should be a number from 1 to 65535".into());
            }
        };
        let name = match self.value(NAME) {
            n if n.is_empty() => host.clone(),
            n => n,
        };
        let clash = config.worlds.iter().any(|w| w.name.eq_ignore_ascii_case(&name) && self.editing.as_deref().is_none_or(|e| !e.eq_ignore_ascii_case(&w.name)));
        if clash {
            self.focus = NAME;
            return Err(format!("There's already a world called {name}"));
        }
        let index = match &self.editing {
            Some(old) => config.worlds.iter().position(|w| w.name.eq_ignore_ascii_case(old)),
            None => None,
        };
        let index = index.unwrap_or_else(|| {
            config.worlds.push(World::default());
            config.worlds.len() - 1
        });
        let world = &mut config.worlds[index];
        world.name = name;
        world.host = host;
        world.port = port;
        world.tls = self.fields[TLS].on;
        world.bouncer = self.fields[BOUNCER].on;

        let character = self.value(CHARACTER);
        let login = self.fields[LOGIN].input.text.clone();
        if !character.is_empty() {
            let existing = self.editing_character.as_deref().or(Some(character.as_str())).and_then(|old| world.characters.iter().position(|c| c.name.eq_ignore_ascii_case(old)));
            match existing {
                Some(i) => {
                    world.characters[i].name = character.clone();
                    world.characters[i].login = login;
                }
                None => world.characters.push(Character { name: character.clone(), login, ..Character::default() }),
            }
        }
        Ok((world.clone(), (!character.is_empty()).then_some(character)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_into(form: &mut WorldForm, field: usize, text: &str) {
        form.fields[field].input.set(text);
    }

    #[test]
    fn new_world_from_host_and_port() {
        let mut config = Config::default();
        let mut form = WorldForm::new(None, None, false);
        type_into(&mut form, HOST, "moo.example.org:7777");
        type_into(&mut form, CHARACTER, "Bob");
        type_into(&mut form, LOGIN, "connect Bob pw");
        let (world, character) = form.save(&mut config).unwrap();
        assert_eq!((world.name.as_str(), world.host.as_str(), world.port), ("moo.example.org", "moo.example.org", 7777));
        assert_eq!(character.as_deref(), Some("Bob"));
        assert_eq!(config.worlds[0].characters[0].login, "connect Bob pw");

        let mut again = WorldForm::new(None, None, false);
        type_into(&mut again, HOST, "moo.example.org:7777");
        assert!(again.save(&mut config).unwrap_err().contains("already a world"));
    }

    #[test]
    fn editing_renames_and_updates_the_character() {
        let mut config = Config::default();
        let mut form = WorldForm::new(None, None, false);
        type_into(&mut form, NAME, "Home");
        type_into(&mut form, HOST, "h.example.org");
        type_into(&mut form, PORT, "4000");
        type_into(&mut form, CHARACTER, "Bob");
        form.save(&mut config).unwrap();
        let mut edit = WorldForm::new(config.world("Home"), Some("Bob"), true);
        assert_eq!(edit.fields[PORT].input.text, "4000");
        type_into(&mut edit, NAME, "Home MOO");
        type_into(&mut edit, CHARACTER, "Robert");
        edit.fields[TLS].on = true;
        edit.fields[BOUNCER].on = true;
        edit.save(&mut config).unwrap();
        assert!(config.worlds[0].bouncer);
        assert_eq!(config.worlds.len(), 1);
        let w = &config.worlds[0];
        assert_eq!((w.name.as_str(), w.tls, w.characters.len(), w.characters[0].name.as_str()), ("Home MOO", true, 1, "Robert"));
        let mut bad = WorldForm::new(None, None, false);
        type_into(&mut bad, HOST, "x.example.org");
        type_into(&mut bad, PORT, "huge");
        assert!(bad.save(&mut config).is_err());
        assert_eq!(bad.focus, PORT, "the problem field gets focus");
    }
}
