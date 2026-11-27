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
    /// Sanitizes an untrusted extension from a server (e.g. MCP `type`) so it cannot traverse
    /// directories or inject unexpected suffixes.
    fn safe_extension(ext: &str) -> String {
        let clean: String = ext.chars().filter(|c| c.is_ascii_alphanumeric()).take(16).collect();
        if clean.is_empty() { "txt".into() } else { clean }
    }

    fn temp_dir() -> Result<PathBuf, String> {
        let dir = std::env::temp_dir().join(format!("familiar-tui-{}", std::process::id()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true).mode(0o700);
            builder.create(&dir).map_err(|e| e.to_string())?;
        }
        #[cfg(not(unix))]
        {
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        }
        Ok(dir)
    }

    /// `content` in a temporary file named after `name`.
    pub fn temporary(name: &str, extension: &str, content: &str) -> Result<Self, String> {
        use std::io::Write;
        let dir = Self::temp_dir()?;
        let short: String = name.chars().take(40).collect();
        let ext = Self::safe_extension(extension);
        let path = dir.join(format!("{}.{ext}", familiar_config::paths::file_name(&short, "edit")));
        let added_newline = !content.is_empty() && !content.ends_with('\n');
        let body = if added_newline { format!("{content}\n") } else { content.to_string() };
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&path).map_err(|e| e.to_string())?;
        file.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
        Ok(Self { path, temporary: true, added_newline })
    }

    /// A file that's edited where it is.
    pub fn file(path: &Path) -> Self {
        Self { path: path.to_path_buf(), temporary: false, added_newline: false }
    }

    /// The text after editing.
    pub fn finish(mut self) -> Result<String, String> {
        let text = std::fs::read_to_string(&self.path).map_err(|e| e.to_string());
        let added = self.added_newline;
        self.cleanup();
        text.map(|text| if added { text.strip_suffix('\n').map(str::to_string).unwrap_or(text) } else { text })
    }

    pub fn discard(mut self) {
        self.cleanup();
    }

    fn cleanup(&mut self) {
        if self.temporary {
            self.temporary = false;
            let _ = std::fs::remove_file(&self.path);
            // And the folder made for these files, once it's empty.
            if let Some(dir) = self.path.parent() {
                let _ = std::fs::remove_dir(dir);
            }
        }
    }
}

impl Drop for Document {
    fn drop(&mut self) {
        self.cleanup();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_sanitizes_extension_and_cleans_up_on_drop() {
        let doc = Document::temporary("desc", "../../etc/passwd", "hello").unwrap();
        let path = doc.path.clone();
        assert!(path.starts_with(std::env::temp_dir().join(format!("familiar-tui-{}", std::process::id()))));
        assert_eq!(path.extension().and_then(|s| s.to_str()), Some("etcpasswd"));
        assert!(path.exists());
        drop(doc);
        assert!(!path.exists());
    }
}
