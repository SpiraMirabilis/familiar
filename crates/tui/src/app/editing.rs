//! `$EDITOR`: in a tab of its own, beside the world it was opened from (where the sidebar
//! goes), or (the main loop's job) with the whole terminal.

use super::*;

/// The narrowest terminal an editor can sit beside a world in. The same as for the sidebar.
pub const DOCK_MIN_WIDTH: u16 = 90;

/// How wide an editor beside a world is, border included, in a terminal `width` wide: 80
/// columns when the world can keep 40, and never less than half.
pub fn dock_width(width: u16) -> u16 {
    width.saturating_sub(40).min(80).max(width / 2)
}

impl App {
    /// Opens `$EDITOR` for `job`: in a tab, or by handing it the terminal.
    pub(super) fn edit(&mut self, job: External) {
        if self.config.settings.editor_in_tab {
            match self.open_editor_tab(&job) {
                Ok(()) => return,
                Err(e) => self.toast(&format!("Couldn't open the editor in a tab ({e}), so it gets the whole terminal")),
            }
        }
        self.external = Some(job);
    }

    /// The file to edit for `job`, a title for it, and the line to open it at.
    pub fn document(&mut self, job: &External) -> Result<(Document, String, Option<u32>), String> {
        match job {
            External::Edit { request, line, .. } => {
                let extension = match request.language.as_deref() {
                    Some(lang) if !lang.is_empty() => lang,
                    _ => "txt",
                };
                Ok((Document::temporary(&request.name, extension, &request.content)?, request.name.clone(), *line))
            }
            External::EditInput { session, puppet } => {
                let text = self.input_of(*session, puppet.as_deref()).map(|i| i.text.clone()).ok_or("that tab has closed")?;
                let name = match puppet {
                    Some(name) => format!("input-{session}-{name}"),
                    None => format!("input-{session}"),
                };
                Ok((Document::temporary(&name, "txt", &text)?, "input".into(), None))
            }
            External::EditConfig => Ok((Document::file(&self.config_path), "config".into(), None)),
            External::Agent { .. } => Err("an agent has nothing to edit".into()),
        }
    }

    fn input_of(&mut self, session: u64, puppet: Option<&str>) -> Option<&mut Input> {
        let idx = self.session_index(session)?;
        let s = &mut self.sessions[idx];
        Some(match puppet {
            Some(name) => &mut s.puppet_mut(name).input,
            None => &mut s.input,
        })
    }

    /// The editor on screen: the active tab's, or the one beside the active world.
    pub fn visible_editor(&mut self) -> Option<&mut EditorTab> {
        match self.tabs.get(self.active)? {
            Tab::Editor(id) => self.editors.iter_mut().find(|e| e.id == *id),
            Tab::Main(session) => self.editors.iter_mut().find(|e| e.docked == Some(*session)),
            Tab::Puppet(..) => None,
        }
    }

    /// The editor the keyboard is going to, if it's going to one.
    pub fn focused_editor(&mut self) -> Option<&mut EditorTab> {
        self.visible_editor().filter(|e| e.docked.is_none() || e.focused)
    }

    /// Moves the keyboard between a world and the editor beside it.
    pub(super) fn toggle_editor_focus(&mut self) {
        match self.visible_editor().filter(|e| e.docked.is_some()) {
            Some(editor) => editor.focused = !editor.focused,
            None => self.toast("There's no editor beside this world (/editor sidebar puts them there)"),
        }
    }

