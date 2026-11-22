//! One connection tab. The [`Engine`] does the work; this renders its [`Output`]s and forwards
//! input to it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gio, glib};
use mu_ansi::Palette;
use mu_config::{Character, World};
use mu_ansi::StyledLine;
use mu_engine::{Engine, Output, Status, Target, WebViewCommand};

use crate::app::AppState;
use crate::input::InputView;
use crate::media_player::MediaPlayer;
use crate::output::OutputView;
use crate::stats_pane::StatsPane;

/// The most events handled in one go before letting GTK redraw.
const BATCH_LIMIT: usize = 500;

pub struct SessionView {
    pub root: gtk::Paned,
    pub output: Rc<OutputView>,
    pub input: Rc<InputView>,
    state: Rc<AppState>,
    engine: RefCell<Engine>,
    page: RefCell<Option<adw::TabPage>>,
    /// Incremented on each connect, so events from an old connection are dropped.
    generation: Cell<u64>,
    /// Why the connection closed, for the tab icon. None while connected.
    disconnected: RefCell<Option<String>>,
    /// An important trigger fired while the tab wasn't selected.
    important: Cell<bool>,
    /// The side dock: spawn windows, stat panes and web views, as tabs.
    side: gtk::Notebook,
    spawns: RefCell<HashMap<String, Rc<OutputView>>>,
    stats: RefCell<HashMap<String, StatsPane>>,
    /// MCP status text, shown under the output.
    status: gtk::Label,
    sounds: RefCell<Vec<gtk::MediaFile>>,
    media: Rc<MediaPlayer>,
    on_open_uri: RefCell<Option<Box<dyn Fn(&str)>>>,
    on_logging_changed: RefCell<Option<Box<dyn Fn()>>>,
}

impl SessionView {
    pub fn new(state: Rc<AppState>, world: World, character: Option<Character>) -> Rc<Self> {
        let (engine, notices) = Engine::new(&state.config.borrow(), world, character);
        let settings = state.config.borrow().settings.clone();
        let output = OutputView::new(Palette::default(), settings.scrollback_lines);
        let input = InputView::new(settings.input_history_size);

        let status = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .margin_start(10)
            .margin_end(10)
            .margin_top(3)
            .margin_bottom(3)
            .css_classes(["caption", "dim-label"])
            .visible(false)
            .build();
        let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
        main.append(&output.scrolled);
        main.append(&status);
        main.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        main.append(&input.root);

        let side = gtk::Notebook::builder().scrollable(true).visible(false).build();
        let root = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .start_child(&main)
            .end_child(&side)
            .resize_start_child(true)
            .resize_end_child(false)
            .shrink_end_child(false)
            .build();

        let this = Rc::new(Self {
            root,
            output,
            input,
            state,
            engine: RefCell::new(engine),
            page: RefCell::default(),
            generation: Cell::new(0),
            disconnected: RefCell::default(),
            important: Cell::new(false),
            side,
            spawns: RefCell::default(),
            stats: RefCell::default(),
            status,
            sounds: RefCell::default(),
            media: Rc::default(),
            on_open_uri: RefCell::default(),
            on_logging_changed: RefCell::default(),
        });
        this.setup();
        this.apply(notices);
        this
    }

    pub fn title(&self) -> String {
        self.engine.borrow().title()
    }

    pub fn set_page(&self, page: &adw::TabPage) {
        let world = self.engine.borrow().world().clone();
        page.set_title(&self.title());
        page.set_tooltip(&format!("{}:{}{}", world.host, world.port, if world.tls { " (TLS)" } else { "" }));
        *self.page.borrow_mut() = Some(page.clone());
    }

