//! `$EDITOR`: in a tab of its own, or (the main loop's job) with the whole terminal.

use super::*;

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

    pub(super) fn active_editor(&mut self) -> Option<&mut EditorTab> {
        let Some(Tab::Editor(id)) = self.tabs.get(self.active) else { return None };
        self.editors.iter_mut().find(|e| e.id == *id)
    }

    fn open_editor_tab(&mut self, job: &External) -> Result<(), String> {
        let (document, title, line) = self.document(job)?;
        // Already being edited: go there instead of opening the same file twice.
        if let Some(open) = self.editors.iter().find(|e| e.document.path == document.path) {
            let tab = Tab::Editor(open.id);
            if let Some(pos) = self.tabs.iter().position(|t| *t == tab) {
                self.switch_to(pos);
            }
            return Ok(());
        }
        let (command, name) = crate::editor::command(&document.path, line);
        let id = self.next_id;
        // Everything but the tab bar and the status line.
        let size = (self.screen.0, self.screen.1.saturating_sub(2));
        let term = match Term::spawn(&command, size, id, self.pty_tx.clone()) {
            Ok(term) => term,
            Err(e) => {
                document.discard();
                return Err(format!("{name}: {e}"));
            }
        };
        self.next_id += 1;
        self.editors.push(EditorTab { id, title, term, job: job.clone(), document });
        // Beside the tab it belongs to.
        let owner = self.owner_tab(job).unwrap_or(self.active);
        let pos = (owner + 1).min(self.tabs.len());
        self.tabs.insert(pos, Tab::Editor(id));
        self.overlay = None;
        self.switch_to(pos);
        Ok(())
    }

    /// The tab an edit was started from.
    fn owner_tab(&self, job: &External) -> Option<usize> {
        let tab = match job {
            External::Edit { session, .. } | External::EditInput { session, puppet: None } => Tab::Main(*session),
            External::EditInput { session, puppet: Some(name) } => Tab::Puppet(*session, name.clone()),
            External::EditConfig => return None,
        };
        self.tabs.iter().position(|t| *t == tab)
    }

    /// Takes an editor and its tab away, keeping the same tab active where it can.
    pub(super) fn remove_editor(&mut self, id: u64) -> Option<EditorTab> {
        let editor = self.editors.remove(self.editors.iter().position(|e| e.id == id)?);
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
                let result = if ok {
                    editor.document.finish()
                } else {
                    editor.document.discard();
                    Err(format!("The editor on {} exited with an error, so nothing was saved", editor.title))
                };
                self.finish_edit(editor.job, result);
                if self.tabs.is_empty() {
                    self.open_connect_picker();
                }
                self.mark_seen();
            }
        }
    }

    /// Uses what came back from the editor: `result` is the edited text.
    pub fn finish_edit(&mut self, job: External, result: Result<String, String>) {
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
        }
    }
}
