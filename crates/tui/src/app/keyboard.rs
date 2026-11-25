//! Keys for the keyboard shortcuts window. Changes are saved at once and take effect at once.

use familiar_engine::keymap;

use super::events::chord;
use super::*;
use crate::keyboard::unusable;
use crate::keys::pretty;

impl App {
    pub(super) fn on_keys_key(&mut self, key: KeyEvent) {
        let views = self.keymap.describe(&self.config);
        let Some(Overlay::Keys(view)) = &mut self.overlay else { return };
        let count = views.len();
        view.selected = view.selected.min(count.saturating_sub(1));
        let Some(action) = views.get(view.selected) else { return };
        let keys: Vec<String> = action.keys.iter().map(|k| k.key.clone()).collect();
        view.key = view.key.min(keys.len().saturating_sub(1));

        // Recording: the key pressed is the new key. Nothing else runs meanwhile.
        if view.recording {
            view.recording = false;
            if key.code == KeyCode::Esc {
                return;
            }
            let Some(chord) = chord(&key) else {
                view.notice = Some("That key can't be told apart from others in a terminal".into());
                return;
            };
            if let Some(reason) = unusable(&chord) {
                view.notice = Some(reason.into());
                return;
            }
            if let Some(at) = keys.iter().position(|k| KeyChord::parse(k).is_some_and(|c| c.matches(&chord))) {
                view.key = at;
                view.notice = Some(format!("{} already has {}", action.label, pretty(&chord)));
                return;
            }
            view.key = keys.len();
            let notice = format!("Added {} to {}", pretty(&chord), action.label);
            let mut keys = keys;
            keys.push(chord.to_string());
            return self.set_action_keys(action.id, Some(keys), notice);
        }

        let armed = std::mem::take(&mut view.reset_armed);
        view.notice = None;
        match key.code {
            KeyCode::Esc => self.overlay = None,
            KeyCode::Up | KeyCode::Char('k') => view.select(-1, count),
            KeyCode::Down | KeyCode::Char('j') => view.select(1, count),
            KeyCode::PageUp => view.select(-10, count),
            KeyCode::PageDown => view.select(10, count),
            KeyCode::Home => view.select(isize::MIN / 2, count),
            KeyCode::End => view.select(isize::MAX / 2, count),
            KeyCode::Left | KeyCode::Char('h') => view.key = view.key.saturating_sub(1),
            KeyCode::Right | KeyCode::Char('l') => view.key = (view.key + 1).min(keys.len().saturating_sub(1)),
            KeyCode::Enter | KeyCode::Char('a' | 'n' | '+') => view.recording = true,
            KeyCode::Char('d' | '-') | KeyCode::Delete | KeyCode::Backspace => {
                if let Some(removed) = keys.get(view.key) {
                    let notice = format!("Removed {} from {}", crate::keys::pretty_text(removed), action.label);
                    let rest: Vec<String> = keys.iter().filter(|k| *k != removed).cloned().collect();
                    view.key = view.key.saturating_sub(1);
                    self.set_action_keys(action.id, Some(rest), notice);
                }
            }
            KeyCode::Char('r') if action.changed => {
                view.key = 0;
                let notice = format!("{} has its usual keys again", action.label);
                self.set_action_keys(action.id, None, notice);
            }
            KeyCode::Char('r') => view.notice = Some(format!("{} already has its usual keys", action.label)),
            KeyCode::Char('R') if armed => {
                self.config.settings.terminal_keys.clear();
                self.keys_changed();
                if let Some(Overlay::Keys(view)) = &mut self.overlay {
                    view.notice = Some("Every shortcut has its usual keys again".into());
                }
            }
            KeyCode::Char('R') if views.iter().any(|v| v.changed) => view.reset_armed = true,
            KeyCode::Char('R') => view.notice = Some("No shortcut has been changed".into()),
            KeyCode::Char('?') => self.overlay = Some(Overlay::Help),
            _ => {}
        }
    }

    /// Gives an action these keys (None: its defaults), saves, and says `notice` (or what went
    /// wrong) in the window.
    fn set_action_keys(&mut self, id: &str, keys: Option<Vec<String>>, notice: String) {
        let result = keymap::set_keys(&mut self.config.settings, Platform::Terminal, id, keys.as_deref());
        if result.is_ok() {
            self.keys_changed();
        }
        if let Some(Overlay::Keys(view)) = &mut self.overlay {
            view.notice = Some(result.err().unwrap_or(notice));
        }
    }

    /// The shortcuts changed: save them and use them.
    fn keys_changed(&mut self) {
        self.save_config();
        self.keymap = Keymap::new(&self.config.settings, Platform::Terminal);
    }
}
