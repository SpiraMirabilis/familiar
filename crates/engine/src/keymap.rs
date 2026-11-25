//! Keyboard shortcuts for the client's own commands: every action any frontend has, its default
//! keys on each platform, and the user's changes from `[settings.keys]` (desktop apps) or
//! `[settings.terminal_keys]` (the terminal client).
//!
//! Keys are written as [`KeyChord`]s with GDK's key names ("Ctrl+Page_Down", "Alt+bracketright",
//! "Cmd+Shift+G"), the way keyboard macros are. A frontend names a key press the same way it does
//! for macros: the key without Shift applied ("slash", not "question") plus the modifiers.
//!
//! The client's shortcuts come before keyboard macros: a macro on a key an action uses doesn't
//! run. [`Keymap::describe`] reports such clashes so the settings page can show them.

use std::collections::BTreeMap;

use familiar_config::{Config, Settings};
use serde::Serialize;

use crate::KeyChord;

/// Which app the shortcuts are for. The desktop apps share the action list but not the
/// defaults; the terminal client has its own list and its own setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux,
    Mac,
    Windows,
    Terminal,
}

impl Platform {
    /// The desktop app's platform for this build.
    pub fn desktop() -> Self {
        if cfg!(target_os = "macos") {
            Platform::Mac
        } else if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }

    pub fn is_terminal(self) -> bool {
        self == Platform::Terminal
    }
}

/// Where a shortcut works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// The main window: its tabs, output and input.
    Window,
    /// A program editor's window (MCP simpleedit, `/edit`). Desktop apps only.
    Editor,
}

/// Which apps have an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Apps {
    All,
    Desktop,
    Terminal,
}

/// One command a shortcut can run.
#[derive(Debug)]
pub struct Action {
    /// Stable name, used in the config: "next-tab".
    pub id: &'static str,
    /// For menus and the settings page: "Next Tab".
    pub label: &'static str,
    /// The settings page's heading for it.
    pub group: &'static str,
    pub scope: Scope,
    apps: Apps,
    linux: &'static [&'static str],
    windows: &'static [&'static str],
    mac: &'static [&'static str],
    terminal: &'static [&'static str],
}

impl Action {
    pub fn available(&self, platform: Platform) -> bool {
        match self.apps {
            Apps::All => true,
            Apps::Desktop => !platform.is_terminal(),
            Apps::Terminal => platform.is_terminal(),
        }
    }

    /// The keys it has until the user changes them.
    pub fn defaults(&self, platform: Platform) -> Vec<KeyChord> {
        let mut keys: Vec<&str> = match platform {
            Platform::Linux => self.linux,
            Platform::Windows => self.windows,
            Platform::Mac => self.mac,
            Platform::Terminal => self.terminal,
        }
        .to_vec();
        // Mac terminals send Option+→ and ← as Esc f and Esc b, which arrive as Alt+F and
        // Alt+B, so those switch tabs there too.
        if platform.is_terminal() && cfg!(target_os = "macos") {
            match self.id {
                "next-tab" => keys.push("Alt+F"),
                "prev-tab" => keys.push("Alt+B"),
                _ => {}
            }
        }
        keys.iter().filter_map(|k| KeyChord::parse(k)).collect()
    }
}

const fn action(
    id: &'static str,
    label: &'static str,
    group: &'static str,
    linux_windows: &'static [&'static str],
    mac: &'static [&'static str],
    terminal: &'static [&'static str],
) -> Action {
    Action { id, label, group, scope: Scope::Window, apps: Apps::All, linux: linux_windows, windows: linux_windows, mac, terminal }
}

