//! Client configuration: worlds, characters and settings, stored as TOML.

mod address;
pub mod directory;
pub mod examples;
pub mod paths;
mod rules;

pub use address::Address;
pub use rules::{Alias, Highlight, Macro, Pattern, Rules, Scope, Trigger};

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Directory name under the XDG config and data dirs.
pub const APP_DIR: &str = "familiar";
/// The folder name used before the client was named, for [`migrate_old_dirs`].
const OLD_APP_DIR: &str = "mu-client";

/// The client's name, for the UI and for servers that ask (telnet TTYPE, MCP, GMCP).
pub const APP_NAME: &str = "Familiar";

/// Copies settings and data from the pre-release folder name (`mu-client`) the first time the
/// new folders are missing. Copies rather than moves, so an older build still finds its files.
/// Returns what was copied, to tell the user.
pub fn migrate_old_dirs() -> Vec<String> {
    let mut done = Vec::new();
    let pairs = [(dirs::config_dir(), config_dir()), (dirs::data_local_dir(), data_dir())];
    for (base, new) in pairs {
        let Some(old) = base.map(|b| b.join(OLD_APP_DIR)) else { continue };
        if old.is_dir() && !new.exists() && copy_dir(&old, &new).is_ok() {
            done.push(format!("Copied {} to {}", old.display(), new.display()));
        }
    }
    done
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path}: {source}")]
    Parse { path: PathBuf, source: toml::de::Error },
    #[error(transparent)]
    Serialize(#[from] toml::ser::Error),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub settings: Settings,
    /// An mbnc bouncer that worlds with `bouncer = true` connect through.
    #[serde(skip_serializing_if = "Bouncer::is_unset")]
    pub bouncer: Bouncer,
    /// Rules that apply to every world.
    #[serde(skip_serializing_if = "Rules::is_empty")]
    pub rules: Rules,
    // Leave the key out when empty: `world = []` would clash with [[world]] tables added by
    // hand later.
    #[serde(rename = "world", skip_serializing_if = "Vec::is_empty")]
    pub worlds: Vec<World>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The default font, as a Pango font description, e.g. "Monospace 11". Worlds can have
    /// their own.
    pub font: String,
    pub scrollback_lines: usize,
    /// Show sent commands in the output window.
    pub local_echo: bool,
    pub input_history_size: usize,
    /// Prefix each line in log files with the time.
    pub log_timestamps: bool,
    /// Milliseconds of silence before an unterminated line is shown as a prompt.
    pub prompt_timeout_ms: u64,
    /// Show a thumbnail under lines that link to images. Off by default: fetching an image
    /// someone posted tells that site your IP address.
    pub inline_images: bool,
    /// Speak every line that arrives (for screen-reader and text-to-speech users).
    pub speak_output: bool,
    /// Underline misspellings in the input.
    pub spellcheck: bool,
    /// Terminal client: draw the 16 basic colours with the terminal's own theme instead of the
    /// standard ones. Themes often change them a lot (a pastel "red"), which misrepresents
    /// what a server meant, so this is off by default.
    pub terminal_colors: bool,
    /// Terminal client: run `$EDITOR` in a tab of its own, so the worlds stay in view. When
    /// off, the editor takes over the whole terminal until it exits.
    pub editor_in_tab: bool,
    /// Terminal client: when `$EDITOR` would open in a tab, put it beside the world it came
    /// from instead, where the sidebar goes, if that world has nothing in its sidebar.
    pub editor_in_sidebar: bool,
    /// Terminal client: the hint that `/editor syntax` installs MOO highlighting for `$EDITOR`
    /// has been shown. It's shown once, after the first MOO edit in an editor without it.
    pub editor_syntax_hint_shown: bool,
    /// Terminal client: LLM agents `/agent` can run beside a world, besides the built-in ones
    /// ([`Agent::builtin`]). One with a built-in one's name replaces it.
    #[serde(rename = "agent", skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<Agent>,
    /// Terminal client: the agents `/agent` tries, in order, when it isn't given a name. It runs
    /// the first one whose program is installed.
    #[serde(skip_serializing_if = "is_default_agent_order")]
    pub agent_order: Vec<String>,
}

/// The agents `/agent` tries when the config doesn't say.
pub const DEFAULT_AGENT_ORDER: [&str; 4] = ["claude", "codex", "agy", "grok"];

