//! Keys for the triggers, aliases and macros window.

use super::events::chord;
use super::*;

impl App {
    /// Opens the window on the rules of the tab's character (or its world, or all worlds).
    pub(super) fn open_rules(&mut self) {
        let preferred = self.current().and_then(|(i, _)| {
            let engine = &self.sessions[i].engine;
            self.config.scopes_for(&engine.world().name, engine.character_name().as_deref()).into_iter().next()
        });
        self.overlay = Some(Overlay::Rules(Box::new(RulesView::new(&self.config, preferred.as_ref()))));
    }

    /// Saves changed rules and gives them to the open sessions.
    fn rules_changed(&mut self) {
        self.save_config();
        for idx in 0..self.sessions.len() {
            let out = self.sessions[idx].engine.reload(&self.config);
            self.apply(idx, out);
        }
    }

    pub(super) fn on_rules_key(&mut self, key: KeyEvent) {
        let Some(Overlay::Rules(view)) = &mut self.overlay else { return };
        if view.form.is_some() {
            return self.on_rule_form_key(key);
        }
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let count = view.rows(&self.config).len();
        let armed = std::mem::take(&mut view.delete_armed);
        view.notice = None;
        let rules = self.config.rules_mut(&view.scopes[view.scope]);
        let changed = match key.code {
            KeyCode::Esc => {
                self.overlay = None;
                false
            }
            KeyCode::Tab | KeyCode::Right => {
                view.change_section(1);
                false
            }
            KeyCode::BackTab | KeyCode::Left => {
                view.change_section(-1);
                false
            }
            KeyCode::Char(']') => {
                view.change_scope(1);
                false
            }
            KeyCode::Char('[') => {
                view.change_scope(-1);
                false
            }
            KeyCode::Up | KeyCode::Char('K') if shift || key.code != KeyCode::Up => rules.is_some_and(|r| view.move_by(r, -1)),
            KeyCode::Down | KeyCode::Char('J') if shift || key.code != KeyCode::Down => rules.is_some_and(|r| view.move_by(r, 1)),
            KeyCode::Up | KeyCode::Char('k') => {
                view.select(-1, count);
                false
            }
            KeyCode::Down | KeyCode::Char('j') => {
                view.select(1, count);
                false
            }
            KeyCode::PageUp => {
                view.select(-10, count);
                false
            }
            KeyCode::PageDown => {
                view.select(10, count);
                false
            }
            KeyCode::Home => {
                view.select(isize::MIN / 2, count);
                false
            }
            KeyCode::End => {
                view.select(isize::MAX / 2, count);
                false
            }
            KeyCode::Enter | KeyCode::Char('e') => {
                view.open_form(&self.config, count == 0);
                false
            }
            KeyCode::Char('n' | 'a' | '+') => {
                view.open_form(&self.config, true);
                false
            }
            KeyCode::Char(' ') => rules.is_some_and(|r| view.toggle(r)),
            // Examples of every kind, to read and change. Triggers and aliases come switched off.
            KeyCode::Char('x') => rules.is_some_and(|r| {
                let added = r.add_examples();
                view.notice = Some(added.describe());
                added.total() > 0
            }),
            KeyCode::Char('d') | KeyCode::Delete if armed => rules.is_some_and(|r| view.delete(r)),
            KeyCode::Char('d') | KeyCode::Delete => {
                view.delete_armed = count > 0;
                false
            }
            _ => false,
        };
        if changed {
            self.rules_changed();
        }
    }

    fn on_rule_form_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let Some(Overlay::Rules(view)) = &mut self.overlay else { return };
        let Some(form) = &mut view.form else { return };
        if form.recording {
            form.recording = false;
            if key.code != KeyCode::Esc {
                // A plain character would stop you typing it.
                let plain = matches!(key.code, KeyCode::Char(_)) && !ctrl && !alt && !key.state.contains(KeyEventState::KEYPAD);
                match chord(&key).filter(|_| !plain) {
                    Some(chord) => form.focused().input.set(&chord.to_string()),
                    None => form.error = Some("Use a function key, the number pad, or a key with Ctrl or Alt".into()),
                }
            }
            return;
        }
        form.error = None;
        let kind = form.fields[form.focus].kind;
        match key.code {
            KeyCode::Esc => view.form = None,
            KeyCode::Enter if kind == FieldKind::Multi && (shift || alt) => form.focused().input.insert("\n"),
            KeyCode::Char('j') if ctrl && kind == FieldKind::Multi => form.focused().input.insert("\n"),
            KeyCode::Enter | KeyCode::Char('s') if key.code == KeyCode::Enter || ctrl => {
                let saved = match self.config.rules_mut(&view.scopes[view.scope]) {
                    Some(rules) => form.save(rules),
                    None => Err("That world or character no longer exists".into()),
                };
                match saved {
                    Ok(row) => {
                        view.selected = row;
                        view.form = None;
                        self.rules_changed();
                    }
                    Err(e) => form.error = Some(e),
                }
            }
            KeyCode::Char('t') if ctrl && form.on_key_field() => form.recording = true,
            KeyCode::Tab => form.next(),
            KeyCode::BackTab => form.prev(),
            KeyCode::Down => {
                if !(kind == FieldKind::Multi && form.focused().input.line_down()) {
                    form.next();
                }
            }
            KeyCode::Up => {
                if !(kind == FieldKind::Multi && form.focused().input.line_up()) {
                    form.prev();
                }
            }
            KeyCode::PageDown => form.focus = (form.focus + 8).min(form.fields.len() - 1),
            KeyCode::PageUp => form.focus = form.focus.saturating_sub(8),
            KeyCode::Char(' ') if kind == FieldKind::Toggle => {
                let field = form.focused();
                field.on = !field.on;
            }
            _ if kind == FieldKind::Toggle => {}
            KeyCode::Backspace if ctrl || alt => form.focused().input.kill_word(),
            KeyCode::Backspace => form.focused().input.backspace(),
            KeyCode::Delete => form.focused().input.delete(),
            KeyCode::Left if ctrl => form.focused().input.word_left(),
            KeyCode::Right if ctrl => form.focused().input.word_right(),
            KeyCode::Left => form.focused().input.left(),
            KeyCode::Right => form.focused().input.right(),
            KeyCode::Home => form.focused().input.home(),
            KeyCode::End => form.focused().input.end(),
            KeyCode::Char('a') if ctrl => form.focused().input.home(),
            KeyCode::Char('e') if ctrl => form.focused().input.end(),
            KeyCode::Char('u') if ctrl => form.focused().input.kill_to_start(),
            KeyCode::Char('k') if ctrl => form.focused().input.kill_to_end(),
            KeyCode::Char('w') if ctrl => form.focused().input.kill_word(),
            KeyCode::Char(c) if !ctrl => {
                let mut buf = [0; 4];
                form.focused().input.insert(c.encode_utf8(&mut buf));
            }
            _ => {}
        }
    }
}