const fn desktop(id: &'static str, label: &'static str, group: &'static str, linux_windows: &'static [&'static str], mac: &'static [&'static str]) -> Action {
    Action { apps: Apps::Desktop, ..action(id, label, group, linux_windows, mac, &[]) }
}

const fn terminal(id: &'static str, label: &'static str, group: &'static str, keys: &'static [&'static str]) -> Action {
    Action { apps: Apps::Terminal, ..action(id, label, group, &[], &[], keys) }
}

const fn editor(id: &'static str, label: &'static str, linux_windows: &'static [&'static str], mac: &'static [&'static str]) -> Action {
    Action { scope: Scope::Editor, ..desktop(id, label, EDITOR, linux_windows, mac) }
}

const WORLDS: &str = "Worlds";
const TABS: &str = "Tabs";
const OUTPUT: &str = "Output";
const DOCK: &str = "Side Dock";
const TOOLS: &str = "Tools";
const APP: &str = "Familiar";
const EDITOR: &str = "Program Editor";

/// Every action, in the order the settings page lists them.
pub const ACTIONS: &[Action] = &[
    action("connect", "Connect…", WORLDS, &["Ctrl+O"], &["Cmd+N"], &["Ctrl+O"]),
    action("new-world", "New World…", WORLDS, &["Ctrl+Shift+N"], &["Cmd+Shift+N"], &[]),
    action("edit-world", "Edit World…", WORLDS, &["Ctrl+Shift+E"], &["Cmd+Shift+E"], &[]),
    action("reconnect", "Reconnect", WORLDS, &["Ctrl+R"], &["Cmd+R"], &[]),
    action("disconnect", "Disconnect", WORLDS, &["Ctrl+Shift+D"], &["Cmd+Shift+D"], &[]),
    action("close-tab", "Close Tab", TABS, &["Ctrl+W"], &["Cmd+W"], &[]),
    action(
        "next-tab",
        "Next Tab",
        TABS,
        &["Alt+Right", "Ctrl+Tab", "Ctrl+Page_Down"],
        &["Ctrl+Tab", "Cmd+Shift+bracketright", "Cmd+Alt+Right"],
        &["Alt+Right", "Ctrl+Page_Down"],
    ),
    action(
        "prev-tab",
        "Previous Tab",
        TABS,
        &["Alt+Left", "Ctrl+Shift+Tab", "Ctrl+Page_Up"],
        &["Ctrl+Shift+Tab", "Cmd+Shift+bracketleft", "Cmd+Alt+Left"],
        &["Alt+Left", "Ctrl+Page_Up"],
    ),
    action("tab-1", "Tab 1", TABS, &["Alt+1"], &["Cmd+1"], &["Alt+1"]),
    action("tab-2", "Tab 2", TABS, &["Alt+2"], &["Cmd+2"], &["Alt+2"]),
    action("tab-3", "Tab 3", TABS, &["Alt+3"], &["Cmd+3"], &["Alt+3"]),
    action("tab-4", "Tab 4", TABS, &["Alt+4"], &["Cmd+4"], &["Alt+4"]),
    action("tab-5", "Tab 5", TABS, &["Alt+5"], &["Cmd+5"], &["Alt+5"]),
    action("tab-6", "Tab 6", TABS, &["Alt+6"], &["Cmd+6"], &["Alt+6"]),
    action("tab-7", "Tab 7", TABS, &["Alt+7"], &["Cmd+7"], &["Alt+7"]),
    action("tab-8", "Tab 8", TABS, &["Alt+8"], &["Cmd+8"], &["Alt+8"]),
    action("tab-9", "Tab 9", TABS, &["Alt+9"], &["Cmd+9"], &["Alt+9"]),
    action("find", "Find…", OUTPUT, &["Ctrl+F"], &["Cmd+F"], &["Ctrl+F"]),
    action("find-next", "Find Next", OUTPUT, &["F3", "Ctrl+G"], &["Cmd+G"], &["F3"]),
    action("find-previous", "Find Previous", OUTPUT, &["Shift+F3"], &["Cmd+Shift+G"], &["Shift+F3"]),
    action("clear", "Clear Output", OUTPUT, &["Ctrl+Shift+L"], &["Cmd+K"], &[]),
    action("toggle-log", "Log to a File", OUTPUT, &["Ctrl+Shift+G"], &["Cmd+Shift+L"], &[]),
    action("open-logs", "Show the Logs Folder", OUTPUT, &[], &[], &[]),
    action("toggle-sidebar", "Show or Hide the Side Dock", DOCK, &["Ctrl+B"], &["Cmd+Ctrl+S"], &["Ctrl+B"]),
    action("next-panel", "Next Side Dock Tab", DOCK, &["Alt+bracketright"], &["Cmd+Alt+bracketright"], &["Alt+bracketright"]),
    action("prev-panel", "Previous Side Dock Tab", DOCK, &["Alt+bracketleft"], &["Cmd+Alt+bracketleft"], &["Alt+bracketleft"]),
    action("show-map", "Show the Map", DOCK, &["Ctrl+Shift+M"], &["Cmd+Shift+M"], &[]),
    action("palette", "Command Palette…", TOOLS, &["Ctrl+K", "Ctrl+Shift+P"], &["Cmd+Shift+P"], &["Ctrl+K", "Ctrl+P"]),
    action("history-search", "Search Input History…", TOOLS, &["Ctrl+Shift+R"], &["Cmd+Shift+R"], &["Ctrl+R"]),
    action("rules", "Triggers, Aliases and Macros…", TOOLS, &["Ctrl+T"], &["Cmd+Shift+T"], &["F2"]),
    action("reopen-editor", "Reopen the Last Program", TOOLS, &["Alt+E"], &["Cmd+Shift+O"], &["Alt+E"]),
    action("restart-scripts", "Restart Scripts", TOOLS, &[], &[], &[]),
    action("stop-speech", "Stop Sounds and Speech", TOOLS, &["Ctrl+period"], &["Cmd+period"], &[]),
    terminal("edit-input", "Edit the Input in $EDITOR", TOOLS, &["Ctrl+G"]),
    terminal("editor-focus", "Between the World and the Editor Beside It", TOOLS, &["F6", "Alt+0"]),
    terminal("move-editor", "Move the Editor Between Its Tab and the Sidebar", TOOLS, &["F7"]),
    action("import-beipmu", "Import BeipMU Settings…", APP, &[], &[], &[]),
    action("import-map", "Import a BeipMU Map…", APP, &[], &[], &[]),
    action("preferences", "Settings…", APP, &["Ctrl+comma"], &["Cmd+comma"], &[]),
    action("keyboard", "Keyboard Shortcuts…", APP, &["F1"], &["Cmd+Alt+slash"], &["F1"]),
    action("open-settings-folder", "Open the Settings Folder", APP, &[], &[], &[]),
    action("help", "Familiar Help", APP, &[], &["Cmd+Shift+slash"], &[]),
    action("about", "About Familiar", APP, &[], &[], &[]),
    Action { linux: &["Ctrl+Q"], windows: &[], ..action("quit", "Quit", APP, &[], &["Cmd+Q"], &[]) },
    editor("editor-send", "Send to the Server", &["Ctrl+S"], &["Cmd+S"]),
    editor("editor-find", "Find…", &["Ctrl+F"], &["Cmd+F"]),
    editor("editor-find-next", "Find Next", &["F3", "Ctrl+G"], &["Cmd+G"]),
    editor("editor-find-previous", "Find Previous", &["Shift+F3", "Ctrl+Shift+G"], &["Cmd+Shift+G"]),
    editor("editor-reindent", "Re-indent", &["Ctrl+Shift+I"], &["Cmd+Shift+I"]),
];

/// The action with this id.
pub fn action_by_id(id: &str) -> Option<&'static Action> {
    ACTIONS.iter().find(|a| a.id == id)
}

