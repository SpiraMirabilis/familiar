//! Keyboard, paste and mouse input.

use super::*;

impl App {
    pub fn on_event(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                self.selection = None;
                self.on_key(crate::keys::normalize(key, cfg!(windows)))
            }
            Event::Mouse(mouse) => self.on_mouse(mouse),
            Event::Paste(text) => {
                if let Some(Overlay::World(form)) = &mut self.overlay {
                    form.focused().input.insert(&text.replace(['\r', '\n'], " "));
                } else if let Some(Overlay::Picker(p)) = &mut self.overlay {
                    p.query.insert(&text.replace(['\r', '\n'], " "));
                    self.picker_query_changed();
                } else if let Some(Overlay::Rules(view)) = &mut self.overlay {
                    view.paste(&text);
                } else if let Some(Overlay::Find(find)) = &mut self.overlay {
                    find.query.insert(&text.replace(['\r', '\n'], " "));
                    self.find_step(None);
                } else if self.overlay.is_some() {
                } else if let Some(editor) = self.focused_editor() {
                    editor.term.paste(&text);
                } else if let Some(input) = self.current_input() {
                    input.insert(&text.replace("\r\n", "\n").replace('\r', "\n"));
                }
            }
            Event::FocusGained => self.focused = true,
            Event::FocusLost => self.focused = false,
            _ => {}
        }
    }

    pub(super) fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        if self.overlay.is_some() {
            return self.on_overlay_key(key);
        }
        let pressed = chord(&key);
        let action = pressed.as_ref().and_then(|c| self.keymap.lookup(Scope::Window, c));
        // An editor gets every key but the ones that switch tabs, the one that moves it and
        // (beside a world) the ones that go back to the world.
        if let Some((id, docked)) = self.focused_editor().map(|e| (e.id, e.docked.is_some())) {
            match action.filter(|a| pressed.as_ref().is_some_and(|c| reaches_past_editor(a, c, docked))) {
                Some(action) => self.run(action),
                None => {
                    if let Some(editor) = self.editors.iter_mut().find(|e| e.id == id) {
                        editor.term.key(key);
                    }
                }
            }
            return;
        }
        // Selecting, copying and pasting in the input box, even with text typed (Ctrl+A is
        // otherwise readline's start of the line). With nothing selected, copy and cut do
        // nothing, and Ctrl+C goes on to clear the input below.
        if let Some(action) = action.filter(|a| matches!(*a, "select-all" | "copy" | "cut" | "paste")) {
            let selected = self.current_input().is_some_and(|i| i.selection().is_some());
            if selected || !matches!(action, "copy" | "cut") {
                return self.run(action);
            }
            if !(key.code == KeyCode::Char('c') && ctrl) {
                return;
            }
        }
        // Ctrl+C clears the input, or (pressed twice) quits. It can't be a shortcut, except
        // to copy the selection.
        if key.code == KeyCode::Char('c') && ctrl {
            match self.current_input() {
                Some(input) if !input.is_empty() => {
                    input.take(false);
                }
                _ => self.arm_quit(),
            }
            return;
        }
        // The shortcuts. A readline key that's also one (Ctrl+K: kill to the end of the line, or
        // the command palette) is the shortcut only when there's nothing typed.
        if let Some(action) = action {
            let typed = self.current_input().is_some_and(|i| !i.is_empty());
            if !(typed && pressed.as_ref().is_some_and(is_readline_key)) {
                // Quit from a key works like Ctrl+C on an empty line: press it twice.
                return if action == "quit" { self.arm_quit() } else { self.run(action) };
            }
        }
        if key.code == KeyCode::Char('d') && ctrl && self.current_input().is_none_or(|i| i.is_empty()) {
            return self.arm_quit();
        }
        let Some((idx, puppet)) = self.current() else { return };

        // Keyboard macros, for keys the client doesn't use itself.
        let reserved = matches!(key.code, KeyCode::Char(_)) && !ctrl && !alt
            || matches!(key.code, KeyCode::Enter | KeyCode::Backspace | KeyCode::Delete | KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End | KeyCode::Tab | KeyCode::BackTab | KeyCode::Esc)
            || (ctrl && matches!(key.code, KeyCode::Char('a' | 'e' | 'u' | 'w' | 'y' | 'j' | 'l')))
            || (matches!(key.code, KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown) && !key.state.contains(KeyEventState::KEYPAD));
        if (!reserved || key.state.contains(KeyEventState::KEYPAD))
            && puppet.is_none()
            && let Some(out) = self.run_macro(idx, &key)
        {
            self.sessions[idx].output.scroll_to_bottom();
            self.apply(idx, out);
            return;
        }

        // The / command popup.
        let suggestions = self.suggestions();
        if !suggestions.is_empty() {
            let n = suggestions.len();
            match key.code {
                KeyCode::Up => return self.suggestion = (self.suggestion + n - 1) % n,
                KeyCode::Down => return self.suggestion = (self.suggestion + 1) % n,
                KeyCode::Tab => {
                    let pick = &suggestions[self.suggestion.min(n - 1)];
                    self.sessions[idx].input.set(&pick.insert);
                    self.suggestion = 0;
                    return;
                }
                KeyCode::Enter if !shift && !alt => {
                    let pick = &suggestions[self.suggestion.min(n - 1)];
                    let typed = self.sessions[idx].input.text.trim_end().to_string();
                    // Finish an incomplete command word rather than sending it.
                    if pick.insert.trim_end() != typed && pick.insert.to_lowercase().starts_with(&typed.to_lowercase()) && pick.insert.ends_with(' ') {
                        self.sessions[idx].input.set(&pick.insert);
                        self.suggestion = 0;
                        return;
                    }
                }
                KeyCode::Esc => {
                    self.suggestions_hidden = true;
                    return;
                }
                _ => {}
            }
        }

        // Scrolling.
        let (w, h) = self.view;
        let page = (h as isize - 2).max(1);
        match key.code {
            KeyCode::PageUp => return self.scroll(page, w, h),
            KeyCode::PageDown => return self.scroll(-page, w, h),
            KeyCode::Up if shift => return self.scroll(1, w, h),
            KeyCode::Down if shift => return self.scroll(-1, w, h),
            KeyCode::Home if ctrl => return self.scroll(isize::MAX / 4, w, h),
            KeyCode::End if ctrl => {
                if let Some(b) = self.current_buffer() {
                    b.scroll_to_bottom();
                }
                return;
            }
            KeyCode::Esc => {
                let scrolled = self.current_buffer().is_some_and(|b| b.scroll > 0);
                if scrolled {
                    if let Some(b) = self.current_buffer() {
                        b.scroll_to_bottom();
                    }
                    return;
                }
                // Esc twice clears the input.
                if self.esc_at.is_some_and(|t| t.elapsed() < Duration::from_millis(600)) {
                    if let Some(i) = self.current_input() {
                        i.take(false);
                    }
                    self.esc_at = None;
                } else {
                    self.esc_at = Some(Instant::now());
                }
                return;
            }
            _ => {}
        }

        if key.code == KeyCode::Tab {
            // Complete a word from recent output (names, mostly), cycling on repeated presses.
            let s = &mut self.sessions[idx];
            let (word, words) = match &puppet {
                Some(name) => {
                    let p = s.puppet_mut(name);
                    let word = p.input.current_word().1.to_string();
                    let words = p.buffer.words_starting(&word, 400);
                    (word, words)
                }
                None => {
                    let word = s.input.current_word().1.to_string();
                    (word.clone(), s.output.words_starting(&word, 400))
                }
            };
            let input = match &puppet {
                Some(name) => &mut s.puppet_mut(name).input,
                None => &mut s.input,
            };
            if !input.complete(|_| words) && word.is_empty() {
                input.insert("    ");
            }
            return;
        }
        let session = &mut self.sessions[idx];
        let input = match &puppet {
            Some(name) => &mut session.puppet_mut(name).input,
            None => &mut session.input,
        };
        self.suggestions_hidden = self.suggestions_hidden && !input.text.is_empty();
        let before = input.text.clone();
        match key.code {
            KeyCode::Enter if shift || alt => input.insert("\n"),
            KeyCode::Char('j') if ctrl => input.insert("\n"),
            KeyCode::Enter => return self.submit(),
            KeyCode::Backspace if ctrl || alt => input.kill_word(),
            KeyCode::Backspace => input.backspace(),
            KeyCode::Delete => input.delete(),
            KeyCode::Left if ctrl && shift => input.select(Input::word_left),
            KeyCode::Right if ctrl && shift => input.select(Input::word_right),
            KeyCode::Left if shift => input.select(Input::left),
            KeyCode::Right if shift => input.select(Input::right),
            KeyCode::Home if shift => input.select(Input::home),
            KeyCode::End if shift => input.select(Input::end),
            KeyCode::Left if ctrl => input.word_left(),
            KeyCode::Right if ctrl => input.word_right(),
            KeyCode::Left => input.left(),
            KeyCode::Right => input.right(),
            KeyCode::Char('b') if alt => input.word_left(),
            KeyCode::Char('f') if alt => input.word_right(),
            KeyCode::Char('d') if alt => input.kill_word_forward(),
            KeyCode::Home => input.home(),
            KeyCode::End => input.end(),
            KeyCode::Char('a') if ctrl => input.home(),
            KeyCode::Char('e') if ctrl => input.end(),
            KeyCode::Char('u') if ctrl => input.kill_to_start(),
            KeyCode::Char('k') if ctrl => input.kill_to_end(),
            KeyCode::Char('w') if ctrl => input.kill_word(),
            KeyCode::Char('y') if ctrl => input.yank(),
            KeyCode::Char('l') if ctrl => {}
            KeyCode::Up => {
                if !input.line_up() {
                    input.history_older();
                }
            }
            KeyCode::Down => {
                if !input.line_down() {
                    input.history_newer();
                }
            }
            KeyCode::Char(c) if !ctrl => {
                let mut buf = [0; 4];
                input.insert(c.encode_utf8(&mut buf));
            }
            _ => {}
        }
        if input.text != before {
            self.suggestion = 0;
            if input.text.is_empty() {
                self.suggestions_hidden = false;
            }
        }
    }

    pub(super) fn arm_quit(&mut self) {
        if self.quit_armed.is_some() {
            self.quit = true;
        } else {
            self.quit_armed = Some(Instant::now());
        }
    }

    pub(super) fn scroll(&mut self, delta: isize, w: u16, h: u16) {
        if let Some(b) = self.current_buffer() {
            b.scroll_by(delta, w, h);
        }
    }

    pub(super) fn on_overlay_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if matches!(self.overlay, Some(Overlay::Help)) {
            self.overlay = None;
            return;
        }
        if matches!(self.overlay, Some(Overlay::World(_))) {
            return self.on_form_key(key);
        }
        if matches!(self.overlay, Some(Overlay::Rules(_))) {
            return self.on_rules_key(key);
        }
        if matches!(self.overlay, Some(Overlay::Keys(_))) {
            return self.on_keys_key(key);
        }
        if matches!(self.overlay, Some(Overlay::Find(_))) {
            return self.on_find_key(key);
        }
        // In the world picker: Ctrl+N adds a world, Ctrl+E edits the selected one and Ctrl+D
        // (twice) deletes it.
        if let Some(Overlay::Picker(p)) = &self.overlay
            && p.kind == PickerKind::Connect
            && ctrl
        {
            let selected = p.matches.get(p.selected).map(|(i, _)| &p.items[*i]);
            let saved = selected.and_then(|item| match &item.action {
                Action::Connect { world, character } if item.group == "Saved worlds" => Some((world.name.clone(), character.clone())),
                _ => None,
            });
            match key.code {
                KeyCode::Char('n') => {
                    // Start from an address typed into the filter, if there is one.
                    let typed = parse_address(p.query.text.trim());
                    let mut form = WorldForm::new(None, None, false);
                    if let Some(w) = typed {
                        form.fields[crate::form::HOST].input.set(&w.host);
                        form.fields[crate::form::PORT].input.set(&w.port.to_string());
                        form.fields[crate::form::TLS].on = w.tls;
                    }
                    self.overlay = Some(Overlay::World(Box::new(form)));
                    return;
                }
                KeyCode::Char('e') => {
                    if let Some((name, character)) = saved
                        && let Some(w) = self.config.world(&name).cloned()
                    {
                        self.overlay = Some(Overlay::World(Box::new(WorldForm::new(Some(&w), character.as_deref(), true))));
                    }
                    return;
                }
                KeyCode::Char('d') => {
                    if let Some((name, _)) = saved {
                        if self.delete_armed.as_deref() == Some(name.as_str()) {
                            self.config.worlds.retain(|w| w.name != name);
                            self.save_config();
                            self.delete_armed = None;
                            self.toast(&format!("Deleted {name}"));
                            self.open_connect_picker();
                        } else {
                            self.toast(&format!("Press Ctrl+D again to delete {name}"));
                            self.delete_armed = Some(name);
                        }
                    }
                    return;
                }
                _ => {}
            }
        }
        let Some(Overlay::Picker(p)) = &mut self.overlay else { return };
        let n = p.matches.len().max(1);
        match key.code {
            KeyCode::Esc => {
                if self.tabs.is_empty() {
                    // Nothing behind the picker: Esc clears the filter instead.
                    p.query.take(false);
                    self.picker_query_changed();
                } else {
                    self.overlay = None;
                }
            }
            KeyCode::Char('c') if ctrl => {
                if self.tabs.is_empty() {
                    self.arm_quit();
                } else {
                    self.overlay = None;
                }
            }
            KeyCode::Enter => self.run_picker(),
            KeyCode::Up => p.selected = (p.selected + n - 1) % n,
            KeyCode::Char('p') if ctrl => p.selected = (p.selected + n - 1) % n,
            KeyCode::Down | KeyCode::Tab => p.selected = (p.selected + 1) % n,
            KeyCode::Char('n') if ctrl => p.selected = (p.selected + 1) % n,
            KeyCode::PageUp => p.selected = p.selected.saturating_sub(8),
            KeyCode::PageDown => p.selected = (p.selected + 8).min(n - 1),
            KeyCode::Backspace => {
                p.query.backspace();
                self.picker_query_changed();
            }
            KeyCode::Char('u') if ctrl => {
                p.query.take(false);
                self.picker_query_changed();
            }
            KeyCode::Char('w') if ctrl => {
                p.query.kill_word();
                self.picker_query_changed();
            }
            KeyCode::Left => p.query.left(),
            KeyCode::Right => p.query.right(),
            KeyCode::Char(c) if !ctrl => {
                let mut buf = [0; 4];
                p.query.insert(c.encode_utf8(&mut buf));
                self.picker_query_changed();
            }
            _ => {}
        }
    }

    pub(super) fn on_form_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(Overlay::World(form)) = &mut self.overlay else { return };
        form.error = None;
        match key.code {
            KeyCode::Esc => {
                // Back to the picker if that's where the form came from (or nothing's open).
                self.overlay = None;
                if self.tabs.is_empty() {
                    self.open_connect_picker();
                }
            }
            KeyCode::Tab | KeyCode::Down => form.next(),
            KeyCode::BackTab | KeyCode::Up => form.prev(),
            KeyCode::Enter | KeyCode::Char('s') if key.code == KeyCode::Enter || ctrl => {
                let connect = key.code == KeyCode::Enter;
                let Some(Overlay::World(mut form)) = self.overlay.take() else { return };
                match form.save(&mut self.config) {
                    Ok((world, character)) => {
                        self.save_config();
                        self.toast(&format!("Saved {}", world.name));
                        // Open tabs on this world pick up the new character and login.
                        for idx in 0..self.sessions.len() {
                            let out = self.sessions[idx].engine.reload(&self.config);
                            self.apply(idx, out);
                        }
                        if connect && !self.sessions.iter().any(|s| s.engine.world().name == world.name && s.engine.is_connected()) {
                            self.open_session(world, character);
                        } else if self.tabs.is_empty() {
                            self.open_connect_picker();
                        }
                    }
                    Err(e) => {
                        form.error = Some(e);
                        self.overlay = Some(Overlay::World(form));
                    }
                }
            }
            KeyCode::Char(' ') if form.fields[form.focus].kind == FieldKind::Toggle => {
                let f = form.focused();
                f.on = !f.on;
            }
            _ if form.fields[form.focus].kind == FieldKind::Toggle => {}
            KeyCode::Backspace => form.focused().input.backspace(),
            KeyCode::Delete => form.focused().input.delete(),
            KeyCode::Left => form.focused().input.left(),
            KeyCode::Right => form.focused().input.right(),
            KeyCode::Home => form.focused().input.home(),
            KeyCode::End => form.focused().input.end(),
            KeyCode::Char('u') if ctrl => form.focused().input.kill_to_start(),
            KeyCode::Char('w') if ctrl => form.focused().input.kill_word(),
            KeyCode::Char(c) if !ctrl => {
                let mut buf = [0; 4];
                form.focused().input.insert(c.encode_utf8(&mut buf));
            }
            _ => {}
        }
    }

    pub(super) fn submit(&mut self) {
        let Some((idx, puppet)) = self.current() else { return };
        self.sessions[idx].output.scroll_to_bottom();
        let secret = self.sessions[idx].password && puppet.is_none();
        let input = match &puppet {
            Some(name) => &mut self.sessions[idx].puppet_mut(name).input,
            None => &mut self.sessions[idx].input,
        };
        let remember = !secret && !input.text.trim().is_empty();
        let text = input.take(remember);
        self.suggestion = 0;
        self.suggestions_hidden = false;
        if let Some(name) = puppet {
            let out = self.sessions[idx].engine.puppet_submit(&name, &text);
            self.sessions[idx].puppet_mut(&name).buffer.scroll_to_bottom();
            return self.apply(idx, out);
        }
        if text.is_empty() && matches!(self.sessions[idx].status, Status::Disconnected(_)) {
            return self.connect(idx);
        }
        if !secret && self.client_command(&text) {
            return;
        }
        let out = self.sessions[idx].engine.submit(&text, secret);
        self.apply(idx, out);
    }

    /// Commands the terminal client handles itself. Returns true if `text` was one.
    pub(super) fn client_command(&mut self, text: &str) -> bool {
        let Some(command) = text.strip_prefix('/') else { return false };
        let (name, args) = command.split_once(' ').unwrap_or((command, ""));
        let args = args.trim();
        match name.to_lowercase().as_str() {
            "connect" if args.is_empty() => self.open_connect_picker(),
            "connect" => self.connect_command(args),
            "close" => self.close_tab(),
            "quit" | "exit" => self.quit = true,
            "sidebar" => self.run("toggle-sidebar"),
            "world" | "worlds" => self.run("edit-world"),
            "config" => self.run("preferences"),
            "rules" | "triggers" | "aliases" | "macros" => self.run("rules"),
            "import" => self.import_command(args),
            "agent" | "agents" => self.agent_command(args),
            "editor" => {
                let settings = &mut self.config.settings;
                match args.to_lowercase().as_str() {
                    "tab" => (settings.editor_in_tab, settings.editor_in_sidebar) = (true, false),
                    "sidebar" | "side" => (settings.editor_in_tab, settings.editor_in_sidebar) = (true, true),
                    "fullscreen" | "full" => (settings.editor_in_tab, settings.editor_in_sidebar) = (false, false),
                    "move" => {
                        self.move_editor();
                        return true;
                    }
                    "syntax" | "syntax force" => {
                        self.install_syntax(args.to_lowercase().ends_with("force"));
                        return true;
                    }
                    "" => {}
                    _ => {
                        self.toast("Use /editor tab, /editor sidebar or /editor fullscreen (/editor move moves the one that's open, /editor syntax installs MOO highlighting)");
                        return true;
                    }
                }
                if !args.is_empty() {
                    self.save_config();
                }
                let settings = &self.config.settings;
                self.toast(match (settings.editor_in_tab, settings.editor_in_sidebar) {
                    (false, _) => "$EDITOR takes over the whole terminal",
                    (true, false) => "$EDITOR opens in a tab",
                    (true, true) => "$EDITOR opens beside the world when its sidebar is free, otherwise in a tab",
                });
            }
            "colors" | "colours" => {
                match args.to_lowercase().as_str() {
                    "terminal" | "theme" => self.config.settings.terminal_colors = true,
                    "standard" | "exact" => self.config.settings.terminal_colors = false,
                    "" => {}
                    _ => {
                        self.toast("Use /colors standard or /colors terminal");
                        return true;
                    }
                }
                let themed = self.config.settings.terminal_colors;
                if !args.is_empty() {
                    crate::theme::set_terminal_colors(themed);
                    self.save_config();
                }
                self.toast(if themed { "Basic colours: the terminal's theme" } else { "Basic colours: standard" });
            }
            "keys" | "shortcuts" => self.run("keyboard"),
            _ => return false,
        }
        true
    }

    /// `/connect WORLD [CHARACTER]` (world names may have spaces) or `/connect host:port`.
    pub fn connect_command(&mut self, args: &str) {
        let args = args.trim();
        let lower = args.to_lowercase();
        let found = self
            .config
            .worlds
            .iter()
            .map(|w| w.name.clone())
            .filter(|n| lower == n.to_lowercase() || lower.starts_with(&format!("{} ", n.to_lowercase())))
            .max_by_key(String::len);
        match found {
            Some(world) => {
                let character = args[world.len()..].trim();
                self.open_saved(&world, (!character.is_empty()).then_some(character));
            }
            None => match parse_address(args) {
                Some(world) => self.open_session(world, None),
                None => self.toast(&format!("No saved world called {args}")),
            },
        }
    }

    pub(super) fn on_mouse(&mut self, mouse: MouseEvent) {
        let at = Position::new(mouse.column, mouse.row);
        if let Some(Overlay::Rules(view)) = &mut self.overlay {
            // The wheel moves through the list.
            if view.form.is_none() && matches!(mouse.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) {
                let count = view.rows(&self.config).len();
                view.select(if mouse.kind == MouseEventKind::ScrollUp { -1 } else { 1 }, count);
            }
            return;
        }
        if let Some(Overlay::Keys(view)) = &mut self.overlay {
            if !view.recording && matches!(mouse.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) {
                let count = self.keymap.describe(&self.config).len();
                view.select(if mouse.kind == MouseEventKind::ScrollUp { -1 } else { 1 }, count);
            }
            return;
        }
        // Over an editor's screen, the mouse is the editor's. Clicking chooses between an
        // editor beside a world and the world.
        let area = self.hits.editor;
        if self.overlay.is_none()
            && let Some(editor) = self.visible_editor()
        {
            let inside = area.contains(at);
            if editor.docked.is_some() && matches!(mouse.kind, MouseEventKind::Down(_)) {
                editor.focused = inside;
            }
            if inside {
                return editor.term.mouse(mouse, at.x - area.x, at.y - area.y);
            }
        }
        match mouse.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let delta = if mouse.kind == MouseEventKind::ScrollUp { 3 } else { -3 };
                if let Some(Overlay::Picker(p)) = &mut self.overlay {
                    let n = p.matches.len().max(1);
                    p.selected = if delta > 0 { p.selected.saturating_sub(1) } else { (p.selected + 1).min(n - 1) };
                } else if self.hits.sidebar.contains(at) {
                    if let Some((i, _)) = self.current() {
                        let s = &mut self.sessions[i];
                        if let Some(Panel::Spawn(name)) = s.panels().get(s.panel).cloned() {
                            let (w, h) = (self.hits.sidebar.width.saturating_sub(2), self.hits.sidebar.height.saturating_sub(2));
                            s.spawn_mut(&name).0.buffer.scroll_by(delta, w, h);
                        }
                    }
                } else {
                    let (w, h) = self.view;
                    self.scroll(delta, w, h);
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(Overlay::Picker(p)) = &mut self.overlay {
                    if let Some((_, i)) = self.hits.picker_items.iter().find(|(r, _)| r.contains(at)) {
                        p.selected = *i;
                        self.run_picker();
                    }
                    return;
                }
                if matches!(self.overlay, Some(Overlay::Help)) {
                    self.overlay = None;
                    return;
                }
                if let Some(&(_, t)) = self.hits.tabs.iter().find(|(r, _)| r.contains(at)) {
                    return self.switch_to(t);
                }
                if let Some(&(_, p)) = self.hits.panels.iter().find(|(r, _)| r.contains(at)) {
                    if let Some((i, _)) = self.current() {
                        self.sessions[i].panel = p;
                    }
                    return;
                }
                // On text: the start of a selection if the mouse moves, a click (on a link) if not.
                self.selection = None;
                self.drag = None;
                self.press = self
                    .hits
                    .rows
                    .iter()
                    .find(|r| r.y == at.y && (r.x..r.x + r.width).contains(&at.x))
                    .and_then(|r| Some(Press { buffer: r.buffer, cell: self.cell_at(r, at)?, live: r.live }));
                if self.press.is_some() {
                    return;
                }
                if let Some((rect, (x0, y0, x1, y1), area)) = self.hits.map.clone()
                    && rect.contains(at)
                    && let Some((i, _)) = self.current()
                {
                    // Walk to the clicked room.
                    let fx = (at.x - rect.x) as f64 + 0.5;
                    let fy = (at.y - rect.y) as f64 + 0.5;
                    let x = x0 + fx / rect.width as f64 * (x1 - x0);
                    let y = y0 + fy / rect.height as f64 * (y1 - y0);
                    let room = self.sessions[i].engine.map().room_at(&area, x, y);
                    if let Some(room) = room {
                        let out = self.sessions[i].engine.map_walk_to(room);
                        self.apply(i, out);
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Middle) if self.overlay.is_none() => {
                // A way to close an editor that has stopped answering.
                if let Some(&(_, t)) = self.hits.tabs.iter().find(|(r, _)| r.contains(at))
                    && matches!(self.tabs[t], Tab::Editor(_))
                {
                    self.switch_to(t);
                    self.close_tab();
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => self.drag_to(at),
            MouseEventKind::Up(MouseButton::Left) => {
                self.drag = None;
                if self.press.take().is_none() {
                    return;
                }
                if let Some(selection) = self.selection {
                    let text = self.buffer(selection.buffer).map(|b| b.selected_text(&selection)).unwrap_or_default();
                    if text.is_empty() {
                        self.selection = None;
                    } else {
                        self.clipboard = Some(text);
                        self.toast("Copied");
                    }
                } else if let Some(uri) = self.hits.links.iter().find(|(y, x0, x1, _)| *y == at.y && (*x0..*x1).contains(&at.x)).map(|l| l.3.clone())
                    && let Some((i, _)) = self.current()
                {
                    let out = self.sessions[i].engine.follow_link(&uri);
                    self.apply(i, out);
                }
            }
            _ => {}
        }
    }

    /// Extends the selection to the mouse, and scrolls when the mouse is at (or past) the top
    /// or bottom of the part of the output that scrolls, so the selection can go on from there.
    /// Called for every movement, and on a timer while the button is held.
    pub(super) fn drag_to(&mut self, at: Position) {
        let Some(press) = self.press else { return };
        // The nearest row of the same pane, so dragging past its edges still selects. The live
        // tail under a scrolled-back view shows the same buffer but is a pane of its own: a
        // drag doesn't cross from one to the other, which would select everything between.
        let row = self.hits.rows.iter().filter(|r| r.buffer == press.buffer && r.live == press.live).min_by_key(|r| r.y.abs_diff(at.y));
        if let Some(head) = row.and_then(|r| self.cell_at(r, at)) {
            self.selection = (head != press.cell).then_some(Selection { buffer: press.buffer, anchor: press.cell, head });
        }
        let last = self.drag.map(|(_, scrolled)| scrolled);
        let mut scrolled = last.unwrap_or_else(|| Instant::now() - AUTOSCROLL);
        if !press.live
            && let Some((id, pane)) = self.hits.scrolling
            && id == press.buffer
            && scrolled.elapsed() >= AUTOSCROLL
        {
            let bottom = pane.y + pane.height.saturating_sub(1);
            // Faster the further past the edge the mouse is, up to three rows at a time.
            let rows = if at.y <= pane.y {
                (1 + (pane.y - at.y) as isize).min(3)
            } else if at.y >= bottom {
                -(1 + (at.y - bottom) as isize).min(3)
            } else {
                0
            };
            let (w, h) = self.view;
            if rows != 0
                && let Some(buffer) = self.buffer_mut(press.buffer)
            {
                buffer.scroll_by(rows, w, h);
                scrolled = Instant::now();
            }
        }
        self.drag = Some((at, scrolled));
    }

    pub(super) fn buffer_mut(&mut self, id: u64) -> Option<&mut Buffer> {
        self.sessions
            .iter_mut()
            .flat_map(|s| std::iter::once(&mut s.output).chain(s.puppets.iter_mut().map(|p| &mut p.buffer)).chain(s.spawns.iter_mut().map(|p| &mut p.buffer)))
            .find(|b| b.id == id)
    }

    pub(super) fn buffer(&self, id: u64) -> Option<&Buffer> {
        self.sessions.iter().flat_map(|s| std::iter::once(&s.output).chain(s.puppets.iter().map(|p| &p.buffer)).chain(s.spawns.iter().map(|p| &p.buffer))).find(|b| b.id == id)
    }

    /// The buffer cell under (or, for a position off the row's ends, nearest) the mouse.
    fn cell_at(&self, row: &RowHit, at: Position) -> Option<Cell> {
        let entry = self.buffer(row.buffer)?.entry(row.seq)?;
        // Past the right edge is past the end of the row.
        let col = if at.x >= row.x + row.width { usize::MAX } else { at.x.saturating_sub(row.x) as usize };
        Some(entry.cell_at(row.width, row.row, col))
    }
}

/// "host:port", "host port" or "host:port tls" as a new world. Stricter than the GTK client's
/// address box, because here the same text is also a search of the saved worlds: there must be
/// a port, and the host has to look like one.
pub(super) fn parse_address(text: &str) -> Option<World> {
    let address = familiar_config::Address::parse(text)?;
    address.port?;
    (address.host.contains(['.', ':']) || address.host == "localhost").then(|| address.into_world(0))
}

/// Whether a shortcut reaches Familiar from an editor running in a tab or beside a world, which
/// gets every other key: switching tabs, moving the editor, and (beside a world) going back to
/// the world. Alt+B and Alt+F stay the editor's even where they switch tabs (Option+← and → on
/// a Mac send them): Esc then b typed quickly in Vim arrives as Alt+B.
pub(super) fn reaches_past_editor(action: &str, chord: &KeyChord, docked: bool) -> bool {
    let word_movement = chord.alt && !chord.ctrl && !chord.shift && matches!(chord.key.as_str(), "B" | "F");
    let allowed = matches!(action, "next-tab" | "prev-tab" | "move-editor") || action.starts_with("tab-") || (action == "editor-focus" && docked);
    allowed && !word_movement
}

/// The readline keys the input box has (Ctrl+A, E, U, K, W, Y). As a shortcut, one of these
/// works only while nothing is typed.
pub(super) fn is_readline_key(chord: &KeyChord) -> bool {
    chord.ctrl && !chord.alt && !chord.shift && !chord.cmd && matches!(chord.key.as_str(), "A" | "E" | "U" | "K" | "W" | "Y")
}

impl App {
    /// Runs the keyboard macro on this key, if there is one. Macros recorded before punctuation
    /// keys had GDK's names ("Alt+]" rather than "Alt+bracketright") still work.
    fn run_macro(&mut self, idx: usize, key: &KeyEvent) -> Option<Vec<Output>> {
        let pressed = chord(key)?;
        let engine = &mut self.sessions[idx].engine;
        if let Some(out) = engine.run_macro(&pressed) {
            return Some(out);
        }
        match key.code {
            KeyCode::Char(c) if crate::keys::punctuation_name(c).is_some() => engine.run_macro(&KeyChord { key: c.to_string(), ..pressed }),
            _ => None,
        }
    }
}

/// A key as a shortcut or macro chord, in the GTK client's naming (GDK key names).
pub(super) fn chord(key: &KeyEvent) -> Option<KeyChord> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let mut shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let keypad = key.state.contains(KeyEventState::KEYPAD);
    let name = match key.code {
        KeyCode::F(n) => format!("F{n}"),
        KeyCode::Char(c) if keypad => match c {
            '0'..='9' => format!("KP_{c}"),
            '+' => "KP_Add".into(),
            '-' => "KP_Subtract".into(),
            '*' => "KP_Multiply".into(),
            '/' => "KP_Divide".into(),
            '.' => "KP_Decimal".into(),
            _ => return None,
        },
        KeyCode::Char(c) => {
            // Shifted characters already say so.
            shift &= ctrl || alt;
            crate::keys::punctuation_name(c).map_or_else(|| c.to_uppercase().to_string(), str::to_string)
        }
        KeyCode::Enter if keypad => "KP_Enter".into(),
        KeyCode::Up if keypad => "KP_8".into(),
        KeyCode::Down if keypad => "KP_2".into(),
        KeyCode::Left if keypad => "KP_4".into(),
        KeyCode::Right if keypad => "KP_6".into(),
        KeyCode::Home if keypad => "KP_7".into(),
        KeyCode::End if keypad => "KP_1".into(),
        KeyCode::PageUp if keypad => "KP_9".into(),
        KeyCode::PageDown if keypad => "KP_3".into(),
        KeyCode::Up => "Up".into(),
        KeyCode::Down => "Down".into(),
        KeyCode::Left => "Left".into(),
        KeyCode::Right => "Right".into(),
        KeyCode::PageUp => "Page_Up".into(),
        KeyCode::PageDown => "Page_Down".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::Insert => "Insert".into(),
        KeyCode::Delete => "Delete".into(),
        KeyCode::Enter => "Return".into(),
        KeyCode::Esc => "Escape".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::Backspace => "BackSpace".into(),
        _ => return None,
    };
    Some(KeyChord::new(&name, ctrl, alt, shift))
}
