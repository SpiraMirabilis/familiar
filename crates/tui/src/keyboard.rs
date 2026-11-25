//! The keyboard shortcuts window: every action with its keys, to add keys to, remove them from
//! and reset. The actions and their keys come from `Keymap::describe` each time, so the window
//! always shows what's in effect.

use familiar_engine::KeyChord;

#[derive(Default)]
pub struct KeysView {
    /// The selected action (an index into `Keymap::describe`'s list) and which of its keys.
    pub selected: usize,
    pub key: usize,
    /// Waiting for a key press to add to the selected action.
    pub recording: bool,
    /// R was pressed once: pressing it again resets every shortcut.
    pub reset_armed: bool,
    /// What the last key did, said in place of the key hints until the next key.
    pub notice: Option<String>,
}

impl KeysView {
    /// Moves the selection by `delta` actions, staying in the list.
    pub fn select(&mut self, delta: isize, count: usize) {
        let last = count.saturating_sub(1) as isize;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
        self.key = 0;
    }
}

/// Why a key can't be a shortcut, if it can't: typing keys, and the keys the input box keeps,
/// have to stay as they are.
pub fn unusable(chord: &KeyChord) -> Option<&'static str> {
    if chord.ctrl && !chord.alt && chord.key == "C" {
        return Some("Ctrl+C clears the input and quits, so it can't be a shortcut");
    }
    let special = chord.key.len() > 1 && (chord.key.starts_with('F') && chord.key[1..].chars().all(|c| c.is_ascii_digit()) || chord.key.starts_with("KP_") || chord.key == "Insert");
    if !(chord.ctrl || chord.alt || special) {
        return Some("Use a function key, or a key with Ctrl or Alt: the others are for typing");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_keys_cant_be_shortcuts() {
        let check = |text: &str| unusable(&KeyChord::parse(text).unwrap());
        for ok in ["Ctrl+F", "Alt+Right", "F5", "Shift+F3", "KP_8", "Ctrl+Alt+C", "Alt+C"] {
            assert_eq!(check(ok), None, "{ok}");
        }
        for bad in ["Ctrl+C", "A", "Right", "Shift+Tab", "Return", "Page_Up", "space"] {
            assert!(check(bad).is_some(), "{bad}");
        }
    }
}