/// The actions an app has, in order.
pub fn actions(platform: Platform) -> impl Iterator<Item = &'static Action> {
    ACTIONS.iter().filter(move |a| a.available(platform))
}

/// The shortcuts in effect for one app.
#[derive(Debug, Clone)]
pub struct Keymap {
    platform: Platform,
    /// For every available action, in [`ACTIONS`] order: its keys, and whether they're the
    /// user's rather than the defaults.
    bindings: Vec<(&'static Action, Vec<KeyChord>, bool)>,
}

impl Keymap {
    pub fn new(settings: &Settings, platform: Platform) -> Self {
        let changed = if platform.is_terminal() { &settings.terminal_keys } else { &settings.keys };
        let bindings = actions(platform)
            .map(|a| match changed.get(a.id) {
                Some(keys) => (a, keys.iter().filter_map(|k| KeyChord::parse(k)).collect(), true),
                None => (a, a.defaults(platform), false),
            })
            .collect();
        Self { platform, bindings }
    }

    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// The action a key press runs in `scope`, if any.
    pub fn lookup(&self, scope: Scope, chord: &KeyChord) -> Option<&'static str> {
        self.bindings.iter().find(|(a, keys, _)| a.scope == scope && keys.iter().any(|k| k.matches(chord))).map(|(a, _, _)| a.id)
    }

    /// An action's keys, the first being the one to show in a menu. Empty for an action
    /// without keys, or one this app doesn't have.
    pub fn keys(&self, id: &str) -> &[KeyChord] {
        self.bindings.iter().find(|(a, _, _)| a.id == id).map(|(_, keys, _)| keys.as_slice()).unwrap_or(&[])
    }

