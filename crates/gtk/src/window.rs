//! The main window: a tab per connection. Its commands are window actions named as in the
//! engine's keymap ("win.next-tab"), and its shortcuts come from the keymap too.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use familiar_config::{Character, World};
use familiar_engine::KeyChord;
use familiar_engine::keymap::{self, Platform, Scope};

use crate::app::AppState;
use crate::palette::{self, Item};
use crate::session::SessionView;
use crate::{connect_dialog, rules_dialog, world_dialog};

const HELP_URL: &str = "https://github.com/SpiraMirabilis/familiar#readme";

pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    tab_view: adw::TabView,
    stack: gtk::Stack,
    toasts: adw::ToastOverlay,
    /// Show the current shortcuts.
    connect_button: gtk::Button,
    menu_button: gtk::MenuButton,
    state: Rc<AppState>,
    sessions: RefCell<Vec<(adw::TabPage, Rc<SessionView>)>>,
}

impl MainWindow {
    pub fn new(app: &adw::Application, state: Rc<AppState>) -> Rc<Self> {
        let tab_view = adw::TabView::new();
        let tab_bar = adw::TabBar::builder().view(&tab_view).build();

        // AdwTabView's own shortcuts (Ctrl+Tab, Alt+1...) would take keys before the keymap,
        // and couldn't be changed.
        tab_view.set_shortcuts(adw::TabViewShortcuts::empty());

        let connect_button = gtk::Button::builder().icon_name("list-add-symbolic").action_name("win.connect").build();
        let menu_button = gtk::MenuButton::builder().icon_name("open-menu-symbolic").tooltip_text("Main Menu").primary(true).build();

        let header = adw::HeaderBar::new();
        header.pack_start(&connect_button);
        header.pack_end(&menu_button);

        let empty = adw::StatusPage::builder()
            .icon_name("network-server-symbolic")
            .title("Not Connected")
            .description("Connect to a MUD, MUSH, MUCK or MOO to get started.")
            .build();
        let open = gtk::Button::builder()
            .label("Connect…")
            .action_name("win.connect")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        empty.set_child(Some(&open));

        let stack = gtk::Stack::new();
        stack.add_named(&empty, Some("empty"));
        stack.add_named(&tab_view, Some("tabs"));

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&stack));
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.add_top_bar(&tab_bar);
        toolbar.set_content(Some(&toasts));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(crate::APP_NAME)
            .default_width(1000)
            .default_height(720)
            .content(&toolbar)
            .build();

        let this = Rc::new(Self { window, tab_view, stack, toasts, connect_button, menu_button, state, sessions: RefCell::default() });
        this.setup_actions();
        this.setup_keys();
        this.setup_tabs();
        this.show_keys();
        let weak = Rc::downgrade(&this);
        this.state.connect_keys_changed(move || weak.upgrade().inspect(|this| this.show_keys()).is_some());
        // Closing the window quits, so save maps and clean up sessions first.
        let weak = Rc::downgrade(&this);
        this.window.connect_close_request(move |_| {
            if let Some(this) = weak.upgrade() {
                for (_, session) in this.sessions.borrow().iter() {
                    session.close();
                }
            }
            glib::Propagation::Proceed
        });
        if let Some(e) = this.state.load_error() {
            this.toast(&format!("Couldn't read the config file: {e}"));
        }
        this
    }

    pub fn toast(&self, text: &str) {
        let toast = adw::Toast::new(text);
        toast.set_use_markup(false);
        self.toasts.add_toast(toast);
    }

    fn current(&self) -> Option<Rc<SessionView>> {
        let page = self.tab_view.selected_page()?;
        self.sessions.borrow().iter().find(|(p, _)| *p == page).map(|(_, s)| s.clone())
    }

    fn setup_tabs(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.tab_view.connect_close_page(move |view, page| {
            if let Some(this) = weak.upgrade() {
                let removed = {
                    let mut sessions = this.sessions.borrow_mut();
                    sessions.iter().position(|(p, _)| p == page).map(|pos| sessions.remove(pos).1)
                };
                match removed {
                    Some(session) => {
                        session.close();
                        // A character's puppets close with it.
                        for puppet in session.puppet_pages() {
                            view.close_page(&puppet);
                        }
                    }
                    None => {
                        for (_, session) in this.sessions.borrow().iter() {
                            if session.puppet_tab_closed(page) {
                                break;
                            }
                        }
                    }
                }
            }
            view.close_page_finish(page, true);
            glib::Propagation::Stop
        });

        let weak = Rc::downgrade(self);
        self.tab_view.connect_n_pages_notify(move |view| {
            if let Some(this) = weak.upgrade() {
                this.stack.set_visible_child_name(if view.n_pages() > 0 { "tabs" } else { "empty" });
            }
        });

        let weak = Rc::downgrade(self);
        self.tab_view.connect_selected_page_notify(move |_| {
            let Some(this) = weak.upgrade() else { return };
            this.update_log_state();
            if let Some(session) = this.current() {
                session.seen();
                session.input.grab_focus();
            }
        });
    }

    /// Connects to a saved world by name (ignoring case), as a named character or the first.
    pub fn open_world(self: &Rc<Self>, world: &str, character: Option<&str>) {
        let found = self.state.config.borrow().worlds.iter().find(|w| w.name.eq_ignore_ascii_case(world)).cloned();
        let Some(found) = found else {
            return self.toast(&format!("No saved world called {world}"));
        };
        let chosen = match character {
            Some(name) => match found.characters.iter().find(|c| c.name.eq_ignore_ascii_case(name)) {
                Some(c) => Some(c.clone()),
                None => return self.toast(&format!("{} has no character called {name}", found.name)),
            },
            None => found.characters.first().cloned(),
        };
        self.open_session(found, chosen);
    }

    pub fn open_session(self: &Rc<Self>, world: World, character: Option<Character>) {
        let session = SessionView::new(self.state.clone(), world, character);
        let page = self.tab_view.append(&session.root);
        session.set_page(&page);

        let window = self.window.clone();
        session.connect_open_uri(move |uri| {
            if familiar_engine::media::is_safe_open_uri(uri) {
                gtk::UriLauncher::new(uri).launch(Some(&window), None::<&gio::Cancellable>, |_| {});
            }
        });
        let tabs = self.tab_view.clone();
        session.connect_open_tab(move |widget, title| {
            let page = tabs.append(widget);
            page.set_title(title);
            page
        });
        let weak = Rc::downgrade(self);
        session.connect_logging_changed(move || {
            if let Some(this) = weak.upgrade() {
                this.update_log_state();
            }
        });

        self.sessions.borrow_mut().push((page.clone(), session.clone()));
        self.tab_view.set_selected_page(&page);
        // Focus once the page is mapped; before that grab_focus does nothing.
        let input = session.input.clone();
        glib::idle_add_local_once(move || input.grab_focus());
        session.connect();
    }

    fn update_log_state(&self) {
        let logging = self.current().is_some_and(|s| s.is_logging());
        if let Some(action) = self.window.lookup_action("toggle-log") {
            action.change_state(&logging.to_variant());
        }
    }

    /// A window action for every command in the keymap, each running [`Self::run`].
    fn setup_actions(self: &Rc<Self>) {
        for action in keymap::actions(Platform::Linux).filter(|a| a.scope == Scope::Window) {
            let id = action.id;
            // Stateful, so the menu shows a check mark while logging.
            let gio_action = match id {
                "toggle-log" => gio::SimpleAction::new_stateful(id, None, &false.to_variant()),
                _ => gio::SimpleAction::new(id, None),
            };
            let weak = Rc::downgrade(self);
            gio_action.connect_activate(move |_, _| {
                if let Some(this) = weak.upgrade() {
                    this.run(id);
                }
            });
            self.window.add_action(&gio_action);
        }
    }

    /// Runs shortcuts from the keymap. In the capture phase, so they come before keyboard
    /// macros (which the input and output check for themselves) and before the focused
    /// widget's own keys.
    fn setup_keys(self: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |controller, key, keycode, state| {
            let Some(this) = weak.upgrade() else { return glib::Propagation::Proceed };
            // A dialog (Preferences recording a key, a picker) has the keyboard to itself.
            if this.window.visible_dialog().is_some() {
                return glib::Propagation::Proceed;
            }
            let Some(chord) = crate::keys::chord(controller, key, keycode, state) else { return glib::Propagation::Proceed };
            let Some(id) = this.state.keymap().lookup(Scope::Window, &chord) else { return glib::Propagation::Proceed };
            if this.terminal_wants(&chord) {
                return glib::Propagation::Proceed;
            }
            this.run(id);
            glib::Propagation::Stop
        });
        self.window.add_controller(keys);
    }

    /// An agent's terminal has the focus and this is a key terminal programs use themselves
    /// (Ctrl or Alt with a letter), so it goes to the program rather than to a shortcut.
    #[cfg(feature = "agents")]
    fn terminal_wants(&self, chord: &KeyChord) -> bool {
        let focus = gtk::prelude::GtkWindowExt::focus(&self.window);
        let in_terminal = focus.is_some_and(|w| w.is::<vte4::Terminal>() || w.ancestor(vte4::Terminal::static_type()).is_some());
        let letter = chord.key.len() == 1 && chord.key.chars().all(|c| c.is_ascii_alphabetic());
        in_terminal && letter && chord.ctrl != chord.alt && !chord.shift && !chord.cmd
    }

    #[cfg(not(feature = "agents"))]
    fn terminal_wants(&self, _chord: &KeyChord) -> bool {
        false
    }

    /// Puts the current shortcuts in the main menu and the Connect button's tooltip.
    fn show_keys(&self) {
        let tooltip = match self.state.key_label("connect") {
            Some(key) => format!("Connect ({key})"),
            None => "Connect".into(),
        };
        self.connect_button.set_tooltip_text(Some(&tooltip));
        self.menu_button.set_menu_model(Some(&self.menu()));
    }

    fn menu(&self) -> gio::Menu {
        let keymap = self.state.keymap();
        let section = |items: &[(&str, &str)]| {
            let section = gio::Menu::new();
            for (label, id) in items {
                let item = gio::MenuItem::new(Some(label), Some(&format!("win.{id}")));
                // Only shown: the window's key controller runs the shortcuts.
                if let Some(accel) = keymap.keys(id).first().and_then(crate::keys::accel) {
                    item.set_attribute_value("accel", Some(&accel.to_variant()));
                }
                section.append_item(&item);
            }
            section
        };
        let about = format!("About {}", crate::APP_NAME);
        let menu = gio::Menu::new();
        menu.append_section(None, &section(&[("New World…", "new-world"), ("Edit World…", "edit-world"), ("Reconnect", "reconnect"), ("Disconnect", "disconnect")]));
        menu.append_section(None, &section(&[("Find…", "find"), ("Clear Output", "clear"), ("Log to File", "toggle-log"), ("Open Log Folder", "open-logs")]));
        menu.append_section(
            None,
            &section(&[
                ("Command Palette…", "palette"),
                ("Show Map", "show-map"),
                ("Triggers, Aliases & Macros", "rules"),
                ("Import from BeipMU…", "import-beipmu"),
                ("Import a BeipMU Map…", "import-map"),
            ]),
        );
        menu.append_section(
            None,
            &section(&[("Preferences", "preferences"), ("Keyboard Shortcuts", "keyboard"), ("Help", "help"), (&about, "about"), ("Quit", "quit")]),
        );
        menu
    }

    /// Runs a command from the keymap, by its id.
    fn run(self: &Rc<Self>, id: &str) {
        let current = self.current();
        let session = current.as_ref();
        match id {
            "connect" => {
                let weak = Rc::downgrade(self);
                connect_dialog::show(&self.window, self.state.clone(), move |world, character| {
                    if let Some(this) = weak.upgrade() {
                        this.open_session(world, character);
                    }
                });
            }
            "new-world" => self.new_world(),
            "edit-world" => match session {
                Some(s) => self.edit_world(s.world()),
                None => self.new_world(),
            },
            "reconnect" => session.into_iter().for_each(|s| s.connect()),
            "disconnect" => session.into_iter().for_each(|s| s.disconnect()),
            "close-tab" => {
                if let Some(page) = self.tab_view.selected_page() {
                    self.tab_view.close_page(&page);
                }
            }
            "next-tab" => self.cycle_tabs(1),
            "prev-tab" => self.cycle_tabs(-1),
            "find" => session.into_iter().for_each(|s| s.find()),
            "find-next" => session.into_iter().for_each(|s| s.find_again(false)),
            "find-previous" => session.into_iter().for_each(|s| s.find_again(true)),
            "clear" => session.into_iter().for_each(|s| s.clear()),
            "toggle-log" => {
                if let Some(s) = session {
                    s.toggle_log();
                    self.update_log_state();
                }
            }
            "open-logs" => self.open_folder(&familiar_config::log_dir()),
            "toggle-sidebar" => {
                if session.is_some_and(|s| !s.toggle_side()) {
                    self.toast("Nothing is in the side dock yet");
                }
            }
            "next-panel" | "prev-panel" => {
                if session.is_some_and(|s| !s.cycle_side(if id == "next-panel" { 1 } else { -1 })) {
                    self.toast("Nothing is in the side dock yet");
                }
            }
            "show-map" => session.into_iter().for_each(|s| s.show_map(true)),
            "palette" => self.show_palette(),
            "history-search" => {
                if let Some(s) = session {
                    self.show_history(s);
                }
            }
            "rules" => {
                let current = session.map(|s| s.scopes()).unwrap_or_default();
                let weak = Rc::downgrade(self);
                rules_dialog::show(&self.window, self.state.clone(), current.first().cloned(), move || {
                    if let Some(this) = weak.upgrade() {
                        this.reload_sessions();
                    }
                });
            }
            "reopen-editor" => {
                if session.is_some_and(|s| !s.reopen_editor()) {
                    self.toast("Nothing to reopen: no program has been sent from an editor in this tab yet");
                }
            }
            "restart-scripts" => session.into_iter().for_each(|s| s.restart_scripts()),
            "stop-speech" => self.state.speaker.stop(),
            "import-beipmu" => {
                let weak = Rc::downgrade(self);
                crate::import::choose_and_import(self.window.upcast_ref(), self.state.clone(), move || {
                    if let Some(this) = weak.upgrade() {
                        this.reload_sessions();
                    }
                });
            }
            "import-map" => match session {
                Some(s) => self.import_map(s.clone()),
                None => self.toast("Open a world's tab first: the map is imported into that world's map"),
            },
            "preferences" => self.show_preferences(None),
            "keyboard" => self.show_preferences(Some("keyboard")),
            "open-settings-folder" => self.open_folder(&familiar_config::config_dir()),
            "help" => gtk::UriLauncher::new(HELP_URL).launch(Some(&self.window), None::<&gio::Cancellable>, |_| {}),
            "about" => self.show_about(),
            // Closing the window saves the maps, and quits.
            "quit" => self.window.close(),
            _ => {
                // tab-1 to tab-9.
                let n = id.strip_prefix("tab-").and_then(|n| n.parse::<i32>().ok());
                if let Some(n) = n.filter(|n| (1..=self.tab_view.n_pages()).contains(n)) {
                    self.tab_view.set_selected_page(&self.tab_view.nth_page(n - 1));
                }
            }
        }
    }

    fn reload_sessions(&self) {
        for (_, session) in self.sessions.borrow().iter() {
            session.reload();
        }
    }

    /// The next (`step` 1) or previous (-1) tab, wrapping around.
    fn cycle_tabs(&self, step: i32) {
        let n = self.tab_view.n_pages();
        if n == 0 {
            return;
        }
        let at = self.tab_view.selected_page().map_or(0, |p| self.tab_view.page_position(&p));
        self.tab_view.set_selected_page(&self.tab_view.nth_page((at + step).rem_euclid(n)));
    }

    fn open_folder(&self, dir: &Path) {
        if let Err(e) = std::fs::create_dir_all(dir) {
            self.toast(&format!("Couldn't create {}: {e}", dir.display()));
            return;
        }
        let file = gio::File::for_path(dir);
        gtk::FileLauncher::new(Some(&file)).launch(Some(&self.window), None::<&gio::Cancellable>, |_| {});
    }

    fn show_preferences(self: &Rc<Self>, page: Option<&str>) {
        let weak = Rc::downgrade(self);
        crate::preferences_dialog::show(&self.window, self.state.clone(), page, move || {
            if let Some(this) = weak.upgrade() {
                this.reload_sessions();
            }
        });
    }

    /// Saves the config after a world changed, and lets open tabs pick it up.
    fn save_worlds(&self) {
        match self.state.save() {
            Ok(()) => self.reload_sessions(),
            Err(e) => self.toast(&e),
        }
    }

    /// The world editor for a new world, as the Connect dialog's + button opens it.
    fn new_world(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let default_font = self.state.config.borrow().settings.font.clone();
        let on_save = move |world: World| {
            let Some(this) = weak.upgrade() else { return };
            let name = world.name.clone();
            this.state.config.borrow_mut().worlds.push(world);
            this.save_worlds();
            this.toast(&format!("Saved {name}"));
        };
        world_dialog::show(&self.window, None, &default_font, on_save, None);
    }

    /// The world editor for the current tab's world. A quick connection isn't saved yet, so
    /// saving it adds it to the saved worlds.
    fn edit_world(self: &Rc<Self>, world: World) {
        let original = world.name.clone();
        let saved = self.state.config.borrow().worlds.iter().find(|w| w.name == original).cloned();
        let default_font = self.state.config.borrow().settings.font.clone();
        let on_save = {
            let weak = Rc::downgrade(self);
            let original = original.clone();
            move |edited: World| {
                let Some(this) = weak.upgrade() else { return };
                {
                    let mut config = this.state.config.borrow_mut();
                    match config.worlds.iter_mut().find(|w| w.name == original) {
                        Some(slot) => *slot = edited,
                        None => config.worlds.push(edited),
                    }
                }
                this.save_worlds();
            }
        };
        let on_delete = saved.is_some().then(|| {
            let weak = Rc::downgrade(self);
            Box::new(move || {
                let Some(this) = weak.upgrade() else { return };
                this.state.config.borrow_mut().worlds.retain(|w| w.name != original);
                this.save_worlds();
            }) as Box<dyn Fn()>
        });
        world_dialog::show(&self.window, Some(saved.as_ref().unwrap_or(&world)), &default_font, on_save, on_delete);
    }

    /// Asks for a BeipMU .atlas file and adds its rooms to the tab's map.
    fn import_map(&self, session: Rc<SessionView>) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("BeipMU maps (.atlas)"));
        filter.add_pattern("*.atlas");
        filter.add_pattern("*.ATLAS");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder().title("Import a BeipMU Map").filters(&filters).default_filter(&filter).build();
        dialog.open(Some(&self.window), None::<&gio::Cancellable>, move |result| {
            let Some(path) = result.ok().and_then(|file| file.path()) else { return };
            session.import_map(&path);
            session.show_map(true);
        });
    }

    /// The command palette: every command, the saved worlds, and the client's / commands.
    fn show_palette(self: &Rc<Self>) {
        let mut items = Vec::new();
        {
            let keymap = self.state.keymap();
            // Tab N only for tabs there are.
            let tabs = self.tab_view.n_pages();
            let exists = |id: &str| id.strip_prefix("tab-").and_then(|n| n.parse::<i32>().ok()).is_none_or(|n| n <= tabs);
            for action in keymap::actions(Platform::Linux).filter(|a| a.scope == Scope::Window && a.id != "palette" && exists(a.id)) {
                let weak = Rc::downgrade(self);
                let id = action.id;
                items.push(Item {
                    group: "Commands",
                    label: action.label.to_string(),
                    detail: keymap.keys(id).first().map(crate::keys::label).unwrap_or_default(),
                    run: Rc::new(move || {
                        if let Some(this) = weak.upgrade() {
                            this.run(id);
                        }
                    }),
                });
            }
        }
        let worlds = self.state.config.borrow().worlds.clone();
        for world in worlds {
            let address = format!("{}:{}", world.host, world.port);
            // One entry per character, or one for the world if it has none.
            let characters: Vec<Option<Character>> =
                if world.characters.is_empty() { vec![None] } else { world.characters.iter().cloned().map(Some).collect() };
            for character in characters {
                let label = match &character {
                    Some(c) => format!("{} @ {}", c.name, world.name),
                    None => world.name.clone(),
                };
                let weak = Rc::downgrade(self);
                let world = world.clone();
                items.push(Item {
                    group: "Worlds",
                    label,
                    detail: address.clone(),
                    run: Rc::new(move || {
                        if let Some(this) = weak.upgrade() {
                            this.open_session(world.clone(), character.clone());
                        }
                    }),
                });
            }
        }
        let commands: Vec<&(&str, &str)> = familiar_engine::COMMANDS.iter().collect();
        #[cfg(feature = "agents")]
        let commands = [commands, vec![&familiar_engine::AGENT_COMMAND]].concat();
        for (usage, description) in commands {
            let weak = Rc::downgrade(self);
            let text = palette::insert_for(usage);
            items.push(Item {
                group: "Client Commands",
                label: usage.to_string(),
                detail: description.to_string(),
                run: Rc::new(move || {
                    let Some(this) = weak.upgrade() else { return };
                    match this.current() {
                        Some(session) => {
                            session.input.set_text(&text);
                            session.input.grab_focus();
                        }
                        None => this.toast("Open a world's tab first: commands go in its input box"),
                    }
                }),
            });
        }
        palette::show(&self.window, "Command Palette", "Search commands, worlds and / commands", items);
    }

    /// Searches the tab's input history, newest first. The chosen line goes in the input box.
    fn show_history(&self, session: &Rc<SessionView>) {
        let mut seen = std::collections::HashSet::new();
        let items: Vec<Item> = session
            .input
            .history()
            .into_iter()
            .rev()
            .filter(|line| seen.insert(line.clone()))
            .map(|line| {
                let input = Rc::downgrade(&session.input);
                Item {
                    group: "History",
                    label: line.replace('\n', " ⏎ "),
                    detail: String::new(),
                    run: Rc::new(move || {
                        if let Some(input) = input.upgrade() {
                            input.set_text(&line);
                            input.grab_focus();
                        }
                    }),
                }
            })
            .collect();
        if items.is_empty() {
            return self.toast("Nothing has been sent from this tab yet");
        }
        palette::show(&self.window, "Input History", "Search what you've sent", items);
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name(crate::APP_NAME)
            .application_icon("network-server-symbolic")
            .version(env!("CARGO_PKG_VERSION"))
            .comments("A MU* client, inspired by BeipMU.")
            .license_type(gtk::License::MitX11)
            .build();
        about.present(Some(&self.window));
    }
}

#[cfg(test)]
mod tests {
    use familiar_engine::keymap::{self, Platform, Scope};

    /// Every window command in the keymap has an arm in `MainWindow::run`. (Read from the
    /// source, as the window can't be built without a display.)
    #[test]
    fn every_window_action_is_handled() {
        let source = include_str!("window.rs");
        let run = &source[source.find("fn run(").unwrap()..source.find("fn reload_sessions").unwrap()];
        for action in keymap::actions(Platform::Linux).filter(|a| a.scope == Scope::Window) {
            let handled = run.contains(&format!("\"{}\"", action.id)) || action.id.strip_prefix("tab-").is_some_and(|n| n.parse::<u8>().is_ok());
            assert!(handled, "{} isn't handled", action.id);
        }
        let editor = include_str!("code_editor.rs");
        for action in keymap::actions(Platform::Linux).filter(|a| a.scope == Scope::Editor) {
            assert!(editor.contains(&format!("Some(\"{}\")", action.id)), "{} isn't handled in the editor", action.id);
        }
    }
}
