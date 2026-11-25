//! What the keyboard shortcuts' actions do (`familiar_engine::keymap`), the search over the
//! output, and importing BeipMU's settings.

use std::path::Path;

use super::events::chord;
use super::*;
use crate::buffer;

/// Familiar's online help.
const HELP_URL: &str = "https://github.com/SpiraMirabilis/familiar#readme";

impl App {
    /// Runs an action by its id ("next-tab"), from a key or the command palette.
    pub fn run(&mut self, id: &str) {
        let current = self.current();
        match id {
            "connect" => self.open_connect_picker(),
            "new-world" => self.overlay = Some(Overlay::World(Box::new(WorldForm::new(None, None, false)))),
            "edit-world" => match current {
                Some((i, _)) => {
                    let world = self.sessions[i].engine.world().clone();
                    let saved = self.config.worlds.iter().find(|w| w.name.eq_ignore_ascii_case(&world.name)).cloned();
                    let character = self.sessions[i].engine.character_name();
                    let form = match &saved {
                        Some(w) => WorldForm::new(Some(w), character.as_deref(), true),
                        // A quick connection: offer to save it.
                        None => WorldForm::new(Some(&world), character.as_deref(), false),
                    };
                    self.overlay = Some(Overlay::World(Box::new(form)));
                }
                None => self.run("new-world"),
            },
            "reconnect" => {
                if let Some((i, _)) = current {
                    self.connect(i);
                }
            }
            "disconnect" => {
                if let Some((i, _)) = current {
                    self.sessions[i].engine.disconnect();
                }
            }
            "close-tab" => self.close_tab(),
            "next-tab" if !self.tabs.is_empty() => self.switch_to((self.active + 1) % self.tabs.len()),
            "prev-tab" if !self.tabs.is_empty() => self.switch_to((self.active + self.tabs.len() - 1) % self.tabs.len()),
            "find" => self.open_find(),
            "find-next" => self.find_again(true),
            "find-previous" => self.find_again(false),
            "clear" => {
                if let Some(b) = self.current_buffer() {
                    b.clear();
                }
            }
            "toggle-log" => {
                if let Some((i, _)) = current {
                    let out = self.sessions[i].engine.toggle_log();
                    self.apply(i, out);
                }
            }
            "open-logs" => self.notice(&format!("Logs are kept in {}", familiar_config::paths::display(&familiar_config::log_dir()))),
            "toggle-sidebar" => self.sidebar = !self.sidebar,
            "next-panel" | "prev-panel" => {
                if let Some((i, _)) = current {
                    let s = &mut self.sessions[i];
                    let n = s.panels().len().max(1);
                    s.panel = if id == "next-panel" { (s.panel + 1) % n } else { (s.panel + n - 1) % n };
                    self.sidebar = true;
                }
            }
            "show-map" => {
                if let Some((i, _)) = current {
                    let out = self.sessions[i].engine.submit("/map", false);
                    self.apply(i, out);
                }
            }
            "palette" => self.open_palette(),
            "history-search" => self.open_history(),
            "rules" => self.open_rules(),
            "reopen-editor" => {
                // The last program sent from the editor, at its first error.
                if let Some((i, _)) = current
                    && let Some((request, line)) = self.sessions[i].last_edit.clone()
                {
                    self.edit(External::Edit { session: self.sessions[i].id, request, line });
                } else {
                    self.toast("Nothing to reopen: no program has been edited here yet");
                }
            }
            "restart-scripts" => match current {
                Some((i, _)) => {
                    let out = self.sessions[i].engine.submit("/script reload", false);
                    self.apply(i, out);
                }
                None => self.toast("Scripts run in a world's tab: open one first"),
            },
            "stop-speech" => {
                self.speaker.stop();
                self.sounds.stop(None);
            }
            "edit-input" => {
                if let Some((i, puppet)) = current {
                    self.edit(External::EditInput { session: self.sessions[i].id, puppet });
                }
            }
            "editor-focus" => self.toggle_editor_focus(),
            "move-editor" => self.move_editor(),
            "import-beipmu" => self.insert_command("/import "),
            "import-map" => self.insert_command("/map import "),
            "preferences" => self.edit(External::EditConfig),
            "keyboard" => self.overlay = Some(Overlay::Keys(Box::default())),
            "open-settings-folder" => {
                let dir = familiar_config::paths::display(&familiar_config::paths::config_dir());
                self.notice(&format!("Settings are kept in {dir}; the config file is {}", familiar_config::paths::display(&self.config_path)));
            }
            "help" => {
                self.notice(&format!("Familiar's help is online: {HELP_URL}"));
                self.overlay = Some(Overlay::Help);
            }
            "about" => self.notice(&format!("{} {}, a terminal client for MUDs, MUSHes, MUCKs and MOOs. {HELP_URL}", familiar_config::APP_NAME, env!("CARGO_PKG_VERSION"))),
            "quit" => self.quit = true,
            id => {
                if let Some(n) = id.strip_prefix("tab-").and_then(|n| n.parse::<usize>().ok()) {
                    self.switch_to(n.saturating_sub(1));
                }
            }
        }
    }

