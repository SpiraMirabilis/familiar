//! Client configuration: worlds, characters and settings, stored as TOML.

pub mod directory;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Directory name under the XDG config and data dirs.
pub const APP_DIR: &str = "mu-client";

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
    #[serde(rename = "world")]
    pub worlds: Vec<World>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Pango font description, e.g. "Monospace 11".
    pub font: String,
    pub scrollback_lines: usize,
    /// Show sent commands in the output window.
    pub local_echo: bool,
    pub input_history_size: usize,
    /// Prefix each line in log files with the time.
    pub log_timestamps: bool,
    /// Milliseconds of silence before an unterminated line is shown as a prompt.
    pub prompt_timeout_ms: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font: "Monospace 11".into(),
            scrollback_lines: 10_000,
            local_echo: true,
            input_history_size: 500,
            log_timestamps: true,
            prompt_timeout_ms: 250,
        }
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
    /// Charset name understood by `mu_net::Encoding::from_name`.
    pub encoding: String,
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
            encoding: "UTF-8".into(),
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
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join(APP_DIR)
}

pub fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join(APP_DIR)
}

pub fn default_config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

impl Config {
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
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io_err)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).map_err(io_err)?;
        std::fs::rename(&tmp, path).map_err(io_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let config = Config {
            settings: Settings { font: "Fira Mono 12".into(), ..Settings::default() },
            worlds: vec![World {
                name: "Test".into(),
                host: "example.com".into(),
                port: 7777,
                tls: true,
                characters: vec![Character { name: "Bob".into(), login: "connect bob pw".into() }],
                ..World::default()
            }],
        };
        let text = toml::to_string_pretty(&config).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap(), config);
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
