//! Converts GDK key events to the engine's UI-neutral [`KeyChord`].

use gtk::gdk;
use familiar_engine::KeyChord;

/// Returns None for presses of modifier keys on their own.
pub fn chord(key: gdk::Key, state: gdk::ModifierType) -> Option<KeyChord> {
    let name = key.to_lower().name()?;
    let is_modifier = [
        "Shift_L", "Shift_R", "Control_L", "Control_R", "Alt_L", "Alt_R", "Meta_L", "Meta_R", "Super_L", "Super_R",
        "ISO_Level3_Shift", "Caps_Lock", "Num_Lock", "Hyper_L", "Hyper_R",
    ]
    .contains(&name.as_str());
    if is_modifier {
        return None;
    }
    Some(KeyChord::new(
        &name,
        state.contains(gdk::ModifierType::CONTROL_MASK),
        state.contains(gdk::ModifierType::ALT_MASK),
        state.contains(gdk::ModifierType::SHIFT_MASK),
    ))
}
