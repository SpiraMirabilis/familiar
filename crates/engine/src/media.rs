//! The `Client.Media` GMCP package (MCMP): the server names sounds, gives a base URL, and plays,
//! loops and stops them. See <https://wiki.mudlet.org/w/Standards:MUD_Client_Media_Protocol>.
//!
//! This module only tracks state and decides what to play. The UI downloads (with [`fetch`]) and
//! plays the files.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum MediaCommand {
    Play {
        name: String,
        url: String,
        /// 0.0 to 1.0.
        volume: f64,
        /// How many times to play. -1 means forever.
        loops: i32,
        /// If the same name is already playing: true restarts it, false leaves it playing.
        restart: bool,
    },
    /// Download ahead of time.
    Preload { url: String },
    /// Stop one sound, or all of them.
    Stop { name: Option<String> },
}

#[derive(Debug, Default)]
pub struct Media {
    default_url: Option<String>,
    /// URLs from Client.Media.Load, by name.
    loaded: HashMap<String, String>,
}

fn bool_value(v: &Value) -> Option<bool> {
    // BeipMU only understood the string "true"; accept real booleans too.
    match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) => Some(s.eq_ignore_ascii_case("true")),
        _ => None,
    }
}

impl Media {
    fn resolve(&self, name: &str, base: Option<&str>) -> Result<String, String> {
        if let Some(url) = self.loaded.get(name).filter(|_| base.is_none()) {
            return Ok(url.clone());
        }
        let base = base.or(self.default_url.as_deref()).ok_or_else(|| {
            format!("Client.Media: no URL for \"{name}\" (the server sent no url and no Client.Media.Default)")
        })?;
        Ok(format!("{base}{name}"))
    }

    /// Handles `Client.Media.<message>`. `message` is lowercase: default, load, play or stop.
    pub fn handle(&mut self, message: &str, json: &str) -> Result<Vec<MediaCommand>, String> {
        let v: Value = if json.trim().is_empty() { Value::Null } else { serde_json::from_str(json).map_err(|e| format!("Client.Media.{message}: {e}"))? };
        let s = |key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
        match message {
            "default" => {
                self.default_url = s("url");
                Ok(Vec::new())
            }
            "load" => {
                let name = s("name").ok_or("Client.Media.Load: missing name")?;
                let url = self.resolve(&name, s("url").as_deref())?;
                self.loaded.insert(name, url.clone());
                Ok(vec![MediaCommand::Preload { url }])
            }
            "play" => {
                let name = s("name").ok_or("Client.Media.Play: missing name")?;
                let url = self.resolve(&name, s("url").as_deref())?;
                let volume = v.get("volume").and_then(Value::as_f64).unwrap_or(50.0).clamp(0.0, 100.0) / 100.0;
                let loops = v.get("loops").and_then(Value::as_i64).unwrap_or(1).clamp(-1, i32::MAX as i64) as i32;
                let restart = !v.get("continue").and_then(bool_value).unwrap_or(true);
                Ok(vec![MediaCommand::Play { name, url, volume, loops: if loops == 0 { 1 } else { loops }, restart }])
            }
            "stop" => Ok(vec![MediaCommand::Stop { name: s("name") }]),
            other => Err(format!("Client.Media.{other} isn't supported")),
        }
    }
}

/// Where downloaded media is kept.
pub fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join(familiar_config::APP_DIR)
        .join("media")
}

/// FNV-1a, so cache names are stable across runs and Rust versions.
fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

/// Returns a local path for a media URL, downloading it into the cache if needed. Blocks, so
/// call it off the UI thread. `file://` URLs and plain paths are used as they are.
pub fn fetch(url: &str) -> Result<PathBuf, String> {
    if let Some(path) = url.strip_prefix("file://") {
        return Ok(PathBuf::from(path));
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Ok(PathBuf::from(url));
    }
    let ext = url
        .rsplit('/')
        .next()
        .and_then(|file| file.split(['?', '#']).next())
        .and_then(|file| file.rsplit_once('.'))
        .map(|(_, ext)| ext)
        .filter(|ext| ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or("bin");
    let dir = cache_dir();
    let path = dir.join(format!("{:016x}.{ext}", fnv1a(url)));
    if path.is_file() {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let agent = format!("{}/{} (MU* client; +https://github.com/SpiraMirabilis/familiar)", familiar_config::APP_NAME, env!("CARGO_PKG_VERSION"));
    let mut response = ureq::get(url).header("User-Agent", &agent).call().map_err(|e| format!("{url}: {e}"))?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(50 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| format!("{url}: {e}"))?;
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_url_and_play() {
        let mut m = Media::default();
        m.handle("default", r#"{"url":"https://x.org/sounds/"}"#).unwrap();
        let cmds = m.handle("play", r#"{"name":"rain.ogg","volume":80,"loops":-1,"continue":false}"#).unwrap();
        assert_eq!(
            cmds,
            vec![MediaCommand::Play { name: "rain.ogg".into(), url: "https://x.org/sounds/rain.ogg".into(), volume: 0.8, loops: -1, restart: true }]
        );
        // Defaults: volume 50, one loop, continue.
        let cmds = m.handle("play", r#"{"name":"a.wav"}"#).unwrap();
        assert!(matches!(&cmds[0], MediaCommand::Play { volume, loops: 1, restart: false, .. } if (*volume - 0.5).abs() < 1e-9));
    }

    #[test]
    fn explicit_url_and_load() {
        let mut m = Media::default();
        let cmds = m.handle("load", r#"{"name":"bell.wav","url":"https://y.org/"}"#).unwrap();
        assert_eq!(cmds, vec![MediaCommand::Preload { url: "https://y.org/bell.wav".into() }]);
        // A later play by name uses the loaded URL even without a default.
        let cmds = m.handle("play", r#"{"name":"bell.wav","continue":"true"}"#).unwrap();
        assert!(matches!(&cmds[0], MediaCommand::Play { url, restart: false, .. } if url == "https://y.org/bell.wav"));
    }

    #[test]
    fn missing_url_is_an_error_not_a_crash() {
        let mut m = Media::default();
        assert!(m.handle("play", r#"{"name":"x.wav"}"#).unwrap_err().contains("no URL"));
        assert!(m.handle("play", "{}").is_err());
        assert!(m.handle("play", "not json").is_err());
    }

    #[test]
    fn stop() {
        let mut m = Media::default();
        assert_eq!(m.handle("stop", r#"{"name":"a"}"#).unwrap(), vec![MediaCommand::Stop { name: Some("a".into()) }]);
        assert_eq!(m.handle("stop", "").unwrap(), vec![MediaCommand::Stop { name: None }]);
    }

    #[test]
    fn fetch_local_paths() {
        assert_eq!(fetch("file:///tmp/a.wav").unwrap(), PathBuf::from("/tmp/a.wav"));
        assert_eq!(fetch("/tmp/b.ogg").unwrap(), PathBuf::from("/tmp/b.ogg"));
    }
}