    /// Puts the start of a command in the input box, for the rest to be typed.
    fn insert_command(&mut self, text: &str) {
        match self.current_input() {
            Some(input) => input.set(text),
            None => self.toast(&format!("Type {}… in a world's tab", text.trim_end())),
        }
    }

    // ----- Searching the output -----

    fn open_find(&mut self) {
        let Some(buffer) = self.current_buffer().map(|b| b.id) else {
            return self.toast("There's no output here to search");
        };
        self.overlay = Some(Overlay::Find(Box::new(Find { query: Input::new(1), buffer, current: None, position: (0, 0) })));
    }

    /// Closes the search bar, keeping the search for Find Next.
    pub(super) fn close_find(&mut self) {
        if let Some(Overlay::Find(find)) = self.overlay.take() {
            self.last_find = Some(find);
        }
    }

    /// Find Next (older) or Previous (newer), reopening the last search if the bar is closed.
    fn find_again(&mut self, older: bool) {
        if !matches!(self.overlay, Some(Overlay::Find(_))) {
            let buffer = self.current_buffer().map(|b| b.id);
            match (self.last_find.take(), buffer) {
                (Some(mut find), Some(buffer)) if !find.query.is_empty() => {
                    if find.buffer != buffer {
                        (find.buffer, find.current) = (buffer, None);
                    }
                    self.overlay = Some(Overlay::Find(find));
                }
                _ => return self.open_find(),
            }
        }
        self.find_step(Some(older));
    }

    /// Moves the search to a match and scrolls the output to it: the newest (with `older`
    /// None, for a changed query), or the next older or newer one, wrapping around.
    pub(super) fn find_step(&mut self, older: Option<bool>) {
        let (width, height) = self.view;
        let Some(Overlay::Find(find)) = &self.overlay else { return };
        let (id, at) = (find.buffer, find.current.as_ref().map(|(seq, r)| (*seq, r.start)));
        let query = find.query.text.clone();
        let Some(buffer) = self.buffer(id) else { return };
        let all = buffer.find_all(&query);
        let index = match (older, at) {
            _ if all.is_empty() => None,
            (Some(true), Some(at)) => Some(all.iter().rposition(|(seq, r)| (*seq, r.start) < at).unwrap_or(all.len() - 1)),
            (Some(false), Some(at)) => Some(all.iter().position(|(seq, r)| (*seq, r.start) > at).unwrap_or(0)),
            _ => Some(all.len() - 1),
        };
        let rows = index.and_then(|i| buffer.rows_below(all[i].0, all[i].1.start, width));
        if let Some(Overlay::Find(find)) = &mut self.overlay {
            find.position = index.map_or((0, all.len()), |i| (all.len() - i, all.len()));
            find.current = index.map(|i| all[i].clone());
        }
        if let Some(rows) = rows
            && let Some(b) = self.buffer_mut(id)
        {
            match buffer::scroll_to_show(rows, b.scroll, height) {
                0 => b.scroll_to_bottom(),
                scroll => b.scroll = scroll,
            }
        }
    }

