//! The main window: a tab per connection.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use mu_config::{Character, World};

use crate::app::AppState;
use crate::connect_dialog;
use crate::rules_dialog;
use crate::session::SessionView;

pub struct MainWindow {
    pub window: adw::ApplicationWindow,
    tab_view: adw::TabView,
    stack: gtk::Stack,
    toasts: adw::ToastOverlay,
    state: Rc<AppState>,
    sessions: RefCell<Vec<(adw::TabPage, Rc<SessionView>)>>,
}

impl MainWindow {
    pub fn new(app: &adw::Application, state: Rc<AppState>) -> Rc<Self> {
        let tab_view = adw::TabView::new();
        let tab_bar = adw::TabBar::builder().view(&tab_view).build();

        let connect_button = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("Connect (Ctrl+O)")
            .action_name("win.connect")
            .build();
        let menu = gio::Menu::new();
        let session_section = gio::Menu::new();
        session_section.append(Some("Reconnect"), Some("win.reconnect"));
        session_section.append(Some("Disconnect"), Some("win.disconnect"));
        session_section.append(Some("Clear Output"), Some("win.clear"));
        menu.append_section(None, &session_section);
        let rules_section = gio::Menu::new();
        rules_section.append(Some("Triggers, Aliases & Macros"), Some("win.rules"));
        menu.append_section(None, &rules_section);
        let log_section = gio::Menu::new();
        log_section.append(Some("Log to File"), Some("win.toggle-log"));
        log_section.append(Some("Open Log Folder"), Some("win.open-logs"));
        menu.append_section(None, &log_section);
        let app_section = gio::Menu::new();
        app_section.append(Some("Keyboard Shortcuts"), Some("win.shortcuts"));
        app_section.append(Some(&format!("About {}", crate::APP_NAME)), Some("win.about"));
        menu.append_section(None, &app_section);
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .tooltip_text("Main Menu")
            .primary(true)
            .build();

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

        let this = Rc::new(Self { window, tab_view, stack, toasts, state, sessions: RefCell::default() });
        this.setup_actions(app);
        this.setup_tabs();
        // Closing the window quits, so save every session's map first.
        let weak = Rc::downgrade(&this);
        this.window.connect_close_request(move |_| {
            if let Some(this) = weak.upgrade() {
                for (_, session) in this.sessions.borrow().iter() {
                    session.save_map();
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
                        session.save_map();
                        session.disconnect();
                        if session.is_logging() {
                            session.toggle_log();
                        }
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
            gtk::UriLauncher::new(uri).launch(Some(&window), None::<&gio::Cancellable>, |_| {});
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

    fn setup_actions(self: &Rc<Self>, app: &adw::Application) {
        let add = |name: &str, accels: &[&str], f: Box<dyn Fn(&Rc<Self>)>| {
            let action = gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(this) = weak.upgrade() {
                    f(&this);
                }
            });
            self.window.add_action(&action);
            if !accels.is_empty() {
                app.set_accels_for_action(&format!("win.{name}"), accels);
            }
        };

        add(
            "connect",
            &["<Control>o"],
            Box::new(|this| {
                let weak = Rc::downgrade(this);
                connect_dialog::show(&this.window, this.state.clone(), move |world, character| {
                    if let Some(this) = weak.upgrade() {
                        this.open_session(world, character);
                    }
                });
            }),
        );
        add(
            "reconnect",
            &["<Control>r"],
            Box::new(|this| {
                if let Some(s) = this.current() {
                    s.connect();
                }
            }),
        );
        add(
            "disconnect",
            &["<Control><Shift>d"],
            Box::new(|this| {
                if let Some(s) = this.current() {
                    s.disconnect();
                }
            }),
        );
        add(
            "clear",
            &["<Control><Shift>l"],
            Box::new(|this| {
                if let Some(s) = this.current() {
                    s.clear();
                }
            }),
        );
        add(
            "close-tab",
            &["<Control>w"],
            Box::new(|this| {
                if let Some(page) = this.tab_view.selected_page() {
                    this.tab_view.close_page(&page);
                }
            }),
        );
        add(
            "open-logs",
            &[],
            Box::new(|this| {
                let dir = mu_config::log_dir();
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    this.toast(&format!("Couldn't create {}: {e}", dir.display()));
                    return;
                }
                let file = gio::File::for_path(&dir);
                gtk::FileLauncher::new(Some(&file)).launch(Some(&this.window), None::<&gio::Cancellable>, |_| {});
            }),
        );
        add(
            "rules",
            &["<Control>t"],
            Box::new(|this| {
                let current = this.current().map(|s| s.scopes()).unwrap_or_default();
                let weak = Rc::downgrade(this);
                rules_dialog::show(&this.window, this.state.clone(), current.first().cloned(), move || {
                    if let Some(this) = weak.upgrade() {
                        for (_, session) in this.sessions.borrow().iter() {
                            session.reload();
                        }
                    }
                });
            }),
        );
        add(
            "stop-speech",
            &["<Control>period"],
            Box::new(|this| this.state.speaker.stop()),
        );
        add("about", &[], Box::new(|this| this.show_about()));
        add("shortcuts", &["<Control>question"], Box::new(|this| this.show_shortcuts()));

        // Stateful so the menu shows a check mark while logging.
        let log = gio::SimpleAction::new_stateful("toggle-log", None, &false.to_variant());
        let weak = Rc::downgrade(self);
        log.connect_activate(move |_, _| {
            let Some(this) = weak.upgrade() else { return };
            let Some(session) = this.current() else { return };
            session.toggle_log();
            this.update_log_state();
        });
        self.window.add_action(&log);
        app.set_accels_for_action("win.toggle-log", &["<Control><Shift>g"]);
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name(crate::APP_NAME)
            .application_icon("network-server-symbolic")
            .version(env!("CARGO_PKG_VERSION"))
            .comments("A MU* client for Linux, inspired by BeipMU.")
            .license_type(gtk::License::MitX11)
            .build();
        about.present(Some(&self.window));
    }

    fn show_shortcuts(&self) {
        let rows = [
            ("Ctrl+O", "Connect"),
            ("Ctrl+R", "Reconnect"),
            ("Ctrl+Shift+D", "Disconnect"),
            ("Ctrl+W", "Close tab"),
            ("Ctrl+Shift+L", "Clear output"),
            ("Ctrl+Shift+G", "Log to file"),
            ("Ctrl+T", "Triggers, aliases and macros"),
            ("Ctrl+.", "Stop speaking"),
            ("Enter", "Send"),
            ("Shift+Enter", "New line in input"),
            ("Up / Down", "Input history (Ctrl+Up/Down in multi-line input)"),
            ("Page Up / Page Down", "Scroll output"),
            ("Escape", "Clear input"),
            ("/help", "List client commands (/set, /log, /clear, ...)"),
        ];
        let group = adw::PreferencesGroup::new();
        for (keys, what) in rows {
            let row = adw::ActionRow::builder().title(what).use_markup(false).build();
            row.add_suffix(&gtk::Label::builder().label(keys).css_classes(["dim-label"]).build());
            group.add(&row);
        }
        let page = adw::PreferencesPage::new();
        page.add(&group);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&page));
        let dialog = adw::Dialog::builder()
            .title("Keyboard Shortcuts")
            .content_width(460)
            .content_height(560)
            .child(&toolbar)
            .build();
        dialog.present(Some(&self.window));
    }
}