fn default_agent_order() -> Vec<String> {
    DEFAULT_AGENT_ORDER.iter().map(|s| s.to_string()).collect()
}

fn is_default_agent_order(order: &Vec<String>) -> bool {
    *order == default_agent_order()
}

/// An LLM agent (Claude Code, Codex, ...) the terminal client can run beside a world, with the
/// world's tools served to it over MCP (Model Context Protocol).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    pub name: String,
    /// The program and its arguments. `{url}` in them is replaced by the address of the MCP
    /// server for the world, and `{world}` by the world's name. The program also gets the
    /// address in the environment, as FAMILIAR_MCP_URL.
    pub command: Vec<String>,
    /// Files written into the agent's folder before it starts, by path within the folder, with
    /// `{url}` and `{world}` filled in as in `command`. This is for agents that read their MCP
    /// servers from a file in the project rather than from their command line.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub files: std::collections::BTreeMap<String, String>,
}

impl Agent {
    /// The agents there are without any in the config.
    pub fn builtin() -> Vec<Agent> {
        let agent = |name: &str, command: &[&str], files: &[(&str, &str)]| Agent {
            name: name.into(),
            command: command.iter().map(|s| s.to_string()).collect(),
            files: files.iter().map(|(path, text)| (path.to_string(), text.to_string())).collect(),
        };
        vec![
            agent("claude", &["claude", "--mcp-config", r#"{"mcpServers":{"familiar":{"type":"http","url":"{url}"}}}"#], &[]),
            agent("codex", &["codex", "-c", r#"mcp_servers.familiar.url="{url}""#], &[]),
            // Antigravity's CLI reads a workspace's servers from .agents/mcp_config.json.
            agent("agy", &["agy"], &[(".agents/mcp_config.json", r#"{"mcpServers":{"familiar":{"serverUrl":"{url}"}}}"#)]),
            // Grok Build reads Claude Code's project file, .mcp.json.
            agent("grok", &["grok"], &[(".mcp.json", r#"{"mcpServers":{"familiar":{"type":"http","url":"{url}"}}}"#)]),
        ]
    }

    fn fill(&self, text: &str, url: &str, world: &str) -> String {
        text.replace("{url}", url).replace("{world}", world)
    }

    /// The command with `{url}` and `{world}` filled in.
    pub fn command_for(&self, url: &str, world: &str) -> Vec<String> {
        self.command.iter().map(|arg| self.fill(arg, url, world)).collect()
    }

    /// The files to write into the agent's folder, with `{url}` and `{world}` filled in.
    pub fn files_for(&self, url: &str, world: &str) -> Vec<(String, String)> {
        self.files.iter().map(|(path, text)| (path.clone(), self.fill(text, url, world))).collect()
    }
}

impl Settings {
    /// The agents `/agent` can run: the built-in ones, replaced or joined by the configured
    /// ones.
    pub fn agents(&self) -> Vec<Agent> {
        let mut agents = Agent::builtin();
        for agent in &self.agents {
            match agents.iter_mut().find(|a| a.name.eq_ignore_ascii_case(&agent.name)) {
                Some(builtin) => *builtin = agent.clone(),
                None => agents.push(agent.clone()),
            }
        }
        agents
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            // Pango's "Monospace" is Courier New on Windows, which reads poorly at this size.
            font: if cfg!(windows) {
                "Consolas 11"
            } else if cfg!(target_os = "macos") {
                "Menlo 12"
            } else {
                "Monospace 11"
            }
            .into(),
            scrollback_lines: 10_000,
            local_echo: true,
            input_history_size: 500,
            log_timestamps: true,
            prompt_timeout_ms: 250,
            inline_images: false,
            speak_output: false,
            spellcheck: true,
            terminal_colors: false,
            editor_in_tab: true,
            editor_in_sidebar: false,
            editor_syntax_hint_shown: false,
            agents: Vec::new(),
            agent_order: default_agent_order(),
        }
    }
}

/// An mbnc bouncer (a BNC for M*s). It holds the
/// connection to the world while the client is away and replays what was missed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bouncer {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub verify_certificate: bool,
    /// Stored in plain text, like character logins.
    pub password: String,
}

impl Default for Bouncer {
    fn default() -> Self {
        Self { host: String::new(), port: 7777, tls: true, verify_certificate: true, password: String::new() }
    }
}

impl Bouncer {
    pub fn is_unset(&self) -> bool {
        self.host.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct World {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub verify_certificate: bool,
    /// Charset name understood by `familiar_net::Encoding::from_name`. "Auto" reads UTF-8 and falls
    /// back to Windows-1252 line by line.
    pub encoding: String,
    /// Speak MCP 2.1 if the server offers it (MOOs and MUCKs).
    pub mcp: bool,
    /// The language programs from this world are written in, for the code editor:
    /// "moo", "muf", "lua" or any GtkSourceView language id. Empty means MOO.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub program_language: String,
    /// A JavaScript file to run for this world, after the global startup.js.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub script: String,
    /// This world's font, as a Pango font description. Empty means the default font from the
    /// settings.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub font: String,
    /// Whether the server may open web views (GMCP webview.open). None means ask.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_webviews: Option<bool>,
    /// Connect through the bouncer in the config's `[bouncer]` section instead of directly.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub bouncer: bool,
    #[serde(skip_serializing_if = "Rules::is_empty")]
    pub rules: Rules,
    #[serde(rename = "character", skip_serializing_if = "Vec::is_empty")]
    pub characters: Vec<Character>,
}

impl Default for World {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: String::new(),
            port: 4000,
            tls: false,
            verify_certificate: true,
            encoding: "Auto".into(),
            mcp: true,
            program_language: String::new(),
            script: String::new(),
            font: String::new(),
            allow_webviews: None,
            bouncer: false,
            rules: Rules::default(),
            characters: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Character {
    pub name: String,
    /// Sent right after connecting, e.g. "connect bob secret". Stored in plain text.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub login: String,
    #[serde(skip_serializing_if = "Rules::is_empty")]
    pub rules: Rules,
    #[serde(rename = "puppet", skip_serializing_if = "Vec::is_empty")]
    pub puppets: Vec<Puppet>,
}

/// A second character controlled through this one's connection (e.g. a MUCK zombie or a MUSH
/// puppet), with its own tab. As in BeipMU.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Puppet {
    pub name: String,
    /// Lines starting with this (or matching it, as a regex) belong to the puppet.
    pub receive_prefix: String,
    pub regex: bool,
    /// Remove the prefix from lines shown in the puppet's tab.
    pub hide_prefix: bool,
    /// Put in front of everything typed in the puppet's tab, e.g. "z1 " or "@force puppet=".
    pub send_prefix: String,
    /// Open the puppet's tab when one of its lines arrives.
    pub auto_open: bool,
    /// Open the puppet's tab when the character connects.
    pub open_on_connect: bool,
    /// If something typed already starts with the send prefix, don't add it twice.
    pub remove_accidental_prefix: bool,
}

impl Default for Puppet {
    fn default() -> Self {
        Self {
            name: String::new(),
            receive_prefix: String::new(),
            regex: false,
            hide_prefix: true,
            send_prefix: String::new(),
            auto_open: true,
            open_on_connect: false,
            remove_accidental_prefix: false,
        }
    }
}

pub use paths::{config_dir, data_dir};

pub fn default_config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// Runs for every session, if it exists.
pub fn startup_script_path() -> PathBuf {
    config_dir().join("startup.js")
}

/// Where a world's map is saved.
pub fn map_path(world: &str) -> PathBuf {
    data_dir().join("maps").join(format!("{}.json", paths::file_name(world, "map")))
}

/// Where an agent working on a world runs, so it can keep notes about the world there.
pub fn agent_dir(world: &str) -> PathBuf {
    data_dir().join("agents").join(paths::file_name(world, "world"))
}

pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

impl Config {
    pub fn world(&self, name: &str) -> Option<&World> {
        self.worlds.iter().find(|w| w.name == name)
    }

    /// The rules stored at `scope`, if that world or character exists.
    pub fn rules(&self, scope: &Scope) -> Option<&Rules> {
        match scope {
            Scope::Global => Some(&self.rules),
            Scope::World { world } => self.world(world).map(|w| &w.rules),
            Scope::Character { world, character } => self
                .world(world)
                .and_then(|w| w.characters.iter().find(|c| &c.name == character))
                .map(|c| &c.rules),
        }
    }

    pub fn rules_mut(&mut self, scope: &Scope) -> Option<&mut Rules> {
        match scope {
            Scope::Global => Some(&mut self.rules),
            Scope::World { world } => self.worlds.iter_mut().find(|w| &w.name == world).map(|w| &mut w.rules),
            Scope::Character { world, character } => self
                .worlds
                .iter_mut()
                .find(|w| &w.name == world)
                .and_then(|w| w.characters.iter_mut().find(|c| &c.name == character))
                .map(|c| &mut c.rules),
        }
    }

    /// The scopes that apply to a session, most specific first. A world or character that isn't
    /// saved in the config (e.g. a quick connect) has no scope of its own.
    pub fn scopes_for(&self, world: &str, character: Option<&str>) -> Vec<Scope> {
        let mut scopes = Vec::new();
        if let Some(character) = character {
            let scope = Scope::Character { world: world.into(), character: character.into() };
            if self.rules(&scope).is_some() {
                scopes.push(scope);
            }
        }
        if self.world(world).is_some() {
            scopes.push(Scope::World { world: world.into() });
        }
        scopes.push(Scope::Global);
        scopes
    }

    /// Loads the config, or returns the default if the file doesn't exist yet.
    pub fn load(path: &Path) -> Result<Self, Error> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|source| Error::Parse { path: path.into(), source }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(Error::Io { path: path.into(), source }),
        }
    }

    /// Saves by writing a temporary file and renaming it over the old one, so a crash can't leave
    /// a half-written config.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        let text = toml::to_string_pretty(self)?;
        let io_err = |source| Error::Io { path: path.into(), source };
        paths::write_atomic(path, text.as_bytes()).map_err(io_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let config = Config {
            settings: Settings { font: "Fira Mono 12".into(), ..Settings::default() },
            bouncer: Bouncer { host: "bnc.example".into(), password: "pw".into(), ..Bouncer::default() },
            rules: Rules::default(),
            worlds: vec![World {
                name: "Test".into(),
                host: "example.com".into(),
                port: 7777,
                tls: true,
                bouncer: true,
                characters: vec![Character {
                    name: "Bob".into(),
                    login: "connect bob pw".into(),
                    rules: Rules {
                        aliases: vec![Alias { replacement: "kill $1".into(), ..Alias::default() }],
                        ..Rules::default()
                    },
                    puppets: vec![Puppet { name: "Zed".into(), receive_prefix: "Zed>".into(), send_prefix: "z ".into(), ..Puppet::default() }],
                }],
                ..World::default()
            }],
        };
        let text = toml::to_string_pretty(&config).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), config);
    }

    #[test]
    fn scopes() {
        let mut config = Config::default();
        config.worlds.push(World {
            name: "W".into(),
            characters: vec![Character { name: "C".into(), ..Character::default() }],
            ..World::default()
        });
        let char_scope = Scope::Character { world: "W".into(), character: "C".into() };
        assert_eq!(
            config.scopes_for("W", Some("C")),
            vec![char_scope.clone(), Scope::World { world: "W".into() }, Scope::Global]
        );
        assert_eq!(config.scopes_for("adhoc", Some("C")), vec![Scope::Global]);
        config.rules_mut(&char_scope).unwrap().variables.insert("x".into(), "1".into());
        assert_eq!(config.worlds[0].characters[0].rules.variables["x"], "1");
    }

    #[test]
    fn worlds_can_be_added_by_hand_after_saving() {
        let mut text = toml::to_string_pretty(&Config::default()).unwrap();
        assert!(!text.contains("world"), "{text}");
        text.push_str("\n[[world]]\nname = \"Hand\"\nhost = \"h\"\n");
        assert_eq!(toml::from_str::<Config>(&text).unwrap().worlds[0].name, "Hand");
    }

    #[test]
    fn missing_fields_use_defaults() {
        let config: Config = toml::from_str("[[world]]\nname = \"A\"\nhost = \"a.org\"\n").unwrap();
        assert_eq!(config.settings, Settings::default());
        assert_eq!(config.worlds[0].port, 4000);
        assert!(config.worlds[0].verify_certificate);
    }

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join(format!("mu-config-test-{}", std::process::id()));
        let path = dir.join("config.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        let config = Config { worlds: vec![World { name: "X".into(), ..World::default() }], ..Config::default() };
        config.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), config);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
