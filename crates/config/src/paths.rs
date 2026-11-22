//! Where things live on disk, and turning what users type (`~/x`, `file://` URLs) into paths.
//! Everything goes through `dirs`, so it follows XDG on Linux and the known folders on Windows.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::APP_DIR;

pub fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Settings: `~/.config/familiar`, or `%APPDATA%\familiar` on Windows.
pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join(APP_DIR)
}

/// Maps and logs: `~/.local/share/familiar`, or `%LOCALAPPDATA%\familiar` on Windows (local, so
/// big log folders don't roam with the profile).
pub fn data_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")).join(APP_DIR)
}

/// Things that can be thrown away: `~/.cache/familiar`, or `%LOCALAPPDATA%\familiar\cache`.
pub fn cache_dir() -> PathBuf {
    match dirs::cache_dir() {
        // Windows has no separate cache folder, so keep ours apart from the data.
        Some(dir) if cfg!(windows) => dir.join(APP_DIR).join("cache"),
        Some(dir) => dir.join(APP_DIR),
        None => std::env::temp_dir().join(APP_DIR),
    }
}

/// A path as the user wrote it: `~` is the home folder and `file://` URLs are understood.
pub fn expand(text: &str) -> PathBuf {
    let text = text.trim();
    if let Some(path) = file_url_path(text, cfg!(windows)) {
        return PathBuf::from(path);
    }
    let rest = match text.strip_prefix('~') {
        Some("") => Some(""),
        Some(rest) => rest.strip_prefix('/').or_else(|| if cfg!(windows) { rest.strip_prefix('\\') } else { None }),
        None => None,
    };
    match (rest, home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(text),
    }
}

/// A path for showing to the user, with the home folder shortened to `~`.
pub fn display(path: &Path) -> String {
    match home_dir().and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        None => path.display().to_string(),
    }
}

/// The local path a `file://` URL names, or None if it isn't one. `windows` selects drive
/// letter handling (`file:///C:/x` is `C:/x`, not `/C:/x`), so both forms can be tested anywhere.
pub fn file_url_path(url: &str, windows: bool) -> Option<String> {
    let scheme = url.get(..7)?;
    if !scheme.eq_ignore_ascii_case("file://") {
        return None;
    }
    let rest = &url[7..];
    // file://localhost/x is the same as file:///x.
    let rest = match rest.strip_prefix("localhost") {
        Some(after) if after.starts_with('/') => after,
        _ => rest,
    };
    let path = percent_decode(rest);
    if windows {
        let bytes = path.as_bytes();
        if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
            return Some(path[1..].to_string());
        }
        // file://server/share/x is a UNC path.
        if !path.starts_with('/') && !path.is_empty() {
            return Some(format!("//{path}"));
        }
    }
    Some(path)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(hi << 4 | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A name that's safe as a file name everywhere: awkward characters become `_`, and names
/// Windows reserves for devices (`CON`, `NUL`, `COM1`…) get a `_` in front. `fallback` is used
/// for an empty name.
pub fn file_name(name: &str, fallback: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' }).collect();
    // Windows drops trailing dots, and reserves device names whatever the extension.
    let s = s.trim_end_matches('.').to_string();
    if s.is_empty() {
        return fallback.into();
    }
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    let numbered = |prefix: &str| stem.strip_prefix(prefix).is_some_and(|n| n.len() == 1 && n != "0" && n.chars().all(|c| c.is_ascii_digit()));
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered("COM") || numbered("LPT") { format!("_{s}") } else { s }
}

/// Writes a file by writing a temporary one beside it and renaming it into place, so a crash
/// can't leave it half-written and two writers can't interleave.
pub fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}-{}.tmp", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
    let tmp = path.with_file_name(name);
    std::fs::write(&tmp, contents)?;
    let mut tries = 0;
    loop {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            // On Windows the rename fails while something else (a virus scanner, an editor)
            // has the old file open. That passes quickly.
            Err(e) if cfg!(windows) && e.kind() == std::io::ErrorKind::PermissionDenied && tries < 5 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(40 * tries));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls() {
        assert_eq!(file_url_path("file:///tmp/a%20b.wav", false).as_deref(), Some("/tmp/a b.wav"));
        assert_eq!(file_url_path("file://localhost/tmp/x", false).as_deref(), Some("/tmp/x"));
        assert_eq!(file_url_path("file:///C:/Sounds/x.wav", true).as_deref(), Some("C:/Sounds/x.wav"));
        assert_eq!(file_url_path("FILE:///c:/x", true).as_deref(), Some("c:/x"));
        assert_eq!(file_url_path("file://server/share/x.wav", true).as_deref(), Some("//server/share/x.wav"));
        assert_eq!(file_url_path("https://x.org/a", false), None);
        assert_eq!(file_url_path("C:/x", true), None);
        // A stray percent sign is left alone.
        assert_eq!(file_url_path("file:///a%zz%4", false).as_deref(), Some("/a%zz%4"));
    }

    #[test]
    fn tilde_is_home() {
        let home = home_dir().expect("a home directory");
        assert_eq!(expand("~/x/y.js"), home.join("x/y.js"));
        assert_eq!(expand("~"), home);
        assert_eq!(expand(" /plain/path "), PathBuf::from("/plain/path"));
        // Not a home reference: another user's home, or a name that merely starts with ~.
        assert_eq!(expand("~bob/x"), PathBuf::from("~bob/x"));
    }

    #[test]
    fn safe_file_names() {
        assert_eq!(file_name("My World!", "map"), "My_World_");
        assert_eq!(file_name("", "map"), "map");
        assert_eq!(file_name("CON", "map"), "_CON");
        assert_eq!(file_name("nul.txt", "map"), "_nul.txt");
        assert_eq!(file_name("com1", "map"), "_com1");
        assert_eq!(file_name("Console", "map"), "Console");
        assert_eq!(file_name("COM10", "map"), "COM10");
        assert_eq!(file_name("dots...", "map"), "dots");
    }

    #[test]
    fn atomic_write_replaces() {
        let dir = std::env::temp_dir().join(format!("familiar-paths-test-{}", std::process::id()));
        let path = dir.join("sub").join("f.txt");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
        // No temporary files left behind.
        assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
