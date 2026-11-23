//! What both ways of running `$EDITOR` (in a tab, or with the whole terminal) share: the file
//! being edited and the command to edit it with.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A file open in an editor.
pub struct Document {
    pub path: PathBuf,
    /// It was made for this edit, and goes away afterwards.
    temporary: bool,
    /// A final newline was added for the editor's sake; it's taken off again afterwards.
    added_newline: bool,
}

impl Document {
    /// `content` in a temporary file named after `name`.
    pub fn temporary(name: &str, extension: &str, content: &str) -> Result<Self, String> {
        let dir = std::env::temp_dir().join(format!("familiar-tui-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let short: String = name.chars().take(40).collect();
        let path = dir.join(format!("{}.{extension}", familiar_config::paths::file_name(&short, "edit")));
        let added_newline = !content.is_empty() && !content.ends_with('\n');
        std::fs::write(&path, if added_newline { format!("{content}\n") } else { content.to_string() }).map_err(|e| e.to_string())?;
        Ok(Self { path, temporary: true, added_newline })
    }

    /// A file that's edited where it is.
    pub fn file(path: &Path) -> Self {
        Self { path: path.to_path_buf(), temporary: false, added_newline: false }
    }

    /// The text after editing.
    pub fn finish(self) -> Result<String, String> {
        let text = std::fs::read_to_string(&self.path).map_err(|e| e.to_string());
        let added = self.added_newline;
        self.discard();
        text.map(|text| if added { text.strip_suffix('\n').map(str::to_string).unwrap_or(text) } else { text })
    }

    pub fn discard(self) {
        if self.temporary {
            let _ = std::fs::remove_file(&self.path);
            // And the folder made for these files, once it's empty.
            if let Some(dir) = self.path.parent() {
                let _ = std::fs::remove_dir(dir);
            }
        }
    }
}

/// `$VISUAL` or `$EDITOR` (nano if neither is set; Notepad on Windows) on `path`, at `line` if
/// the editor understands `+N`. Also returns the editor's name.
pub fn command(path: &Path, line: Option<u32>) -> (Command, String) {
    let (mut cmd, name) = familiar_platform::editor_command();
    if let Some(line) = line
        && ["vi", "vim", "nvim", "nano", "emacs", "micro", "kak", "joe", "mg"].contains(&name.as_str())
    {
        cmd.arg(format!("+{line}"));
    }
    cmd.arg(path);
    (cmd, name)
}
