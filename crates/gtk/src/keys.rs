//! Converts GDK key events to the engine's UI-neutral [`KeyChord`], and chords back to GTK's
//! accelerators for menus and labels.

use gtk::gdk;
use gtk::prelude::*;
use familiar_engine::KeyChord;

/// The modifiers a chord records, taken off to find the key itself.
const CHORD_MODIFIERS: gdk::ModifierType = gdk::ModifierType::SHIFT_MASK
    .union(gdk::ModifierType::CONTROL_MASK)
    .union(gdk::ModifierType::ALT_MASK)
    .union(gdk::ModifierType::SUPER_MASK)
    .union(gdk::ModifierType::META_MASK)
    .union(gdk::ModifierType::HYPER_MASK);

/// The key press `controller` is handling, as a chord: the key as it is without Shift ("slash"
/// for "?", "1" for "!"), plus the modifiers. Returns None for presses of modifier keys on
/// their own.
pub fn chord(controller: &gtk::EventControllerKey, key: gdk::Key, keycode: u32, state: gdk::ModifierType) -> Option<KeyChord> {
    // With Shift, the keyboard's layout says which key is under the keycode without it. Only
    // then: GTK has no Num Lock modifier to translate with, so the keypad's keys would lose
    // their numbers.
    let base = if state.contains(gdk::ModifierType::SHIFT_MASK) {
        let layout = controller.current_event().and_then(|e| e.downcast::<gdk::KeyEvent>().ok()).map_or(0, |e| e.layout());
        gdk::Display::default()
            .and_then(|display| display.translate_key(keycode, state - CHORD_MODIFIERS, layout as i32))
            .map_or(key, |(base, ..)| base)
    } else {
        key
    };
    let name = base.to_lower().name()?;
    let is_modifier = [
        "Shift_L", "Shift_R", "Control_L", "Control_R", "Alt_L", "Alt_R", "Meta_L", "Meta_R", "Super_L", "Super_R",
        "ISO_Level3_Shift", "Caps_Lock", "Num_Lock", "Hyper_L", "Hyper_R",
    ]
    .contains(&name.as_str());
    if is_modifier {
        return None;
    }
    let chord = KeyChord::new(
        &name,
        state.contains(gdk::ModifierType::CONTROL_MASK),
        state.contains(gdk::ModifierType::ALT_MASK),
        state.contains(gdk::ModifierType::SHIFT_MASK),
    );
    Some(chord.with_cmd(state.contains(gdk::ModifierType::SUPER_MASK)))
}

/// The GDK key and modifiers for a chord, if GDK knows the key's name.
fn accelerator(chord: &KeyChord) -> Option<(gdk::Key, gdk::ModifierType)> {
    // GDK names letters in lower case; upper case would mean the shifted letter.
    let name = if chord.key.chars().count() == 1 { chord.key.to_lowercase() } else { chord.key.clone() };
    let key = gdk::Key::from_name(&name)?;
    let mut mods = gdk::ModifierType::empty();
    for (on, modifier) in [
        (chord.ctrl, gdk::ModifierType::CONTROL_MASK),
        (chord.alt, gdk::ModifierType::ALT_MASK),
        (chord.shift, gdk::ModifierType::SHIFT_MASK),
        (chord.cmd, gdk::ModifierType::SUPER_MASK),
    ] {
        if on {
            mods |= modifier;
        }
    }
    Some((key, mods))
}

/// A chord as GTK shows it: "Ctrl+Page Down", "Alt+]".
pub fn label(chord: &KeyChord) -> String {
    accelerator(chord).map_or_else(|| chord.to_string(), |(key, mods)| gtk::accelerator_get_label(key, mods).to_string())
}

/// A chord in GTK's accelerator syntax ("<Control>Page_Down"), for a menu item's "accel".
pub fn accel(chord: &KeyChord) -> Option<String> {
    accelerator(chord).map(|(key, mods)| gtk::accelerator_name(key, mods).to_string())
}