    /// The first key for an action, as text ("Ctrl+Shift+G"), for a menu or a hint.
    pub fn label(&self, id: &str) -> Option<String> {
        self.keys(id).first().map(KeyChord::to_string)
    }

    /// Every action with its keys, for a settings page: what clashes with what, and with which
    /// keyboard macros.
    pub fn describe(&self, config: &Config) -> Vec<ActionView> {
        let macros = macro_keys(config);
        self.bindings
            .iter()
            .map(|(a, keys, changed)| ActionView {
                id: a.id,
                label: a.label,
                group: a.group,
                scope: a.scope,
                changed: *changed,
                defaults: a.defaults(self.platform).iter().map(KeyChord::to_string).collect(),
                keys: keys
                    .iter()
                    .map(|k| KeyView {
                        key: k.to_string(),
                        clashes: self
                            .bindings
                            .iter()
                            .filter(|(other, other_keys, _)| other.id != a.id && other.scope == a.scope && other_keys.iter().any(|o| o.matches(k)))
                            .map(|(other, _, _)| other.label)
                            .collect(),
                        // Macros only run in the main window.
                        macros: if a.scope == Scope::Window {
                            macros.iter().filter(|(chord, _)| chord.matches(k)).map(|(_, whose)| whose.clone()).collect()
                        } else {
                            Vec::new()
                        },
                    })
                    .collect(),
            })
            .collect()
    }
}

/// Every keyboard macro's key, with whose macro it is ("Global", "World: Vale",
/// "Character: Mara (Vale)").
fn macro_keys(config: &Config) -> Vec<(KeyChord, String)> {
    let mut out = Vec::new();
    let mut add = |rules: &familiar_config::Rules, whose: String| {
        for m in &rules.macros {
            if let Some(chord) = KeyChord::parse(&m.key) {
                out.push((chord, whose.clone()));
            }
        }
    };
    add(&config.rules, "Global".into());
    for w in &config.worlds {
        add(&w.rules, format!("World: {}", w.name));
        for c in &w.characters {
            add(&c.rules, format!("Character: {} ({})", c.name, w.name));
        }
    }
    out
}

/// One action on a settings page.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ActionView {
    pub id: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub scope: Scope,
    /// The user changed its keys.
    pub changed: bool,
    pub defaults: Vec<String>,
    pub keys: Vec<KeyView>,
}

/// One of an action's keys, with what else uses it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct KeyView {
    pub key: String,
    /// Labels of other actions on the same key (in the same scope).
    pub clashes: Vec<&'static str>,
    /// Whose keyboard macros are on the same key. They don't run while the shortcut has it.
    pub macros: Vec<String>,
}

