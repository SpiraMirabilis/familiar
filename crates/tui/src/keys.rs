//! Smoothing over differences in how terminals report keys.

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

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
    if text.chars().count() >= PASTE_MIN_KEYS && text.contains('\n') { vec![Event::Paste(text)] } else { events }
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
        // One long line needs no special handling.
        let line = keys("a single pasted line");
        assert_eq!(coalesce_paste(line.clone()), line);
        // Anything that isn't text (a shortcut, the mouse) means it isn't a paste.
        let mut mixed = keys("line one\nline two");
        mixed.push(press(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(coalesce_paste(mixed.clone()), mixed);
    }
}
