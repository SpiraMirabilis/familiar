//! Smoothing over differences in how terminals report keys.

use familiar_engine::KeyChord;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// Punctuation keys and GDK's names for them, which shortcuts and macros are written with.
const PUNCTUATION: &[(char, &str)] = &[
    (',', "comma"),
    ('.', "period"),
    ('/', "slash"),
    (';', "semicolon"),
    ('\'', "apostrophe"),
    ('[', "bracketleft"),
    (']', "bracketright"),
    ('\\', "backslash"),
    ('-', "minus"),
    ('=', "equal"),
    ('`', "grave"),
    ('?', "question"),
    ('+', "plus"),
];

/// GDK's name for a punctuation key ("bracketright" for `]`), or "space".
pub fn punctuation_name(c: char) -> Option<&'static str> {
    if c == ' ' {
        return Some("space");
    }
    PUNCTUATION.iter().find(|(p, _)| *p == c).map(|(_, name)| *name)
}

/// A key as people read it: "Alt+→", "Ctrl+PgDn", "Alt+]".
pub fn pretty(chord: &KeyChord) -> String {
    let name = match chord.key.as_str() {
        "Left" => "←".to_string(),
        "Right" => "→".to_string(),
        "Up" => "↑".to_string(),
        "Down" => "↓".to_string(),
        "Page_Up" => "PgUp".to_string(),
        "Page_Down" => "PgDn".to_string(),
        "Return" => "Enter".to_string(),
        "Escape" => "Esc".to_string(),
        "BackSpace" => "Backspace".to_string(),
        "space" => "Space".to_string(),
        key => PUNCTUATION.iter().find(|(_, name)| *name == key).map_or_else(|| key.to_string(), |(c, _)| c.to_string()),
    };
    KeyChord { key: name, ..chord.clone() }.to_string()
}

/// A key written as in the config ("Alt+bracketright"), the way people read it.
pub fn pretty_text(key: &str) -> String {
    KeyChord::parse(key).map_or_else(|| key.to_string(), |c| pretty(&c))
}

/// A key as short as it can be written, for the status line: "^K" for Ctrl and a letter.
pub fn compact(chord: &KeyChord) -> String {
    if chord.ctrl && !chord.alt && !chord.shift && !chord.cmd && chord.key.chars().count() == 1 {
        return format!("^{}", chord.key);
    }
    pretty(chord)
}

/// The fewest keys arriving at once that can count as a paste. Nobody types this many between
/// two reads of the keyboard.
const PASTE_MIN_KEYS: usize = 8;

/// The Windows console reports AltGr as Ctrl+Alt, so on keyboards where `@`, `{` or `€` need
/// AltGr they would look like shortcuts and never get typed. A Ctrl+Alt key that produced a
/// symbol is that symbol; real Ctrl+Alt shortcuts are letters and digits.
pub fn normalize(mut key: KeyEvent, windows: bool) -> KeyEvent {
    let altgr = KeyModifiers::CONTROL | KeyModifiers::ALT;
    if windows
        && key.modifiers.contains(altgr)
        && let KeyCode::Char(c) = key.code
        && !c.is_ascii_alphanumeric()
        && !c.is_control()
    {
        key.modifiers.remove(altgr);
    }
    key
}

/// The character a key press types, if it's plain text.
fn typed(event: &Event) -> Option<char> {
    let Event::Key(key) = event else { return None };
    if key.kind == KeyEventKind::Release || key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
        return None;
    }
    match key.code {
        KeyCode::Char(c) => Some(c),
        KeyCode::Enter => Some('\n'),
        KeyCode::Tab => Some('\t'),
        _ => None,
    }
}

/// The Windows console has no bracketed paste: pasted text arrives as a burst of key presses,
/// and each Enter in it would send a line. Turns a burst that reads like pasted text (several
/// lines' worth of plain characters, all available at once) into one paste event. `events` is
/// everything that was waiting to be read.
pub fn coalesce_paste(events: Vec<Event>) -> Vec<Event> {
    let is_release = |e: &Event| matches!(e, Event::Key(k) if k.kind == KeyEventKind::Release);
    let mut text = String::new();
    for event in events.iter().filter(|e| !is_release(e)) {
        match typed(event) {
            Some(c) => text.push(c),
            None => return events,
        }
    }
    // An Enter with text after it. One line ending in Enter is left as typing: that's also what
    // a macro tool or dictation software sending a command looks like, and it should be sent.
    let multiline = text.trim_end_matches('\n').contains('\n');
    if text.chars().count() >= PASTE_MIN_KEYS && multiline { vec![Event::Paste(text)] } else { events }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    fn keys(text: &str) -> Vec<Event> {
        text.chars().map(|c| press(if c == '\n' { KeyCode::Enter } else { KeyCode::Char(c) }, KeyModifiers::NONE)).collect()
    }

    #[test]
    fn keys_are_shown_the_way_people_read_them() {
        let show = |text: &str| pretty(&KeyChord::parse(text).unwrap());
        assert_eq!(show("Alt+Right"), "Alt+→");
        assert_eq!(show("Ctrl+Page_Down"), "Ctrl+PgDn");
        assert_eq!(show("Alt+bracketright"), "Alt+]");
        assert_eq!(show("Shift+F3"), "Shift+F3");
        assert_eq!(compact(&KeyChord::parse("Ctrl+K").unwrap()), "^K");
        assert_eq!(compact(&KeyChord::parse("Ctrl+Shift+K").unwrap()), "Ctrl+Shift+K");
        assert_eq!(punctuation_name(']'), Some("bracketright"));
        assert_eq!(punctuation_name('a'), None);
    }

    #[test]
    fn altgr_symbols_are_typed_on_windows() {
        let altgr = KeyModifiers::CONTROL | KeyModifiers::ALT;
        let at = KeyEvent::new(KeyCode::Char('@'), altgr);
        assert_eq!(normalize(at, true).modifiers, KeyModifiers::NONE);
        assert_eq!(normalize(KeyEvent::new(KeyCode::Char('€'), altgr | KeyModifiers::SHIFT), true).modifiers, KeyModifiers::SHIFT);
        // Elsewhere, and for real shortcuts, nothing changes.
        assert_eq!(normalize(at, false).modifiers, altgr);
        assert_eq!(normalize(KeyEvent::new(KeyCode::Char('a'), altgr), true).modifiers, altgr);
        assert_eq!(normalize(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL), true).modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn a_burst_of_lines_is_a_paste() {
        let out = coalesce_paste(keys("say hello\nsay again\n"));
        assert_eq!(out, vec![Event::Paste("say hello\nsay again\n".into())]);
    }

    #[test]
    fn key_releases_in_a_paste_are_dropped() {
        let mut events = keys("line one\nline two");
        let mut release = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        events.insert(3, Event::Key(release));
        assert_eq!(coalesce_paste(events), vec![Event::Paste("line one\nline two".into())]);
    }

    #[test]
    fn typing_is_left_alone() {
        // Fast typing: a few keys, even with Enter.
        let typing = keys("hi\n");
        assert_eq!(coalesce_paste(typing.clone()), typing);
        // One long line needs no special handling, with or without Enter at the end (which is
        // also how a macro tool sends a command).
        for text in ["a single pasted line", "a command sent all at once\n"] {
            let line = keys(text);
            assert_eq!(coalesce_paste(line.clone()), line);
        }
        // Anything that isn't text (a shortcut, the mouse) means it isn't a paste.
        let mut mixed = keys("line one\nline two");
        mixed.push(press(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(coalesce_paste(mixed.clone()), mixed);
    }
}
