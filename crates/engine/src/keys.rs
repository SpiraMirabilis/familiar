//! Key chords for keyboard macros, in a UI-neutral text form like "Ctrl+Shift+F5".

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct KeyChord {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Key name. Single characters are stored uppercase; names like "F5" or "KP_8" are kept as
    /// written. Comparison ignores case.
    pub key: String,
}

impl KeyChord {
    pub fn new(key: &str, ctrl: bool, alt: bool, shift: bool) -> Self {
        let key = if key.chars().count() == 1 { key.to_uppercase() } else { key.to_string() };
        Self { ctrl, alt, shift, key }
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
        self.ctrl == other.ctrl && self.alt == other.alt && self.shift == other.shift && self.eq_key(other)
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
        assert_eq!(KeyChord::parse(""), None);
        assert_eq!(KeyChord::parse("Hyper+x"), None);
    }

    #[test]
    fn matching_ignores_key_case() {
        let a = KeyChord::parse("Ctrl+kp_8").unwrap();
        assert!(a.matches(&KeyChord::new("KP_8", true, false, false)));
        assert!(!a.matches(&KeyChord::new("KP_8", false, false, false)));
        assert!(KeyChord::parse("a").unwrap().matches(&KeyChord::new("a", false, false, false)));
    }
}
