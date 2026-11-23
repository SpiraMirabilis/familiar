//! The few things Familiar asks of the desktop, done the right way for each system: open a link,
//! show a notification, play a sound, find an editor. Used by the terminal client for all of
//! them, and by the GTK client where GTK has no working backend of its own.

mod sound;

pub use sound::SoundPlayer;

use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Finds a program on `PATH`. On Windows the extension may be left off (`code` finds `code.cmd`).
pub fn which(program: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    let extensions: Vec<String> = if cfg!(windows) {
        let list = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        std::iter::once(String::new()).chain(list.split(';').filter(|e| !e.is_empty()).map(str::to_string)).collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&paths).find_map(|dir| {
        extensions.iter().map(|ext| dir.join(format!("{program}{ext}"))).find(|candidate| candidate.is_file())
    })
}

fn quiet(command: &mut Command) -> &mut Command {
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
}

/// Opens a link or file in whatever the desktop uses for it.
pub fn open_uri(uri: &str) -> Result<(), String> {
    let mut command = if cfg!(windows) {
        // Not `cmd /c start`: cmd would treat the `&` in a URL's query as a command separator.
        let mut c = Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", uri]);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(uri);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(uri);
        c
    };
    quiet(&mut command).spawn().map(|_| ()).map_err(|e| format!("Couldn't open {uri}: {e}"))
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
        let _ = quiet(command.args(["--", app, title, body])).spawn();
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = quiet(Command::new("notify-send").args(["-a", app, title, body])).spawn();
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

/// The user's editor as a command to run, plus its name without folder or extension (`vim` for
/// `C:\tools\vim.exe`), for deciding which arguments it understands.
pub fn editor_command() -> (Command, String) {
    let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).unwrap_or_default();
    // The variable may hold arguments ("code --wait").
    let mut parts = editor.split_whitespace();
    let program = parts.next().unwrap_or(default_editor());
    let name = std::path::Path::new(program).file_stem().and_then(|n| n.to_str()).unwrap_or(program).to_ascii_lowercase();
    // Windows only runs .exe files directly; scripts like code.cmd need their full path.
    let mut command = Command::new(which(program).unwrap_or_else(|| PathBuf::from(program)));
    command.args(parts);
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