    /// The session an edit can sit beside, or why it can't sit beside one.
    fn dock_for(&self, job: &External) -> Result<u64, &'static str> {
        let (External::Edit { session, .. } | External::EditInput { session, puppet: None } | External::Agent { session, .. }) = job else {
            return Err("it doesn't belong to a world's main tab");
        };
        let Some(s) = self.session_index(*session).map(|i| &self.sessions[i]) else { return Err("its world has closed") };
        if !s.panels().is_empty() {
            return Err("that world's sidebar is in use");
        }
        if self.editors.iter().any(|e| e.docked == Some(*session)) {
            return Err("another editor is already there");
        }
        if self.screen.0 < DOCK_MIN_WIDTH {
            return Err("the terminal is too narrow");
        }
        Ok(*session)
    }

    /// Moves the editor on screen between a tab of its own and beside its world. From a
    /// world's tab with nothing beside it, that's one of the world's editor tabs.
    pub(super) fn move_editor(&mut self) {
        let here = match self.tabs.get(self.active) {
            Some(Tab::Main(session)) => Some(*session),
            _ => None,
        };
        let in_a_tab = |e: &&EditorTab| e.docked.is_none() && matches!(&e.job, External::Edit { session, .. } | External::EditInput { session, puppet: None } | External::Agent { session, .. } if Some(*session) == here);
        let found = self.visible_editor().map(|e| (e.id, e.docked.is_some())).or_else(|| self.editors.iter().find(in_a_tab).map(|e| (e.id, false)));
        let Some((id, docked)) = found else {
            return self.toast("This world has no editor open to move");
        };
        if docked {
            return self.undock(id, true);
        }
        let Some(job) = self.editors.iter().find(|e| e.id == id).map(|e| e.job.clone()) else { return };
        match self.dock_for(&job) {
            Ok(session) => {
                if let Some(pos) = self.tabs.iter().position(|t| *t == Tab::Editor(id)) {
                    self.tabs.remove(pos);
                }
                if let Some(editor) = self.editors.iter_mut().find(|e| e.id == id) {
                    editor.docked = Some(session);
                    editor.focused = true;
                }
                if let Some(owner) = self.owner_tab(&job) {
                    self.switch_to(owner);
                }
            }
            Err(why) => self.toast(&format!("The editor stays in its tab: {why}")),
        }
    }

    /// Gives an editor beside a world a tab of its own instead.
    pub fn undock(&mut self, id: u64, switch: bool) {
        let Some(editor) = self.editors.iter_mut().find(|e| e.id == id) else { return };
        if editor.docked.take().is_none() {
            return;
        }
        let job = editor.job.clone();
        let pos = self.owner_tab(&job).map_or(self.tabs.len(), |owner| owner + 1);
        self.tabs.insert(pos, Tab::Editor(id));
        if switch {
            self.switch_to(pos);
        } else if self.active >= pos {
            self.active += 1;
        }
    }

    fn open_editor_tab(&mut self, job: &External) -> Result<(), String> {
        let (document, title, line) = self.document(job)?;
        // Already being edited: go there instead of opening the same file twice.
        if let Some(open) = self.editors.iter_mut().find(|e| e.document.as_ref().is_some_and(|d| d.path == document.path)) {
            open.focused = true;
            let tab = if open.docked.is_some() { self.owner_tab(job) } else { self.tabs.iter().position(|t| *t == Tab::Editor(open.id)) };
            if let Some(pos) = tab {
                self.switch_to(pos);
            }
            return Ok(());
        }
        let (command, name) = crate::editor::command(&document.path, line);
        let dock = self.config.settings.editor_in_sidebar;
        match self.open_pane(job, &command, title, Some(document), None, dock) {
            Ok(()) => Ok(()),
            Err((e, document)) => {
                if let Some(document) = document {
                    document.discard();
                }
                Err(format!("{name}: {e}"))
            }
        }
    }

    /// Runs `command` in a terminal for `job`, beside the tab it belongs to if `dock` and
    /// there's room, otherwise in a tab of its own. Gives the document back if it can't.
    pub(super) fn open_pane(
        &mut self,
        job: &External,
        command: &std::process::Command,
        title: String,
        document: Option<Document>,
        agent: Option<String>,
        dock: bool,
    ) -> Result<(), (String, Option<Document>)> {
        let id = self.next_id;
        let dock = if dock { self.dock_for(job).ok().filter(|_| self.owner_tab(job).is_some()) } else { None };
        // Roughly what it will get; it's given its exact size when it's first drawn.
        let size = match dock {
            Some(_) => (dock_width(self.screen.0).saturating_sub(1), self.screen.1.saturating_sub(6)),
            // Everything but the tab bar and the status line.
            None => (self.screen.0, self.screen.1.saturating_sub(2)),
        };
        let term = match Term::spawn(command, size, id, self.pty_tx.clone()) {
            Ok(term) => term,
            Err(e) => return Err((e, document)),
        };
        self.next_id += 1;
        self.editors.push(EditorTab { id, title, term, job: job.clone(), document, agent, docked: dock, focused: true });
        self.overlay = None;
        // Beside the tab it belongs to: inside it, or as the next tab.
        let owner = self.owner_tab(job).unwrap_or(self.active);
        if dock.is_some() {
            self.switch_to(owner);
        } else {
            let pos = (owner + 1).min(self.tabs.len());
            self.tabs.insert(pos, Tab::Editor(id));
            self.switch_to(pos);
        }
        Ok(())
    }

    /// The tab an edit was started from.
    fn owner_tab(&self, job: &External) -> Option<usize> {
        let tab = match job {
            External::Edit { session, .. } | External::EditInput { session, puppet: None } | External::Agent { session, .. } => Tab::Main(*session),
            External::EditInput { session, puppet: Some(name) } => Tab::Puppet(*session, name.clone()),
            External::EditConfig => return None,
        };
        self.tabs.iter().position(|t| *t == tab)
    }

    /// Takes an editor and its tab away, keeping the same tab active where it can.
    pub(super) fn remove_editor(&mut self, id: u64) -> Option<EditorTab> {
        let editor = self.editors.remove(self.editors.iter().position(|e| e.id == id)?);
        if let Some(token) = &editor.agent {
            self.agents.close(token);
        }
        if let Some(pos) = self.tabs.iter().position(|t| *t == Tab::Editor(id)) {
            self.tabs.remove(pos);
            if self.active == pos {
                // Back to where the edit was started.
                self.active = self.owner_tab(&editor.job).unwrap_or(pos.saturating_sub(1));
            } else if self.active > pos {
                self.active -= 1;
            }
        }
        Some(editor)
    }

    pub fn on_pty(&mut self, (id, event): (u64, PtyEvent)) {
        match event {
            PtyEvent::Output(bytes) => {
                if let Some(editor) = self.editors.iter_mut().find(|e| e.id == id) {
                    editor.term.feed(&bytes);
                }
            }
            PtyEvent::Exited(ok) => {
                let Some(editor) = self.remove_editor(id) else { return };
                match editor.document {
                    Some(document) => {
                        let result = if ok {
                            document.finish()
                        } else {
                            document.discard();
                            Err(format!("The editor on {} exited with an error, so nothing was saved", editor.title))
                        };
                        self.finish_edit(editor.job, result);
                    }
                    None if ok => self.toast(&format!("{} finished", editor.title)),
                    None => self.toast(&format!("{} exited with an error", editor.title)),
                }
                if self.tabs.is_empty() {
                    self.open_connect_picker();
                }
                self.mark_seen();
            }
        }
    }

    /// Uses what came back from the editor: `result` is the edited text.
    pub fn finish_edit(&mut self, job: External, result: Result<String, String>) {
        // Now, rather than when it opened, so it isn't hidden behind the editor.
        if let External::Edit { request, .. } = &job {
            self.syntax_hint(request.language.as_deref());
        }
        let text = match result {
            Ok(text) => text,
            Err(e) => return self.toast(&e),
        };
        match job {
            External::Edit { session, request, .. } => {
                let Some(idx) = self.session_index(session) else { return self.toast(&format!("{} wasn't sent: its tab has closed", request.name)) };
                if text == request.content && !request.is_local() {
                    return self.toast("Unchanged, so nothing was sent");
                }
                // Remember what was sent, so Alt+E can reopen it if the server reports errors.
                let sent = EditRequest { content: text.clone(), ..request.clone() };
                self.sessions[idx].last_edit = Some((sent, None));
                let out = self.sessions[idx].engine.save_edit(&request, &text);
                self.apply(idx, out);
                self.toast(&format!("Sent {}", request.name));
            }
            External::EditInput { session, puppet } => {
                if let Some(input) = self.input_of(session, puppet.as_deref()) {
                    input.set(text.trim_end_matches('\n'));
                }
            }
            External::EditConfig => self.reload_config(),
            External::Agent { .. } => {}
        }
    }

    /// Once ever: after a MOO edit in an editor that has no MOO syntax file of ours, says that
    /// `/editor syntax` installs one.
    fn syntax_hint(&mut self, language: Option<&str>) {
        if self.config.settings.editor_syntax_hint_shown {
            return;
        }
        let Some(editor) = Editor::current() else { return };
        let Some(file) = language.and_then(|l| editor.file(l)) else { return };
        if file.exists() {
            return;
        }
        let missing = if editor.knows_moo() { "fuller MOO highlighting" } else { "MOO highlighting" };
        self.toast(&format!("/editor syntax installs {missing} for {}", editor.name()));
        self.config.settings.editor_syntax_hint_shown = true;
        self.save_config();
    }

    /// `/editor syntax`: installs the MOO syntax file for `$EDITOR`, replacing one that's
    /// there already only if `force`.
    pub(super) fn install_syntax(&mut self, force: bool) {
        let Some(editor) = Editor::current() else {
            let (_, name) = familiar_platform::editor_command();
            return self.toast(&format!("There's no MOO syntax file for {name}, only for micro, Vim, Neovim and Emacs"));
        };
        let Some(file) = editor.file("moo") else { return self.toast("Couldn't find your home folder") };
        let place = familiar_config::paths::display(&file.path);
        match file.install(force) {
            Ok(Installed::Written) => self.toast(&format!("Installed MOO highlighting for {} at {place}", editor.name())),
            Ok(Installed::AlreadyThere) => self.toast(&format!("{place} is there already, so it was left alone (/editor syntax force replaces it)")),
            Err(e) => return self.toast(&format!("Couldn't write {place}: {e}")),
        }
        // Too long to read in a toast, so it goes in the world's output too.
        if let Some(line) = editor.setup(&file) {
            let note = format!("To use {place}, add this line to your Emacs init file: {line}");
            match self.current() {
                Some((idx, _)) => self.sessions[idx].output.notice(&note),
                None => self.toast(&note),
            }
        }
        // There's nothing left to hint at.
        self.config.settings.editor_syntax_hint_shown = true;
        self.save_config();
    }
}