    pub(super) fn on_find_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let Some(Overlay::Find(find)) = &mut self.overlay else { return };
        let before = find.query.text.clone();
        match key.code {
            KeyCode::Esc => return self.close_find(),
            KeyCode::Char('c') if ctrl => return self.close_find(),
            KeyCode::Enter if shift => return self.find_step(Some(false)),
            KeyCode::Enter | KeyCode::Up => return self.find_step(Some(true)),
            KeyCode::Down => return self.find_step(Some(false)),
            KeyCode::PageUp | KeyCode::PageDown => {
                let (w, h) = self.view;
                let page = (h as isize - 2).max(1);
                return self.scroll(if key.code == KeyCode::PageUp { page } else { -page }, w, h);
            }
            KeyCode::Backspace if ctrl || alt => find.query.kill_word(),
            KeyCode::Backspace => find.query.backspace(),
            KeyCode::Delete => find.query.delete(),
            KeyCode::Left => find.query.left(),
            KeyCode::Right => find.query.right(),
            KeyCode::Home => find.query.home(),
            KeyCode::End => find.query.end(),
            KeyCode::Char('u') if ctrl => find.query.kill_to_start(),
            KeyCode::Char('w') if ctrl => find.query.kill_word(),
            KeyCode::Char(c) if !ctrl && !alt => {
                let mut buf = [0; 4];
                find.query.insert(c.encode_utf8(&mut buf));
            }
            _ => {
                // Find Next and Previous step through the matches; another shortcut closes the
                // bar and does what it does.
                match chord(&key).and_then(|c| self.keymap.lookup(Scope::Window, &c)) {
                    Some("find-next") => self.find_step(Some(true)),
                    Some("find-previous") => self.find_step(Some(false)),
                    Some("find") => {}
                    Some(_) => {
                        self.close_find();
                        self.on_key(key);
                    }
                    None => {}
                }
                return;
            }
        }
        if find.query.text != before {
            find.current = None;
            self.find_step(None);
        }
    }

    // ----- Importing -----

    /// `/import PATH`: adds the worlds, characters and rules in a BeipMU Config.txt to ours,
    /// as the desktop apps' Import BeipMU Settings does.
    pub(super) fn import_command(&mut self, args: &str) {
        if args.is_empty() {
            return self.notice("Use /import followed by the path to BeipMU's Config.txt");
        }
        let path = familiar_config::paths::expand(args.trim_matches('"'));
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => return self.notice(&format!("Couldn't read {}: {e}", path.display())),
        };
        // BeipMU writes UTF-8; very old configs may be Windows-1252.
        let text = String::from_utf8(bytes).unwrap_or_else(|e| e.into_bytes().iter().map(|&b| b as char).collect());
        let imported = familiar_import::config::import(&text);
        let summary = familiar_import::config::merge(&mut self.config, &imported);
        let mut report = summary.describe();
        report.push('.');
        if let Some(script) = &imported.startup_script {
            report.push('\n');
            report.push_str(&startup_script(script, &path));
        }
        if !imported.warnings.is_empty() {
            report.push_str(&format!("\nNot carried over or changed ({}):", imported.warnings.len()));
            for w in &imported.warnings {
                report.push_str(&format!("\n  {w}"));
            }
        }
        if self.config_error.is_some() {
            report.push_str("\nThe config file couldn't be read when Familiar started, so this isn't saved.");
        }
        self.notice(&report);
        self.rules_changed();
    }
}

/// Copies BeipMU's startup script to ours when it can be found next to Config.txt (or at its
/// path, if that's one here) and we don't have one yet. Says what happened.
fn startup_script(written: &str, config: &Path) -> String {
    let name = written.rsplit(['\\', '/']).next().unwrap_or(written);
    let candidates = [Some(Path::new(written).to_path_buf()), config.parent().map(|dir| dir.join(name))];
    let found = candidates.into_iter().flatten().find(|p| p.is_file());
    let ours = familiar_config::startup_script_path();
    let shown = familiar_config::paths::display(&ours);
    match found {
        Some(_) if ours.exists() => format!("BeipMU ran {name} at startup. You already have {shown}, so it was left alone; add BeipMU's code there if you want it."),
        Some(source) => {
            let copied = ours.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::copy(&source, &ours));
            match copied {
                Ok(_) => format!("BeipMU's startup script was copied to {shown}; it runs when a world opens."),
                Err(e) => format!("Couldn't copy the startup script {}: {e}", source.display()),
            }
        }
        None => format!("BeipMU ran a startup script ({written}). Copy it to {shown} to use it here."),
    }
}
