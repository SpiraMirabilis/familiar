//! The few things Familiar asks of the desktop, done the right way for each system: open a link,
//! show a notification, play a sound, find an editor. Used by the terminal client for all of
//! them, and by the GTK client where GTK has no working backend of its own.

mod sound;

pub use sound::SoundPlayer;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else { return false };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// Finds a program on `PATH`. On Windows the extension may be left off (`code` finds `code.cmd`).
pub fn which(program: &str) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return is_executable_file(path).then(|| path.to_path_buf());
    }
    let paths = std::env::var_os("PATH")?;
    let extensions: Vec<String> = if cfg!(windows) {
        let list = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        std::iter::once(String::new()).chain(list.split(';').filter(|e| !e.is_empty()).map(str::to_string)).collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&paths).find_map(|dir| {
        extensions.iter().map(|ext| dir.join(format!("{program}{ext}"))).find(|candidate| is_executable_file(candidate))
    })
}

fn quiet(command: &mut Command) -> &mut Command {
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
}

/// Spawns a fire-and-forget child and reaps it on a background thread on Unix so it never
/// accumulates as a zombie process.
fn spawn_reaped(command: &mut Command) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut child = quiet(command).spawn()?;
        let _ = std::thread::Builder::new().name("familiar-reap".into()).spawn(move || {
            let _ = child.wait();
        });
    }
    #[cfg(not(unix))]
    {
        let _ = quiet(command).spawn()?;
    }
    Ok(())
}

/// True for URIs safe to pass to the desktop opener (`http://`, `https://`, `mailto:`).
pub fn is_safe_uri(uri: &str) -> bool {
    let bytes = uri.as_bytes();
    let ok = bytes.get(..7).is_some_and(|s| s.eq_ignore_ascii_case(b"http://"))
        || bytes.get(..8).is_some_and(|s| s.eq_ignore_ascii_case(b"https://"))
        || bytes.get(..7).is_some_and(|s| s.eq_ignore_ascii_case(b"mailto:"));
    ok && !uri.chars().any(char::is_control)
}

/// Opens an `http://`, `https://`, or `mailto:` link in the system default handler.
pub fn open_uri(uri: &str) -> Result<(), String> {
    if !is_safe_uri(uri) {
        return Err(format!("Refused to open unsafe URI: {uri}"));
    }
    let mut command = if cfg!(windows) {
        // Not `cmd /c start`: cmd would treat the `&` in a URL's query as a command separator.
        let mut c = Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", uri]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000);
        }
        c
    } else if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.args(["--", uri]);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(uri);
        c
    };
    spawn_reaped(&mut command).map_err(|e| format!("Couldn't open {uri}: {e}"))
}

/// Shows a desktop notification. Failures are ignored: there's nothing useful to do about them.
pub fn notify(app: &str, title: &str, body: &str) {
    #[cfg(windows)]
    {
        register_app_id(app);
        let show = |id: &str| tauri_winrt_notification::Toast::new(id).title(title).text1(body).show();
        // If ours is refused, use an id every Windows has rather than show nothing.
        if show(APP_USER_MODEL_ID).is_err() {
            let _ = show(tauri_winrt_notification::Toast::POWERSHELL_APP_ID);
        }
    }
    #[cfg(target_os = "macos")]
    {
        // The words are passed as arguments, never pasted into the script: they come from the server.
        let script = ["on run argv", "display notification (item 3 of argv) with title (item 1 of argv) subtitle (item 2 of argv)", "end run"];
        let mut command = Command::new("osascript");
        for line in script {
            command.args(["-e", line]);
        }
        let _ = spawn_reaped(command.args(["--", app, title, body]));
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let mut command = Command::new("notify-send");
        command.args(["-a", app, "--", title, body]);
        let _ = spawn_reaped(&mut command);
    }
}

/// Windows only shows toasts from an app id it knows. Registering it under the user's own
/// registry key (once per run) makes that true for the portable build as well as the installed
/// one, and gives the toast its "Familiar" heading.
#[cfg(windows)]
fn register_app_id(app: &str) {
    use std::os::windows::process::CommandExt;
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let key = format!(r"HKCU\Software\Classes\AppUserModelId\{APP_USER_MODEL_ID}");
        let _ = quiet(Command::new("reg").args(["add", &key, "/v", "DisplayName", "/t", "REG_SZ", "/d", app, "/f"]))
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    });
}

/// How Windows identifies the app for notifications and taskbar grouping.
pub const APP_USER_MODEL_ID: &str = "io.github.SpiraMirabilis.Familiar";

/// The editor to use when `$VISUAL` and `$EDITOR` aren't set.
pub fn default_editor() -> &'static str {
    if cfg!(windows) { "notepad" } else { "nano" }
}

/// Splits a command string into words, honouring single and double quotes so paths with spaces
/// (`"C:\Program Files\...\code.exe" --wait`) stay intact.
fn split_command(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut in_word = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) if c == q => quote = None,
            Some('"') if c == '\\' && matches!(chars.peek(), Some('"' | '\\')) => {
                cur.push(chars.next().unwrap());
            }
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    parts.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            None => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        parts.push(cur);
    }
    parts
}

/// The user's editor as a command to run, plus its name without folder or extension (`vim` for
/// `C:\tools\vim.exe`), for deciding which arguments it understands.
pub fn editor_command() -> (Command, String) {
    let env_nonempty = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
    let editor = env_nonempty("VISUAL").or_else(|| env_nonempty("EDITOR")).unwrap_or_default();
    let parts = split_command(&editor);
    let program = parts.first().map(String::as_str).unwrap_or(default_editor());
    let name = std::path::Path::new(program).file_stem().and_then(|n| n.to_str()).unwrap_or(program).to_ascii_lowercase();
    // Windows only runs .exe files directly; scripts like code.cmd need their full path.
    let mut command = Command::new(which(program).unwrap_or_else(|| PathBuf::from(program)));
    if parts.len() > 1 {
        command.args(&parts[1..]);
    }
    (command, name)
}

#[cfg(test)]
mod tests {
    #[test]
    fn which_finds_programs_on_path() {
        let known = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(super::which(known).is_some());
        assert!(super::which("no-such-program-familiar").is_none());
    }
}