/// Sets an action's keys in `settings` (for `platform`'s table), or with `keys` None puts it
/// back to its defaults. Keys are stored in their normal spelling; ones that can't be read
/// are an error. Keys the same as the defaults aren't stored.
pub fn set_keys(settings: &mut Settings, platform: Platform, id: &str, keys: Option<&[String]>) -> Result<(), String> {
    let action = action_by_id(id).filter(|a| a.available(platform)).ok_or_else(|| format!("There's no shortcut called {id}"))?;
    let table: &mut BTreeMap<String, Vec<String>> = if platform.is_terminal() { &mut settings.terminal_keys } else { &mut settings.keys };
    let Some(keys) = keys else {
        table.remove(id);
        return Ok(());
    };
    let mut chords: Vec<KeyChord> = Vec::new();
    for k in keys {
        let chord = KeyChord::parse(k).ok_or_else(|| format!("\"{k}\" isn't a key Familiar knows"))?;
        if !chords.iter().any(|c| c.matches(&chord)) {
            chords.push(chord);
        }
    }
    let defaults = action.defaults(platform);
    if chords.len() == defaults.len() && chords.iter().zip(&defaults).all(|(a, b)| a.matches(b)) {
        table.remove(id);
    } else {
        table.insert(id.to_string(), chords.iter().map(KeyChord::to_string).collect());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(text: &str) -> KeyChord {
        KeyChord::parse(text).unwrap()
    }

    #[test]
    fn every_default_is_a_real_key_and_ids_are_unique() {
        let mut ids = std::collections::HashSet::new();
        for a in ACTIONS {
            assert!(ids.insert(a.id), "{} twice", a.id);
            for list in [a.linux, a.windows, a.mac, a.terminal] {
                for k in list {
                    assert!(KeyChord::parse(k).is_some(), "{}: {k}", a.id);
                }
            }
        }
    }

    #[test]
    fn no_default_clashes() {
        for platform in [Platform::Linux, Platform::Mac, Platform::Windows, Platform::Terminal] {
            let map = Keymap::new(&Settings::default(), platform);
            for view in map.describe(&Config::default()) {
                for k in &view.keys {
                    assert!(k.clashes.is_empty(), "{platform:?}: {} {} clashes with {:?}", view.id, k.key, k.clashes);
                }
            }
        }
    }

    #[test]
    fn lookup_by_scope_and_platform() {
        let linux = Keymap::new(&Settings::default(), Platform::Linux);
        assert_eq!(linux.lookup(Scope::Window, &chord("Alt+Right")), Some("next-tab"));
        assert_eq!(linux.lookup(Scope::Window, &chord("alt+right")), Some("next-tab"));
        assert_eq!(linux.lookup(Scope::Window, &chord("Ctrl+S")), None);
        assert_eq!(linux.lookup(Scope::Editor, &chord("Ctrl+S")), Some("editor-send"));
        let mac = Keymap::new(&Settings::default(), Platform::Mac);
        assert_eq!(mac.lookup(Scope::Window, &chord("Cmd+Shift+bracketright")), Some("next-tab"));
        assert_eq!(mac.lookup(Scope::Window, &chord("Ctrl+Shift+bracketright")), None);
        let term = Keymap::new(&Settings::default(), Platform::Terminal);
        assert_eq!(term.lookup(Scope::Window, &chord("Ctrl+G")), Some("edit-input"));
        assert_eq!(linux.lookup(Scope::Window, &chord("Ctrl+G")), Some("find-next"));
        assert!(term.keys("editor-send").is_empty(), "the terminal client has no program editor window");
        assert_eq!(term.keys("next-tab").contains(&chord("Alt+F")), cfg!(target_os = "macos"));
    }

    #[test]
    fn changes_override_the_defaults() {
        let mut settings = Settings::default();
        set_keys(&mut settings, Platform::Linux, "next-tab", Some(&["ctrl+shift+right".into(), "Ctrl+Shift+Right".into()])).unwrap();
        assert_eq!(settings.keys["next-tab"], ["Ctrl+Shift+Right"], "normal spelling, no repeats");
        set_keys(&mut settings, Platform::Linux, "close-tab", Some(&[])).unwrap();
        let map = Keymap::new(&settings, Platform::Linux);
        assert_eq!(map.lookup(Scope::Window, &chord("Alt+Right")), None);
        assert_eq!(map.lookup(Scope::Window, &chord("Ctrl+Shift+Right")), Some("next-tab"));
        assert_eq!(map.lookup(Scope::Window, &chord("Ctrl+W")), None, "unbound");
        assert!(settings.terminal_keys.is_empty(), "the terminal client's keys are separate");
        // Setting the defaults again, or resetting, removes the change.
        set_keys(&mut settings, Platform::Linux, "close-tab", Some(&["Ctrl+W".into()])).unwrap();
        set_keys(&mut settings, Platform::Linux, "next-tab", None).unwrap();
        assert!(settings.keys.is_empty());
        assert!(set_keys(&mut settings, Platform::Linux, "edit-input", Some(&[])).is_err(), "terminal only");
        assert!(set_keys(&mut settings, Platform::Linux, "find", Some(&["Hyper+F".into()])).is_err());
    }

    #[test]
    fn describe_reports_clashes_and_macros() {
        let mut config = Config::default();
        config.rules.macros.push(familiar_config::Macro { key: "Ctrl+R".into(), command: "look".into(), ..Default::default() });
        set_keys(&mut config.settings, Platform::Linux, "find", Some(&["Ctrl+O".into()])).unwrap();
        let views = Keymap::new(&config.settings, Platform::Linux).describe(&config);
        let find = views.iter().find(|v| v.id == "find").unwrap();
        assert!(find.changed);
        assert_eq!(find.defaults, ["Ctrl+F"]);
        assert_eq!(find.keys[0].clashes, ["Connect…"]);
        let reconnect = views.iter().find(|v| v.id == "reconnect").unwrap();
        assert_eq!(reconnect.keys[0].macros, ["Global"]);
        // Editor shortcuts don't clash with the main window's, or with macros.
        let send = views.iter().find(|v| v.id == "editor-find").unwrap();
        assert!(send.keys[0].clashes.is_empty() && send.keys[0].macros.is_empty());
    }
}
