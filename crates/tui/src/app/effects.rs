//! Carrying out what a session's output asks of the app: sounds, notifications, links, editors, tabs.

use super::*;

impl App {
    pub(super) fn effect(&mut self, idx: usize, effect: Effect) {
        let id = self.sessions[idx].id;
        match effect {
            Effect::Notify { title, body } => {
                if !self.focused {
                    familiar_platform::notify(familiar_config::APP_NAME, &title, &body);
                }
            }
            Effect::PlaySound(path) => self.play_sound("", &path, 1.0),
            Effect::Media(cmd) => match cmd {
                familiar_engine::media::MediaCommand::Play { name, url, volume, .. } => {
                    if url.starts_with("http://") || url.starts_with("https://") {
                        // Download off the UI thread; play when it arrives, unless a stop came
                        // in the meantime.
                        let sounds = self.sounds.clone();
                        let stops = sounds.stop_count();
                        tokio::task::spawn_blocking(move || {
                            if let Ok(path) = familiar_engine::media::fetch(&url)
                                && sounds.stop_count() == stops
                            {
                                let _ = sounds.play(&name, &path, volume);
                            }
                        });
                    } else {
                        self.play_sound(&name, &url, volume);
                    }
                }
                familiar_engine::media::MediaCommand::Stop { name } => self.sounds.stop(name.as_deref()),
                familiar_engine::media::MediaCommand::Preload { .. } => {}
            },
            Effect::OpenUri(uri) => {
                if uri.starts_with("http://") || uri.starts_with("https://") || uri.starts_with("mailto:") {
                    match familiar_platform::open_uri(&uri) {
                        Ok(()) => self.toast(&format!("Opening {uri}")),
                        Err(e) => self.toast(&e),
                    }
                }
            }
            Effect::Edit(request) => self.edit(External::Edit { session: id, request, line: None }),
            Effect::Reconnect => self.connect(idx),
            Effect::ReloadScripts => self.start_scripting(idx),
            Effect::Bell => {
                use std::io::Write;
                let _ = std::io::stdout().write_all(b"\x07");
            }
            Effect::Speak(text) => {
                self.speaker.say(&text);
                if let Some(e) = self.speaker.take_error() {
                    self.toast(&e);
                }
            }
            Effect::SpeechStop => self.speaker.stop(),
            Effect::Variables { scope, variables } => {
                if let Some(rules) = self.config.rules_mut(&scope) {
                    rules.variables = variables;
                }
                self.save_config();
            }
            Effect::Settings(settings) => {
                // Settings changed from inside a session (/images, /speech) don't know about ours.
                let Settings { terminal_colors, editor_in_tab, editor_in_sidebar, editor_syntax_hint_shown, .. } = self.config.settings;
                self.config.settings = Settings { terminal_colors, editor_in_tab, editor_in_sidebar, editor_syntax_hint_shown, ..settings };
                self.save_config();
            }
            Effect::OpenPuppet { name, focus } => {
                let tab = Tab::Puppet(id, name);
                let pos = match self.tabs.iter().position(|t| *t == tab) {
                    Some(p) => p,
                    None => {
                        // After the session's other tabs.
                        let after = self.tabs.iter().rposition(|t| matches!(t, Tab::Main(i) | Tab::Puppet(i, _) if *i == id)).map_or(self.tabs.len(), |p| p + 1);
                        self.tabs.insert(after, tab);
                        if self.active >= after && !self.tabs.is_empty() && self.active + 1 < self.tabs.len() {
                            self.active += 1;
                        }
                        after
                    }
                };
                if focus {
                    self.switch_to(pos);
                }
            }
            Effect::Agent(args) => self.agent_command(&args),
            Effect::AgentTools => {
                self.agents.tools_changed(id);
            }
            Effect::AgentResult { id: call, ok, content } => {
                self.agents.result(id, call, ok, content);
            }
            Effect::ShowPanel(panel) => {
                let explicit = panel == Panel::Map;
                let s = &mut self.sessions[idx];
                if let Some(p) = s.panels().iter().position(|p| *p == panel)
                    && (explicit || !self.sidebar || s.panels().len() == 1)
                {
                    s.panel = p;
                }
                if explicit || self.view.0 >= 90 {
                    self.sidebar = true;
                }
            }
        }
    }

    pub(super) fn play_sound(&mut self, name: &str, path: &str, volume: f64) {
        if let Err(e) = self.sounds.play(name, &familiar_config::paths::expand(path), volume) {
            self.toast(&e);
        }
    }
}