    /// Called for links the engine doesn't handle itself (http and so on).
    pub fn connect_open_uri<F: Fn(&str) + 'static>(&self, f: F) {
        *self.on_open_uri.borrow_mut() = Some(Box::new(f));
    }

    pub fn connect_logging_changed<F: Fn() + 'static>(&self, f: F) {
        *self.on_logging_changed.borrow_mut() = Some(Box::new(f));
    }

    pub fn is_logging(&self) -> bool {
        self.engine.borrow().is_logging()
    }

    pub fn scopes(&self) -> Vec<mu_config::Scope> {
        self.engine.borrow().scopes().to_vec()
    }

    /// Re-reads rules and settings after the config changed.
    pub fn reload(self: &Rc<Self>) {
        let out = self.engine.borrow_mut().reload(&self.state.config.borrow());
        self.apply(out);
    }

    fn setup(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.input.connect_submit(move |text, secret| {
            if let Some(this) = weak.upgrade() {
                this.output.scroll_to_end();
                let out = this.engine.borrow_mut().submit(&text, secret);
                this.apply(out);
            }
        });

        let output = Rc::downgrade(&self.output);
        self.input.connect_page(move |up| {
            if let Some(output) = output.upgrade() {
                output.scroll_page(up);
            }
        });

        let weak = Rc::downgrade(self);
        self.input.connect_key(move |key, state| weak.upgrade().is_some_and(|this| this.run_macro(key, state)));

        let weak = Rc::downgrade(self);
        self.output.connect_link(move |uri| {
            if let Some(this) = weak.upgrade() {
                this.follow_link(uri);
            }
        });

        // Typing while the output has focus goes to the input. Macros work there too.
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(this) = weak.upgrade() else { return glib::Propagation::Proceed };
            if this.run_macro(key, state) {
                return glib::Propagation::Stop;
            }
            if state.intersects(gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::ALT_MASK) {
                return glib::Propagation::Proceed;
            }
            if let Some(c) = key.to_unicode().filter(|c| !c.is_control()) {
                this.input.insert(&c.to_string());
                glib::Propagation::Stop
            } else if matches!(key, gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter) {
                this.input.grab_focus();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        self.output.view.add_controller(keys);

        // Send NAWS when the output area changes size. A tick callback is the simplest way to
        // notice size changes on a GTK4 widget, and it only runs while the tab is visible.
        let weak: Weak<Self> = Rc::downgrade(self);
        self.output.view.add_tick_callback(move |_, _| {
            let Some(this) = weak.upgrade() else { return glib::ControlFlow::Break };
            if let Some((w, h)) = this.output.grid_size() {
                this.engine.borrow_mut().set_window_size(w, h);
            }
            glib::ControlFlow::Continue
        });
    }

    fn run_macro(self: &Rc<Self>, key: gtk::gdk::Key, state: gtk::gdk::ModifierType) -> bool {
        let Some(chord) = crate::keys::chord(key, state) else { return false };
        let Some(command) = self.engine.borrow().macro_for(&chord) else { return false };
        self.output.scroll_to_end();
        let out = self.engine.borrow_mut().submit(&command, false);
        self.apply(out);
        true
    }

    fn follow_link(self: &Rc<Self>, uri: &str) {
        let out = self.engine.borrow_mut().follow_link(uri);
        self.apply(out);
    }

    pub fn connect(self: &Rc<Self>) {
        if self.engine.borrow().is_connected() {
            return;
        }
        let (mut events, out) = {
            let _guard = crate::runtime().enter();
            self.engine.borrow_mut().connect()
        };
        self.apply(out);
        let generation = self.generation.get() + 1;
        self.generation.set(generation);

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Some(event) = events.recv().await {
                let Some(this) = weak.upgrade() else { return };
                if this.generation.get() != generation {
                    return;
                }
                let mut done = matches!(event, mu_net::SessionEvent::Disconnected { .. });
                let out = this.engine.borrow_mut().handle_event(event);
                this.apply(out);
                // Handle whatever else has already arrived before letting GTK redraw.
                for _ in 0..BATCH_LIMIT {
                    if done {
                        break;
                    }
                    let Ok(event) = events.try_recv() else { break };
                    done = matches!(event, mu_net::SessionEvent::Disconnected { .. });
                    let out = this.engine.borrow_mut().handle_event(event);
                    this.apply(out);
                }
                if done {
                    return;
                }
            }
        });
    }

    pub fn disconnect(&self) {
        self.engine.borrow_mut().disconnect();
    }

    pub fn toggle_log(self: &Rc<Self>) {
        let out = self.engine.borrow_mut().toggle_log();
        self.apply(out);
    }

    pub fn clear(&self) {
        self.output.clear();
    }

    fn apply(self: &Rc<Self>, outputs: Vec<Output>) {
        for output in outputs {
            match output {
                Output::Line { line, target: Target::Main } => self.output.append_line(&line),
                Output::Line { line, target: Target::Spawn(name) } => self.spawn(&name).append_line(&line),
                Output::Prompt { line, terminated } => self.output.set_prompt(&line, terminated),
                Output::Notice(text) => self.output.notice(&text),
                Output::Echo(text) => self.output.echo(&text),
                Output::Bell => self.output.view.error_bell(),
                Output::PasswordMode(on) => self.input.set_password_mode(on),
                Output::Status(status) => self.set_status(&status),
                Output::Activity { important } => self.mark_activity(important),
                Output::Notify { title, body } => {
                    let notification = gio::Notification::new(&title);
                    notification.set_body(Some(&body));
                    if let Some(app) = gio::Application::default() {
                        app.send_notification(None, &notification);
                    }
                }
                Output::PlaySound(path) => self.play_sound(&path),
                Output::OpenUri(uri) => {
                    if let Some(f) = self.on_open_uri.borrow().as_ref() {
                        f(&uri);
                    }
                }
                Output::SetInput(text) => {
                    self.input.set_text(&text);
                    self.input.grab_focus();
                }
                Output::ClearOutput => self.output.clear(),
                Output::Reconnect => self.connect(),
                Output::VariablesChanged { scope, variables } => {
                    if let Some(rules) = self.state.config.borrow_mut().rules_mut(&scope) {
                        rules.variables = variables;
                    }
                    if let Err(e) = self.state.save() {
                        self.output.notice(&e);
                    }
                }
                Output::LoggingChanged(_) => {
                    if let Some(f) = self.on_logging_changed.borrow().as_ref() {
                        f();
                    }
                }
                Output::Stats { pane, data } => self.update_stats(&pane, data.as_ref()),
                Output::Media(command) => {
                    let weak = Rc::downgrade(self);
                    let notice: Rc<dyn Fn(String)> = Rc::new(move |text| {
                        if let Some(this) = weak.upgrade() {
                            this.output.notice(&text);
                        }
                    });
                    self.media.run(command, notice);
                }
                Output::StatusText(text) => {
                    self.status.set_visible(!text.is_empty());
                    self.status.set_label(&text);
                }
                Output::Edit(request) => {
                    let parent = self.root.root().and_downcast::<gtk::Window>();
                    let weak = Rc::downgrade(self);
                    crate::edit_window::show(parent.as_ref(), &request.clone(), move |text| {
                        if let Some(this) = weak.upgrade() {
                            let out = this.engine.borrow_mut().save_edit(&request, &text);
                            this.apply(out);
                        }
                    });
                }
                Output::WebView(command) => self.webview(command),
                Output::Gmcp { .. } | Output::Received(_) | Output::Sent(_) => {}
            }
        }
    }

    /// The spawn window with this name, created (with its tab) on first use.
    fn spawn(self: &Rc<Self>, name: &str) -> Rc<OutputView> {
        if let Some(view) = self.spawns.borrow().get(name) {
            return view.clone();
        }
        let settings = self.state.config.borrow().settings.clone();
        let view = OutputView::new(Palette::default(), settings.scrollback_lines);
        let weak = Rc::downgrade(self);
        view.connect_link(move |uri| {
            if let Some(this) = weak.upgrade() {
                this.follow_link(uri);
            }
        });
        self.add_side_tab(&view.scrolled, name);
        self.spawns.borrow_mut().insert(name.to_string(), view.clone());
        view
    }

    /// Adds a tab to the side dock, showing the dock if it was hidden.
    fn add_side_tab(&self, widget: &impl IsA<gtk::Widget>, title: &str) {
        widget.set_size_request(260, -1);
        self.side.append_page(widget, Some(&gtk::Label::new(Some(title))));
        if !self.side.is_visible() {
            self.side.set_visible(true);
            // Give the main output about two thirds of the width.
            let width = self.root.width();
            if width > 0 {
                self.root.set_position(width * 2 / 3);
            }
        }
    }

    fn remove_side_tab(&self, widget: &impl IsA<gtk::Widget>) {
        if let Some(n) = self.side.page_num(widget) {
            self.side.remove_page(Some(n));
        }
        if self.side.n_pages() == 0 {
            self.side.set_visible(false);
        }
    }

    fn update_stats(&self, name: &str, data: Option<&mu_engine::stats::StatPane>) {
        let mut stats = self.stats.borrow_mut();
        match data {
            Some(data) => {
                if !stats.contains_key(name) {
                    let pane = StatsPane::new();
                    self.add_side_tab(&pane.root, name);
                    stats.insert(name.to_string(), pane);
                }
                stats[name].update(data);
            }
            None => {
                if let Some(pane) = stats.remove(name) {
                    self.remove_side_tab(&pane.root);
                }
            }
        }
    }

    #[cfg(not(feature = "webview"))]
    fn webview(&self, command: WebViewCommand) {
        let WebViewCommand::Open(open) = command else { return };
        let who = if open.from_server { "The server wants" } else { "You asked" };
        self.output.notice(&format!(
            "{who} to open a web view, but this build doesn't include them. Install webkitgtk-6.0 and build with --features webview."
        ));
        if let Some(url) = open.url {
            let mut line = StyledLine::plain(format!("Open {url} in your browser instead"));
            line.linkify();
            self.output.append_line(&line);
        }
    }

    fn play_sound(&self, path: &str) {
        let path = match path.strip_prefix("~/") {
            Some(rest) => glib::home_dir().join(rest),
            None => PathBuf::from(path),
        };
        if !path.is_file() {
            self.output.notice(&format!("Sound file not found: {}", path.display()));
            return;
        }
        let mut sounds = self.sounds.borrow_mut();
        sounds.retain(|m| !m.is_ended() && m.error().is_none());
        let media = gtk::MediaFile::for_filename(&path);
        let output = Rc::downgrade(&self.output);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        media.connect_error_notify(move |media| {
            if let (Some(output), Some(e)) = (output.upgrade(), media.error()) {
                output.notice(&crate::media_player::describe_error(&name, &e));
            }
        });
        media.play();
        sounds.push(media);
    }

    /// Shows connection state on the tab: a spinner while connecting, an icon once disconnected.
    fn set_status(&self, status: &Status) {
        if let Some(page) = self.page.borrow().as_ref() {
            page.set_loading(*status == Status::Connecting);
        }
        *self.disconnected.borrow_mut() = match status {
            Status::Disconnected(reason) => Some(reason.clone().unwrap_or_else(|| "Disconnected".into())),
            _ => None,
        };
        self.update_indicator();
    }

    fn update_indicator(&self) {
        let Some(page) = self.page.borrow().clone() else { return };
        if let Some(reason) = self.disconnected.borrow().as_deref() {
            page.set_indicator_icon(Some(&gio::ThemedIcon::new("network-offline-symbolic")));
            page.set_indicator_tooltip(reason);
        } else if self.important.get() {
            page.set_indicator_icon(Some(&gio::ThemedIcon::new("starred-symbolic")));
            page.set_indicator_tooltip("An important trigger fired");
        } else {
            page.set_indicator_icon(None::<&gio::Icon>);
        }
    }

    fn mark_activity(&self, important: bool) {
        let Some(page) = self.page.borrow().clone() else { return };
        if page.is_selected() {
            return;
        }
        page.set_needs_attention(true);
        if important && !self.important.get() {
            self.important.set(true);
            self.update_indicator();
        }
    }

    /// The user switched to this tab.
    pub fn seen(&self) {
        if let Some(page) = self.page.borrow().as_ref() {
            page.set_needs_attention(false);
        }
        if self.important.replace(false) {
            self.update_indicator();
        }
    }
}
