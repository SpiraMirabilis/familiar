//! The pickers (connect, command palette, history) and the suggestions shown while typing a / command.

use super::*;

impl App {
    pub fn open_connect_picker(&mut self) {
        let mut items = vec![Item { group: "Saved worlds", label: "＋ New world…".into(), detail: "Ctrl+N".into(), action: Action::Run(Cmd::NewWorld) }];
        for w in &self.config.worlds {
            if w.characters.is_empty() {
                items.push(Item { group: "Saved worlds", label: w.name.clone(), detail: format!("{}:{}", w.host, w.port), action: Action::Connect { world: Box::new(w.clone()), character: None } });
            }
            for c in &w.characters {
                items.push(Item {
                    group: "Saved worlds",
                    label: format!("{} @ {}", c.name, w.name),
                    detail: format!("{}:{}", w.host, w.port),
                    action: Action::Connect { world: Box::new(w.clone()), character: Some(c.name.clone()) },
                });
            }
        }
        for e in familiar_config::directory::entries() {
            if self.config.worlds.iter().any(|w| w.host.eq_ignore_ascii_case(&e.host) && w.port == e.port) {
                continue;
            }
            items.push(Item { group: "Directory", label: e.name.clone(), detail: format!("{}:{}", e.host, e.port), action: Action::Connect { world: Box::new(e.to_world()), character: None } });
        }
        self.overlay = Some(Overlay::Picker(Box::new(Picker::new(PickerKind::Connect, "Connect", items))));
    }

    pub(super) fn open_palette(&mut self) {
        let mut items: Vec<Item> = PALETTE.iter().map(|(cmd, label, keys)| Item { group: "Commands", label: label.to_string(), detail: keys.to_string(), action: Action::Run(*cmd) }).collect();
        for (i, tab) in self.tabs.iter().enumerate() {
            items.push(Item { group: "Tabs", label: format!("Switch to {}", self.tab_title(tab)), detail: format!("Alt+{}", i + 1), action: Action::Insert(format!("\u{0}tab{i}")) });
        }
        for (usage, description) in familiar_engine::COMMANDS.iter().chain(TUI_COMMANDS) {
            items.push(Item { group: "Client commands", label: usage.to_string(), detail: description.to_string(), action: Action::Insert(insert_for(usage)) });
        }
        self.overlay = Some(Overlay::Picker(Box::new(Picker::new(PickerKind::Palette, "Commands", items))));
    }

    pub(super) fn open_history(&mut self) {
        let Some(input) = self.current_input() else { return };
        let mut seen = Vec::new();
        let items = input
            .history()
            .iter()
            .rev()
            .filter(|h| {
                let new = !seen.contains(*h);
                if new {
                    seen.push((*h).clone());
                }
                new
            })
            .map(|h| Item { group: "History", label: h.clone(), detail: String::new(), action: Action::Insert(h.clone()) })
            .collect();
        self.overlay = Some(Overlay::Picker(Box::new(Picker::new(PickerKind::History, "History", items))));
    }

    pub(super) fn picker_query_changed(&mut self) {
        let Some(Overlay::Picker(p)) = &mut self.overlay else { return };
        if p.kind == PickerKind::Connect {
            p.items.retain(|i| !i.group.is_empty());
            if let Some(world) = parse_address(p.query.text.trim()) {
                let label = format!("Connect to {}:{}", world.host, world.port);
                p.items.insert(0, Item { group: "", label, detail: if world.tls { "TLS".into() } else { String::new() }, action: Action::Connect { world: Box::new(world), character: None } });
            }
        }
        p.refilter();
    }

    pub(super) fn run_picker(&mut self) {
        let Some(Overlay::Picker(picker)) = self.overlay.take() else { return };
        let Some(index) = picker.matches.get(picker.selected).map(|(i, _)| *i) else {
            self.overlay = Some(Overlay::Picker(picker));
            return;
        };
        let mut picker = picker;
        let item = picker.items.swap_remove(index);
        match item.action {
            Action::Connect { world, character } => self.open_session(*world, character),
            Action::Run(cmd) => self.run(cmd),
            Action::Insert(text) => {
                if let Some(i) = text.strip_prefix("\u{0}tab").and_then(|n| n.parse().ok()) {
                    self.switch_to(i);
                } else if let Some(input) = self.current_input() {
                    input.set(&text);
                }
            }
        }
    }

