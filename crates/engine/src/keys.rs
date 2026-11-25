//! Key chords for keyboard macros, in a UI-neutral text form like "Ctrl+Shift+F5".

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct KeyChord {
    /// The Mac's Command key (the Super or Windows key elsewhere).
    pub cmd: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Key name. Single characters are stored uppercase; names like "F5" or "KP_8" are kept as
    /// written. Comparison ignores case.
    pub key: String,
}

impl KeyChord {
    pub fn new(key: &str, ctrl: bool, alt: bool, shift: bool) -> Self {
        let key = if key.chars().count() == 1 { key.to_uppercase() } else { canonical_name(key) };
        Self { cmd: false, ctrl, alt, shift, key }
    }

    /// With the Command (Super) key too.
    pub fn with_cmd(mut self, cmd: bool) -> Self {
        self.cmd = cmd;
        self
    }

    /// Parses "Ctrl+Alt+K", "control+k", "F5" or "Shift+KP_8". Returns None for an empty key.
    pub fn parse(text: &str) -> Option<Self> {
        let parts: Vec<&str> = text.split('+').map(str::trim).collect();
        // A trailing "+" means the plus key itself ("Ctrl++").
        let (mods, key) = match parts.as_slice() {
            [.., "", ""] => (&parts[..parts.len() - 2], "+"),
            [mods @ .., key] => (mods, *key),
            [] => return None,
        };
        if key.is_empty() {
            return None;
        }
        let mut chord = Self::new(key, false, false, false);
        for m in mods {
            match m.to_ascii_lowercase().as_str() {
                "cmd" | "command" | "super" => chord.cmd = true,
                "ctrl" | "control" => chord.ctrl = true,
                "alt" | "meta" => chord.alt = true,
                "shift" => chord.shift = true,
                _ => return None,
            }
        }
        Some(chord)
    }

    fn eq_key(&self, other: &Self) -> bool {
        self.key.eq_ignore_ascii_case(&other.key)
    }

    pub fn matches(&self, other: &Self) -> bool {
        self.cmd == other.cmd && self.ctrl == other.ctrl && self.alt == other.alt && self.shift == other.shift && self.eq_key(other)
    }
}

/// Named keys in GDK's spelling, which every frontend uses.
const NAMES: &[&str] = &[
    "Return", "Tab", "Escape", "BackSpace", "Delete", "Insert", "Home", "End", "Page_Up", "Page_Down", "Up", "Down", "Left",
    "Right", "space", "Pause", "Print", "Menu", "Num_Lock", "comma", "period", "slash", "semicolon", "apostrophe",
    "bracketleft", "bracketright", "backslash", "minus", "equal", "grave", "question", "plus", "KP_Enter", "KP_Add",
    "KP_Subtract", "KP_Multiply", "KP_Divide", "KP_Decimal", "KP_Equal", "KP_Begin", "KP_Home", "KP_End", "KP_Page_Up",
    "KP_Page_Down", "KP_Up", "KP_Down", "KP_Left", "KP_Right", "KP_Insert", "KP_Delete",
];

/// A key name as GDK spells it ("page_down" is "Page_Down", "f5" is "F5"). Names it doesn't
/// know are left alone.
fn canonical_name(key: &str) -> String {
    if let Some(name) = NAMES.iter().find(|n| n.eq_ignore_ascii_case(key)) {
        return name.to_string();
    }
    let upper = key.to_ascii_uppercase();
    let numbered = |prefix: &str| upper.strip_prefix(prefix).is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    if numbered("F") || numbered("KP_") {
        return upper;
    }
    key.to_string()
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.cmd {
            f.write_str("Cmd+")?;
        }
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        f.write_str(&self.key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display() {
        let k = KeyChord::parse("control+shift+k").unwrap();
        assert_eq!(k.to_string(), "Ctrl+Shift+K");
        assert_eq!(KeyChord::parse("F5").unwrap().to_string(), "F5");
        assert_eq!(KeyChord::parse("Ctrl++").unwrap().key, "+");
        assert_eq!(KeyChord::parse("ctrl+page_down").unwrap().to_string(), "Ctrl+Page_Down");
        assert_eq!(KeyChord::parse("f12").unwrap().to_string(), "F12");
        assert_eq!(KeyChord::parse("kp_8").unwrap().to_string(), "KP_8");
        assert_eq!(KeyChord::parse("Alt+SPACE").unwrap().to_string(), "Alt+space");
        assert_eq!(KeyChord::parse(""), None);
        assert_eq!(KeyChord::parse("Hyper+x"), None);
        let cmd = KeyChord::parse("shift+command+bracketright").unwrap();
        assert_eq!(cmd.to_string(), "Cmd+Shift+bracketright");
        assert!(!cmd.matches(&KeyChord::new("bracketright", false, false, true)));
    }

    #[test]
    fn matching_ignores_key_case() {
        let a = KeyChord::parse("Ctrl+kp_8").unwrap();
        assert!(a.matches(&KeyChord::new("KP_8", true, false, false)));
        assert!(!a.matches(&KeyChord::new("KP_8", false, false, false)));
        assert!(KeyChord::parse("a").unwrap().matches(&KeyChord::new("a", false, false, false)));
    }
}
