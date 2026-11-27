//! The `Client.Media` GMCP package (MCMP): the server names sounds, gives a base URL, and plays,
//! loops and stops them. See <https://wiki.mudlet.org/w/Standards:MUD_Client_Media_Protocol>.
//!
//! This module only tracks state and decides what to play. The UI downloads (with [`fetch`]) and
//! plays the files.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

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

const MAX_LOADED: usize = 2048;
const MAX_LOOPS: i64 = 100;
const MAX_CACHE_BYTES: u64 = 256 * 1024 * 1024;

/// True for `http://` and `https://` URLs (case-insensitive) without control characters.
pub fn is_http_url(url: &str) -> bool {
    let bytes = url.as_bytes();
    let ok = bytes.get(..7).is_some_and(|s| s.eq_ignore_ascii_case(b"http://"))
        || bytes.get(..8).is_some_and(|s| s.eq_ignore_ascii_case(b"https://"));
    ok && !url.chars().any(char::is_control)
}

/// True for URIs safe to hand to the desktop launcher (`http://`, `https://`, `mailto:`).
pub fn is_safe_open_uri(uri: &str) -> bool {
    let bytes = uri.as_bytes();
    let ok = is_http_url(uri) || bytes.get(..7).is_some_and(|s| s.eq_ignore_ascii_case(b"mailto:"));
    ok && !uri.chars().any(char::is_control)
}

impl Media {
    fn resolve(&self, name: &str, base: Option<&str>) -> Result<String, String> {
        if let Some(url) = self.loaded.get(name).filter(|_| base.is_none()) {
            return Ok(url.clone());
        }
        let base = base.or(self.default_url.as_deref()).ok_or_else(|| {
            format!("Client.Media: no URL for \"{name}\" (the server sent no url and no Client.Media.Default)")
        })?;
        let url = format!("{base}{name}");
        if !is_http_url(&url) {
            return Err("Client.Media: only http:// and https:// URLs are allowed".into());
        }
        Ok(url)
    }

    /// Handles `Client.Media.<message>`. `message` is lowercase: default, load, play or stop.
    pub fn handle(&mut self, message: &str, json: &str) -> Result<Vec<MediaCommand>, String> {
        let v: Value = if json.trim().is_empty() { Value::Null } else { serde_json::from_str(json).map_err(|e| format!("Client.Media.{message}: {e}"))? };
        let s = |key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
        match message {
            "default" => {
                let url = s("url");
                if let Some(ref u) = url
                    && !is_http_url(u)
                {
                    return Err("Client.Media.Default: url must be http:// or https://".into());
                }
                self.default_url = url;
                Ok(Vec::new())
            }
            "load" => {
                let name = s("name").ok_or("Client.Media.Load: missing name")?;
                let url = self.resolve(&name, s("url").as_deref())?;
                if self.loaded.contains_key(&name) || self.loaded.len() < MAX_LOADED {
                    self.loaded.insert(name, url.clone());
                }
                Ok(vec![MediaCommand::Preload { url }])
            }
            "play" => {
                let name = s("name").ok_or("Client.Media.Play: missing name")?;
                let url = self.resolve(&name, s("url").as_deref())?;
                let volume = v.get("volume").and_then(Value::as_f64).filter(|x| x.is_finite()).unwrap_or(50.0).clamp(0.0, 100.0) / 100.0;
                let loops = v.get("loops").and_then(Value::as_i64).unwrap_or(1).clamp(-1, MAX_LOOPS) as i32;
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
    familiar_config::paths::cache_dir().join("media")
}

/// FNV-1a (128-bit), so cache names are stable across runs and collision-resistant.
fn fnv1a_128(s: &str) -> u128 {
    s.bytes().fold(0x6c62272e07bb014262b821756295c58du128, |h, b| {
        (h ^ b as u128).wrapping_mul(0x0000000001000000000000000000013bu128)
    })
}

fn prune_cache(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = entries
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            Some((e.path(), meta.len(), modified))
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, len, _)| *len).sum();
    if total <= MAX_CACHE_BYTES {
        return;
    }
    files.sort_by_key(|(_, _, modified)| *modified);
    for (path, len, _) in files {
        if total <= MAX_CACHE_BYTES {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
}

/// Returns a local path for a remote media URL (`http://` or `https://`), downloading it into
/// the cache if needed. Blocks, so call it off the UI thread.
pub fn fetch(url: &str) -> Result<PathBuf, String> {
    if !is_http_url(url) {
        return Err(format!("{url}: only http:// and https:// URLs are allowed"));
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
    let path = dir.join(format!("{:032x}.{ext}", fnv1a_128(url)));
    if path.is_file() {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let agent = format!("{}/{} (MU* client; +https://github.com/SpiraMirabilis/familiar)", familiar_config::APP_NAME, env!("CARGO_PKG_VERSION"));
    // Without these a server that stops answering would hold a worker thread forever.
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(120)))
        .max_redirects(5)
        .build();
    let mut response =
        ureq::Agent::new_with_config(config).get(url).header("User-Agent", &agent).call().map_err(|e| format!("{url}: {e}"))?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(50 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| format!("{url}: {e}"))?;
    familiar_config::paths::write_atomic(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    prune_cache(&dir);
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
    fn fetch_rejects_local_paths() {
        assert!(fetch("file:///tmp/a%20b.wav").is_err());
        assert!(fetch("/tmp/b.ogg").is_err());
        let mut m = Media::default();
        assert!(m.handle("default", r#"{"url":"file:///etc/"}"#).is_err());
        assert!(m.handle("play", r#"{"name":"passwd","url":"file:///etc/"}"#).is_err());
    }
}
