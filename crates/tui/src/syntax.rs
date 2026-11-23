//! MOO syntax files for editors that have none (micro) or an older one (Vim and Neovim, whose
//! moo.vim predates Stunt), and where each editor looks for them. They're built in, so
//! `/editor syntax` can install them without the source.

use std::path::{Path, PathBuf};

const MICRO_MOO: &str = include_str!("../data/micro/moo.yaml");
const VIM_MOO: &str = include_str!("../data/vim/moo.vim");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editor {
    Micro,
    Vim,
    Neovim,
}

/// A syntax file and where its editor will find it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxFile {
    pub path: PathBuf,
    pub content: &'static str,
}

impl Editor {
    /// From the program's name, as `familiar_platform::editor_command` gives it.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "micro" => Some(Self::Micro),
            "vi" | "vim" | "gvim" | "mvim" => Some(Self::Vim),
            "nvim" => Some(Self::Neovim),
            _ => None,
        }
    }

    /// `$VISUAL` or `$EDITOR`, if it's one we have files for.
    pub fn current() -> Option<Self> {
        Self::from_name(&familiar_platform::editor_command().1)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Micro => "micro",
            Self::Vim => "Vim",
            Self::Neovim => "Neovim",
        }
    }

    /// Whether the editor highlights MOO without our file (only more plainly).
    pub fn knows_moo(self) -> bool {
        self != Self::Micro
    }

    /// The syntax file for `language` ("moo"), if there is one, where this editor looks for it.
    pub fn file(self, language: &str) -> Option<SyntaxFile> {
        let home = familiar_config::paths::home_dir()?;
        self.file_in(language, &home, |name| std::env::var(name).ok())
    }

    /// `file`, with the home folder and environment given, for testing.
    fn file_in(self, language: &str, home: &Path, env: impl Fn(&str) -> Option<String>) -> Option<SyntaxFile> {
        if !language.eq_ignore_ascii_case("moo") {
            return None;
        }
        // An empty variable counts as unset, as the editors treat it.
        let env = |name: &str| env(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let xdg_config = || env("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
        let (dir, name, content) = match self {
            Self::Micro => (env("MICRO_CONFIG_HOME").unwrap_or_else(|| xdg_config().join("micro")).join("syntax"), "moo.yaml", MICRO_MOO),
            Self::Neovim => {
                let config = match env("XDG_CONFIG_HOME") {
                    Some(dir) => dir.join("nvim"),
                    None if cfg!(windows) => env("LOCALAPPDATA").unwrap_or_else(|| home.join("AppData").join("Local")).join("nvim"),
                    None => home.join(".config").join("nvim"),
                };
                (config.join("syntax"), "moo.vim", VIM_MOO)
            }
            Self::Vim => (home.join(if cfg!(windows) { "vimfiles" } else { ".vim" }).join("syntax"), "moo.vim", VIM_MOO),
        };
        Some(SyntaxFile { path: dir.join(name), content })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Installed {
    Written,
    /// There was a file there already, and it was left alone.
    AlreadyThere,
}

impl SyntaxFile {
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Writes the file, making its folder if need be. One that's there already is only
    /// replaced if `force`.
    pub fn install(&self, force: bool) -> Result<Installed, String> {
        if self.exists() && !force {
            return Ok(Installed::AlreadyThere);
        }
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&self.path, self.content).map_err(|e| e.to_string())?;
        Ok(Installed::Written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        |name| vars.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
    }

    #[test]
    fn knows_editors_by_name() {
        assert_eq!(Editor::from_name("micro"), Some(Editor::Micro));
        assert_eq!(Editor::from_name("vi"), Some(Editor::Vim));
        assert_eq!(Editor::from_name("nvim"), Some(Editor::Neovim));
        assert_eq!(Editor::from_name("nano"), None);
    }

    #[test]
    fn only_moo_has_files() {
        let home = Path::new("/home/u");
        assert!(Editor::Micro.file_in("MOO", home, env(&[])).is_some());
        assert!(Editor::Micro.file_in("muf", home, env(&[])).is_none());
    }

    #[cfg(not(windows))]
    #[test]
    fn files_go_where_each_editor_looks() {
        let home = Path::new("/home/u");
        let path = |editor: Editor, vars: &[(&str, &str)]| editor.file_in("moo", home, env(vars)).unwrap().path;
        assert_eq!(path(Editor::Micro, &[]), Path::new("/home/u/.config/micro/syntax/moo.yaml"));
        assert_eq!(path(Editor::Micro, &[("XDG_CONFIG_HOME", "/x")]), Path::new("/x/micro/syntax/moo.yaml"));
        assert_eq!(path(Editor::Micro, &[("MICRO_CONFIG_HOME", "/m"), ("XDG_CONFIG_HOME", "/x")]), Path::new("/m/syntax/moo.yaml"));
        assert_eq!(path(Editor::Micro, &[("MICRO_CONFIG_HOME", "")]), Path::new("/home/u/.config/micro/syntax/moo.yaml"));
        assert_eq!(path(Editor::Neovim, &[]), Path::new("/home/u/.config/nvim/syntax/moo.vim"));
        assert_eq!(path(Editor::Neovim, &[("XDG_CONFIG_HOME", "/x")]), Path::new("/x/nvim/syntax/moo.vim"));
        assert_eq!(path(Editor::Vim, &[("XDG_CONFIG_HOME", "/x")]), Path::new("/home/u/.vim/syntax/moo.vim"));
    }

    #[test]
    fn install_leaves_an_existing_file_unless_forced() {
        let dir = std::env::temp_dir().join(format!("familiar-syntax-test-{}", std::process::id()));
        let file = SyntaxFile { path: dir.join("syntax").join("moo.vim"), content: "new" };
        assert_eq!(file.install(false), Ok(Installed::Written));
        std::fs::write(&file.path, "mine").unwrap();
        assert_eq!(file.install(false), Ok(Installed::AlreadyThere));
        assert_eq!(std::fs::read_to_string(&file.path).unwrap(), "mine");
        assert_eq!(file.install(true), Ok(Installed::Written));
        assert_eq!(std::fs::read_to_string(&file.path).unwrap(), "new");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