    pub fn run(&mut self, cmd: Cmd) {
        let current = self.current();
        match cmd {
            Cmd::Connect => self.open_connect_picker(),
            Cmd::NewWorld => self.overlay = Some(Overlay::World(Box::new(WorldForm::new(None, None, false)))),
            Cmd::EditWorld => match current {
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
                None => self.run(Cmd::NewWorld),
            },
            Cmd::History => self.open_history(),
            Cmd::NextTab if !self.tabs.is_empty() => self.switch_to((self.active + 1) % self.tabs.len()),
            Cmd::PrevTab if !self.tabs.is_empty() => self.switch_to((self.active + self.tabs.len() - 1) % self.tabs.len()),
            Cmd::CloseTab => self.close_tab(),
            Cmd::Reconnect => {
                if let Some((i, _)) = current {
                    self.connect(i);
                }
            }
            Cmd::Disconnect => {
                if let Some((i, _)) = current {
                    self.sessions[i].engine.disconnect();
                }
            }
            Cmd::Clear => {
                if let Some(b) = self.current_buffer() {
                    b.clear();
                }
            }
            Cmd::ToggleSidebar => self.sidebar = !self.sidebar,
            Cmd::NextPanel => {
                if let Some((i, _)) = current {
                    let s = &mut self.sessions[i];
                    let n = s.panels().len().max(1);
                    s.panel = (s.panel + 1) % n;
                    self.sidebar = true;
                }
            }
            Cmd::ShowMap => {
                if let Some((i, _)) = current {
                    let out = self.sessions[i].engine.submit("/map", false);
                    self.apply(i, out);
                }
            }
            Cmd::ToggleLog => {
                if let Some((i, _)) = current {
                    let out = self.sessions[i].engine.toggle_log();
                    self.apply(i, out);
                }
            }
            Cmd::EditInput => {
                if let Some((i, puppet)) = current {
                    self.edit(External::EditInput { session: self.sessions[i].id, puppet });
                }
            }
            Cmd::EditConfig => self.edit(External::EditConfig),
            Cmd::Rules => self.open_rules(),
            Cmd::StopSpeech => self.speaker.stop(),
            Cmd::Help => self.overlay = Some(Overlay::Help),
            Cmd::Quit => self.quit = true,
            Cmd::NextTab | Cmd::PrevTab => {}
        }
    }

    pub fn suggestions(&self) -> Vec<Suggestion> {
        let Some((i, puppet)) = self.current() else { return Vec::new() };
        if self.suggestions_hidden || puppet.is_some() || self.overlay.is_some() {
            return Vec::new();
        }
        let s = &self.sessions[i];
        let text = &s.input.text;
        if !text.starts_with('/') || text.starts_with("//") || text.contains('\n') || s.password {
            return Vec::new();
        }
        let (word, rest) = text.split_once(' ').map_or((text.as_str(), None), |(w, r)| (w, Some(r)));
        let lower = word.to_lowercase();
        let Some(rest) = rest else {
            let mut out: Vec<Suggestion> = familiar_engine::COMMANDS
                .iter()
                .chain(TUI_COMMANDS)
                .filter(|(usage, _)| usage.split(' ').next().is_some_and(|w| w.starts_with(&lower)))
                .map(|(usage, detail)| Suggestion { insert: insert_for(usage), label: usage.to_string(), detail: detail.to_string() })
                .collect();
            out.dedup_by(|a, b| a.insert == b.insert && a.label == b.label);
            return out;
        };
        let rest_lower = rest.to_lowercase();
        let arg = |label: String, insert: String, detail: String| Suggestion { insert, label, detail };
        match lower.as_str() {
            "/connect" => self
                .config
                .worlds
                .iter()
                .flat_map(|w| {
                    let world = std::iter::once(arg(w.name.clone(), format!("/connect {}", w.name), format!("{}:{}", w.host, w.port)));
                    let chars = w.characters.iter().map(move |c| arg(format!("{} {}", w.name, c.name), format!("/connect {} {}", w.name, c.name), "character".into()));
                    world.chain(chars)
                })
                .filter(|sug| sug.label.to_lowercase().starts_with(&rest_lower))
                .take(20)
                .collect(),
            "/puppet" => s
                .engine
                .puppets()
                .filter(|p| p.name.to_lowercase().starts_with(&rest_lower))
                .map(|p| arg(p.name.clone(), format!("/puppet {}", p.name), format!("prefix {}", p.send_prefix)))
                .collect(),
            "/map" => {
                let (sub, query) = rest.split_once(' ').unwrap_or((rest, ""));
                if matches!(sub, "go" | "walk" | "here") && !query.is_empty() {
                    let map = s.engine.map();
                    map.search(query).into_iter().take(12).filter_map(|id| map.room(id)).map(|r| arg(r.name.clone(), format!("/map {sub} {}", r.name), r.area.clone())).collect()
                } else if !rest.contains(' ') {
                    ["go", "here", "addroom", "rename", "delete", "import", "track", "clear"]
                        .iter()
                        .filter(|c| c.starts_with(sub))
                        .map(|c| arg(format!("/map {c}"), format!("/map {c} "), String::new()))
                        .collect()
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        }
    }
}

/// The input text for picking a command from its usage: everything up to the first
/// placeholder or choice, plus a space if arguments follow.
pub fn insert_for(usage: &str) -> String {
    let mut literal = Vec::new();
    let mut more = false;
    for (i, token) in usage.split(' ').enumerate() {
        if i > 0 && (token.contains(['<', '[', '|']) || token == "...") {
            more = true;
            break;
        }
        let cut = token.find('<').map_or(token, |p| &token[..p]);
        literal.push(cut);
        if cut.len() < token.len() {
            break;
        }
    }
    let mut text = literal.join(" ");
    if more {
        text.push(' ');
    }
    text
}
