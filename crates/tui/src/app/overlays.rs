//! The pickers (connect, command palette, history) and the suggestions shown while typing a / command.

use super::*;

impl App {
    pub fn open_connect_picker(&mut self) {
        let mut items = vec![Item { group: "Saved worlds", label: "＋ New world…".into(), detail: "Ctrl+N".into(), action: Action::Run("new-world") }];
        items.extend(self.saved_world_items("Saved worlds"));
        for e in familiar_config::directory::entries() {
            if self.config.worlds.iter().any(|w| w.host.eq_ignore_ascii_case(&e.host) && w.port == e.port) {
                continue;
            }
            items.push(Item { group: "Directory", label: e.name.clone(), detail: format!("{}:{}", e.host, e.port), action: Action::Connect { world: Box::new(e.to_world()), character: None } });
        }
        self.overlay = Some(Overlay::Picker(Box::new(Picker::new(PickerKind::Connect, "Connect", items))));
    }

    /// Each saved world (with characters, each character) as an item that connects to it.
    fn saved_world_items(&self, group: &'static str) -> Vec<Item> {
        let mut items = Vec::new();
        for w in &self.config.worlds {
            if w.characters.is_empty() {
                items.push(Item { group, label: w.name.clone(), detail: format!("{}:{}", w.host, w.port), action: Action::Connect { world: Box::new(w.clone()), character: None } });
            }
            for c in &w.characters {
                items.push(Item {
                    group,
                    label: format!("{} @ {}", c.name, w.name),
                    detail: format!("{}:{}", w.host, w.port),
                    action: Action::Connect { world: Box::new(w.clone()), character: Some(c.name.clone()) },
                });
            }
        }
        items
    }

    pub(super) fn open_palette(&mut self) {
        // Every shortcut's action, with its key. Tab 1 to 9 are under Tabs, by name.
        let mut items: Vec<Item> = familiar_engine::keymap::actions(Platform::Terminal)
            .filter(|a| !a.id.starts_with("tab-"))
            .map(|a| Item { group: "Commands", label: a.label.to_string(), detail: self.key_for(a.id).unwrap_or_default(), action: Action::Run(a.id) })
            .collect();
        for (i, tab) in self.tabs.iter().enumerate() {
            let key = self.key_for(&format!("tab-{}", i + 1)).unwrap_or_default();
            items.push(Item { group: "Tabs", label: format!("Switch to {}", self.tab_title(tab)), detail: key, action: Action::Insert(format!("\u{0}tab{i}")) });
        }
        items.extend(self.saved_world_items("Worlds"));
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
            Action::Run(id) => self.run(id),
            Action::Insert(text) => {
                if let Some(i) = text.strip_prefix("\u{0}tab").and_then(|n| n.parse().ok()) {
                    self.switch_to(i);
                } else if let Some(input) = self.current_input() {
                    input.set(&text);
                }
            }
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
