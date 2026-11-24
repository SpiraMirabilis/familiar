//! One connection tab. The [`Engine`] does the work; this renders its [`Output`]s and forwards
//! input to it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::Instant;

use adw::prelude::*;
use gtk::{gio, glib};
use familiar_ansi::Palette;
use familiar_config::{Character, World};
use familiar_engine::{Engine, Output, Status, Target, WebViewCommand};

use crate::app::AppState;
use crate::input::InputView;
use crate::media_player::MediaPlayer;
use crate::output::OutputView;
use crate::stats_pane::StatsPane;
#[cfg(feature = "webview")]
use crate::webview::PageEvent;

/// Without web views there are no pages to send events to.
#[cfg(not(feature = "webview"))]
#[allow(dead_code)]
enum PageEvent<'a> {
    Gmcp { package: &'a str, data: &'a str },
    Receive(&'a str),
    Send(&'a str),
    Display(&'a str),
}

/// Where clicking an avatar goes; shared with its click handler so beip.ids can change it.
type AvatarLink = Rc<RefCell<Option<String>>>;

/// The most events handled in one go before letting GTK redraw.
const BATCH_LIMIT: usize = 500;

pub struct SessionView {
    pub root: gtk::Paned,
    pub output: Rc<OutputView>,
    pub input: Rc<InputView>,
    state: Rc<AppState>,
    engine: Rc<RefCell<Engine>>,
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
    sounds: RefCell<Vec<crate::media_player::Clip>>,
    media: Rc<MediaPlayer>,
    map: RefCell<Option<Rc<crate::map_view::MapView>>>,
    /// Open puppet tabs, by puppet name.
    puppets: RefCell<HashMap<String, (Rc<crate::puppet_view::PuppetView>, adw::TabPage)>>,
    /// Adds a tab to the window (set by the window): (content, title) -> page.
    on_open_tab: RefCell<Option<Box<dyn Fn(&gtk::Widget, &str) -> adw::TabPage>>>,
    tilemaps: RefCell<HashMap<String, Rc<crate::tilemap_view::TilemapView>>>,
    images: Rc<crate::images::ImageCache>,
    /// Avatar pictures shown for each beip.line.id, with their click URLs, to update when
    /// beip.ids changes them.
    avatars: RefCell<HashMap<String, Vec<(glib::WeakRef<gtk::Picture>, AvatarLink)>>>,
    /// Open code editors by reference ("" for the latest scratch editor), for compile errors.
    editors: RefCell<HashMap<String, Weak<crate::code_editor::CodeEditor>>>,
    /// The pending script timer and when it fires.
    timer: RefCell<Option<(glib::SourceId, Instant)>>,
    #[cfg(feature = "webview")]
    webviews: RefCell<Vec<crate::webview::WebPane>>,
    /// The CSS class for this world's font (see `app::world_class`), for every view it opens.
    font_class: String,
    on_open_uri: RefCell<Option<Box<dyn Fn(&str)>>>,
    on_logging_changed: RefCell<Option<Box<dyn Fn()>>>,
}

impl SessionView {
    pub fn new(state: Rc<AppState>, world: World, character: Option<Character>) -> Rc<Self> {
        let font_class = crate::app::world_class(&world.name);
        let (mut engine, notices) = Engine::new(&state.config.borrow(), world, character);
        engine.set_webviews_supported(cfg!(feature = "webview"));
        let settings = state.config.borrow().settings.clone();
        let output = OutputView::new(Palette::default(), settings.scrollback_lines);
        let input = InputView::new(settings.input_history_size, settings.spellcheck);
        output.view.add_css_class(&font_class);
        input.view.add_css_class(&font_class);

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
            engine: Rc::new(RefCell::new(engine)),
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
            timer: RefCell::default(),
            editors: RefCell::default(),
            map: RefCell::default(),
            puppets: RefCell::default(),
            on_open_tab: RefCell::default(),
            tilemaps: RefCell::default(),
            images: Rc::default(),
            avatars: RefCell::default(),
            #[cfg(feature = "webview")]
            webviews: RefCell::default(),
            font_class,
            on_open_uri: RefCell::default(),
            on_logging_changed: RefCell::default(),
        });
        this.setup();
        this.apply(notices);
        this.start_scripting();
        if !this.engine.borrow().map().is_empty() {
            this.show_map(false);
        }
        // Save the map now and then; it's also saved on disconnect and when the tab closes.
        let weak = Rc::downgrade(&this);
        glib::timeout_add_seconds_local(30, move || {
            let Some(this) = weak.upgrade() else { return glib::ControlFlow::Break };
            this.save_map();
            glib::ControlFlow::Continue
        });
        this
    }

    pub fn save_map(&self) {
        if let Err(e) = self.engine.borrow_mut().save_map() {
            self.output.notice(&format!("Couldn't save the map: {e}"));
        }
    }

    /// Adds the map tab if needed, and switches to it if `select`.
    fn show_map(self: &Rc<Self>, select: bool) {
        let existing = self.map.borrow().clone();
        let view = match existing {
            Some(view) => view,
            None => {
                let weak = Rc::downgrade(self);
                let view = crate::map_view::MapView::new(self.engine.clone(), move |out| {
                    if let Some(this) = weak.upgrade() {
                        this.apply(out);
                    }
                });
                self.add_side_tab(&view.root, "Map");
                *self.map.borrow_mut() = Some(view.clone());
                view
            }
        };
        if select && let Some(n) = self.side.page_num(&view.root) {
            self.side.set_current_page(Some(n));
        }
        view.refresh();
    }

    /// Starts a fresh JavaScript runtime and runs the startup scripts.
    fn start_scripting(self: &Rc<Self>) {
        let host = match familiar_script::JsHost::new() {
            Ok(host) => host,
            Err(e) => return self.output.notice(&format!("Scripting is unavailable: {e}")),
        };
        let mut out = self.engine.borrow_mut().set_script_host(Box::new(host));
        let paths = self.engine.borrow().startup_scripts();
        for path in paths {
            out.push(Output::Notice(format!("Running {}", familiar_config::paths::display(&path))));
            out.extend(self.engine.borrow_mut().load_script(&path));
        }
        self.apply(out);
    }

    /// Makes sure a GLib timer fires when the next script timer is due.
    fn schedule_timer(self: &Rc<Self>) {
        let deadline = self.engine.borrow_mut().next_deadline();
        let mut timer = self.timer.borrow_mut();
        if timer.as_ref().map(|(_, at)| *at) == deadline {
            return;
        }
        if let Some((source, _)) = timer.take() {
            source.remove();
        }
        let Some(deadline) = deadline else { return };
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_local_once(deadline.saturating_duration_since(Instant::now()), move || {
            let Some(this) = weak.upgrade() else { return };
            // This source has fired, so it mustn't be removed again.
            this.timer.borrow_mut().take();
            let out = this.engine.borrow_mut().tick(Instant::now());
            this.apply(out);
        });
        *timer = Some((source, deadline));
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

    pub fn scopes(&self) -> Vec<familiar_config::Scope> {
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
        let Some(out) = self.engine.borrow_mut().run_macro(&chord) else { return false };
        self.output.scroll_to_end();
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
                let mut done = matches!(event, familiar_net::SessionEvent::Disconnected { .. });
                let out = this.engine.borrow_mut().handle_event(event);
                this.apply(out);
                // Handle whatever else has already arrived before letting GTK redraw.
                for _ in 0..BATCH_LIMIT {
                    if done {
                        break;
                    }
                    let Ok(event) = events.try_recv() else { break };
                    done = matches!(event, familiar_net::SessionEvent::Disconnected { .. });
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
        self.apply_outputs(outputs);
        if self.engine.borrow().has_scripting() {
            self.schedule_timer();
        }
    }

    fn apply_outputs(self: &Rc<Self>, outputs: Vec<Output>) {
        for output in outputs {
            match output {
                Output::Line { line, target: Target::Main } => {
                    self.output.append_line(&line);
                    self.to_pages(|| PageEvent::Display(&line.text));
                }
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
                Output::ReloadScripts => {
                    if let Some((source, _)) = self.timer.borrow_mut().take() {
                        source.remove();
                    }
                    self.output.notice("Restarting scripting");
                    self.start_scripting();
                }
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
                Output::MapChanged => {
                    let view = self.map.borrow().clone();
                    match view {
                        Some(view) => view.refresh(),
                        None if !self.engine.borrow().map().is_empty() => self.show_map(false),
                        None => {}
                    }
                }
                Output::ShowMap => self.show_map(true),
                Output::OpenPuppet { name, focus } => self.open_puppet(&name, focus),
                Output::PuppetLine { name, line } => {
                    let open = self.puppets.borrow().get(&name).cloned();
                    if let Some((view, page)) = open {
                        view.output.append_line(&line);
                        if !page.is_selected() {
                            page.set_needs_attention(true);
                        }
                    }
                }
                Output::PuppetEcho { name, text } => {
                    if let Some((view, _)) = self.puppets.borrow().get(&name) {
                        view.output.echo(&text);
                    }
                }
                Output::Speak(text) => {
                    self.state.speaker.say(&text);
                    self.report_speech_error();
                }
                Output::SpeechStop => self.state.speaker.stop(),
                Output::LineAvatar(avatar) => {
                    let picture = self.avatar_picture(&avatar);
                    self.output.set_next_prefix(&picture);
                }
                Output::AvatarUpdated(avatar) => {
                    let id = avatar.id.clone().unwrap_or_default();
                    let shown: Vec<(gtk::Picture, AvatarLink)> = self
                        .avatars
                        .borrow()
                        .get(&id)
                        .map(|list| list.iter().filter_map(|(w, link)| w.upgrade().map(|p| (p, link.clone()))).collect())
                        .unwrap_or_default();
                    for (picture, link) in shown {
                        self.show_avatar(&picture, &link, &avatar);
                    }
                }
                Output::InlineImage { url } => self.inline_image(&url),
                Output::SettingsChanged(settings) => {
                    self.state.config.borrow_mut().settings = settings;
                    if let Err(e) = self.state.save() {
                        self.output.notice(&e);
                    }
                }
                Output::Tilemap { name, map } => {
                    let existing = self.tilemaps.borrow().get(&name).cloned();
                    let view = existing.unwrap_or_else(|| {
                        let view = crate::tilemap_view::TilemapView::new();
                        self.add_side_tab(&view.root, &name);
                        if let Some(n) = self.side.page_num(&view.root) {
                            self.side.set_current_page(Some(n));
                        }
                        self.tilemaps.borrow_mut().insert(name.clone(), view.clone());
                        view
                    });
                    let weak = Rc::downgrade(self);
                    view.update(map, Rc::new(move |text: String| {
                        if let Some(this) = weak.upgrade() {
                            this.output.notice(&text);
                        }
                    }));
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
                Output::Edit(request) => self.open_editor(request),
                Output::CompileError { reference, line, message } => {
                    let editor = self.editors.borrow().get(&reference).and_then(Weak::upgrade);
                    if let Some(editor) = editor {
                        editor.add_error(line, &message);
                    }
                }
                Output::WebView(command) => self.webview(command),
                Output::Gmcp { package, data } => self.to_pages(|| PageEvent::Gmcp { package: &package, data: &data }),
                Output::Received(text) => self.to_pages(|| PageEvent::Receive(&text)),
                Output::Sent(text) => self.to_pages(|| PageEvent::Send(&text)),
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
        view.view.add_css_class(&self.font_class);
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

    fn open_editor(self: &Rc<Self>, request: familiar_engine::EditRequest) {
        // The server sending the same program again reuses its editor.
        if !request.is_local() {
            let existing = self.editors.borrow().get(&request.reference).and_then(Weak::upgrade);
            if let Some(editor) = existing {
                if !editor.is_modified() {
                    editor.load(&request.content);
                }
                editor.present();
                return;
            }
        }
        let parent = self.root.root().and_downcast::<gtk::Window>();
        let weak = Rc::downgrade(self);
        let sent = request.clone();
        let editor = crate::code_editor::CodeEditor::open(parent.as_ref(), &request, move |text| {
            if let Some(this) = weak.upgrade() {
                let out = this.engine.borrow_mut().save_edit(&sent, &text);
                this.apply(out);
            }
        });
        self.editors.borrow_mut().retain(|_, e| e.strong_count() > 0);
        self.editors.borrow_mut().insert(request.reference.clone(), Rc::downgrade(&editor));
        // Keep the editor alive until its window closes.
        let keep = RefCell::new(Some(editor.clone()));
        editor.window.connect_destroy(move |_| {
            keep.borrow_mut().take();
        });
    }

    pub fn connect_open_tab<F: Fn(&gtk::Widget, &str) -> adw::TabPage + 'static>(&self, f: F) {
        *self.on_open_tab.borrow_mut() = Some(Box::new(f));
    }

    fn report_speech_error(&self) {
        if let Some(e) = self.state.speaker.take_error() {
            self.output.notice(&e);
        }
    }

    fn open_puppet(self: &Rc<Self>, name: &str, focus: bool) {
        let existing = self.puppets.borrow().get(name).map(|(_, page)| page.clone());
        let page = match existing {
            Some(page) => page,
            None => {
                let settings = self.state.config.borrow().settings.clone();
                let submit = {
                    let weak = Rc::downgrade(self);
                    let name = name.to_string();
                    move |text: String| {
                        if let Some(this) = weak.upgrade() {
                            let out = this.engine.borrow_mut().puppet_submit(&name, &text);
                            this.apply(out);
                        }
                    }
                };
                let link = {
                    let weak = Rc::downgrade(self);
                    move |uri: &str| {
                        if let Some(this) = weak.upgrade() {
                            this.follow_link(uri);
                        }
                    }
                };
                let view = crate::puppet_view::PuppetView::new(&settings, submit, link);
                view.output.view.add_css_class(&self.font_class);
                view.input.view.add_css_class(&self.font_class);
                let title = format!("{name} ({})", self.title());
                let Some(page) = self.on_open_tab.borrow().as_ref().map(|f| f(view.root.upcast_ref(), &title)) else { return };
                page.set_icon(Some(&gtk::gio::ThemedIcon::new("avatar-default-symbolic")));
                self.puppets.borrow_mut().insert(name.to_string(), (view, page.clone()));
                page
            }
        };
        if focus && let Some(tabs) = page.child().ancestor(adw::TabView::static_type()).and_downcast::<adw::TabView>() {
            tabs.set_selected_page(&page);
            if let Some((view, _)) = self.puppets.borrow().get(name) {
                view.input.grab_focus();
            }
        }
    }

    /// The window closed one of this session's puppet tabs. Returns true if it was ours.
    pub fn puppet_tab_closed(&self, page: &adw::TabPage) -> bool {
        let name = self.puppets.borrow().iter().find(|(_, (_, p))| p == page).map(|(n, _)| n.clone());
        let Some(name) = name else { return false };
        self.puppets.borrow_mut().remove(&name);
        self.engine.borrow_mut().set_puppet_open(&name, false);
        true
    }

    /// Pages of open puppet tabs, so the window can close them with the session.
    pub fn puppet_pages(&self) -> Vec<adw::TabPage> {
        self.puppets.borrow().values().map(|(_, page)| page.clone()).collect()
    }

    /// A small picture for the start of a line. Clicking it opens the avatar's link.
    fn avatar_picture(self: &Rc<Self>, avatar: &familiar_engine::Avatar) -> gtk::Picture {
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .can_shrink(true)
            .width_request(22)
            .height_request(22)
            .valign(gtk::Align::Center)
            .build();
        let link: AvatarLink = Rc::default();
        let click = gtk::GestureClick::new();
        let weak = Rc::downgrade(self);
        {
            let link = link.clone();
            click.connect_released(move |_, _, _, _| {
                if let (Some(this), Some(url)) = (weak.upgrade(), link.borrow().clone()) {
                    this.follow_link(&url);
                }
            });
        }
        picture.add_controller(click);
        if let Some(id) = &avatar.id {
            let mut avatars = self.avatars.borrow_mut();
            let list = avatars.entry(id.clone()).or_default();
            list.retain(|(w, _)| w.upgrade().is_some());
            list.push((picture.downgrade(), link.clone()));
        }
        self.show_avatar(&picture, &link, avatar);
        picture
    }

    fn show_avatar(&self, picture: &gtk::Picture, link: &AvatarLink, avatar: &familiar_engine::Avatar) {
        picture.set_tooltip_text(avatar.hover.as_deref());
        *link.borrow_mut() = avatar.click_url.clone().or(avatar.url.clone());
        if let Some(url) = &avatar.url {
            let picture = picture.downgrade();
            self.images.get(url, move |texture| {
                if let Some(picture) = picture.upgrade() {
                    picture.set_paintable(Some(texture));
                }
            });
        }
    }

    /// A thumbnail under the line, at most 320 by 200, that opens the image when clicked.
    fn inline_image(self: &Rc<Self>, url: &str) {
        let picture = gtk::Picture::builder().content_fit(gtk::ContentFit::Contain).can_shrink(true).tooltip_text(url).build();
        let click = gtk::GestureClick::new();
        let weak = Rc::downgrade(self);
        let target = url.to_string();
        click.connect_released(move |_, _, _, _| {
            if let Some(this) = weak.upgrade() {
                this.follow_link(&target);
            }
        });
        picture.add_controller(click);
        // Reserve its place under the line now; the download may finish after more output.
        picture.set_visible(false);
        self.output.append_widget(&picture);
        let shown = picture.downgrade();
        self.images.get(url, move |texture| {
            let Some(picture) = shown.upgrade() else { return };
            let (w, h) = (texture.width() as f64, texture.height() as f64);
            let scale = (320.0 / w).min(200.0 / h).min(1.0);
            picture.set_size_request((w * scale) as i32, (h * scale) as i32);
            picture.set_paintable(Some(texture));
            picture.set_visible(true);
        });
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

    fn update_stats(&self, name: &str, data: Option<&familiar_engine::stats::StatPane>) {
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
    fn to_pages<'a>(&self, _event: impl FnOnce() -> PageEvent<'a>) {}

    #[cfg(feature = "webview")]
    fn to_pages<'a>(&self, event: impl Fn() -> PageEvent<'a>) {
        for pane in self.webviews.borrow().iter() {
            pane.deliver(event());
        }
    }

    #[cfg(feature = "webview")]
    fn webview(self: &Rc<Self>, command: WebViewCommand) {
        let open = match command {
            WebViewCommand::Close { id } => return self.close_webview(&id),
            WebViewCommand::Open(open) => open,
        };
        if !open.from_server {
            return self.open_webview(open);
        }
        let world = self.engine.borrow().world().name.clone();
        let saved = self.state.config.borrow().world(&world).map(|w| w.allow_webviews);
        match saved {
            Some(Some(true)) => self.open_webview(open),
            Some(Some(false)) => self.output.notice("Blocked a web view from the server (web views are off for this world)."),
            // Not asked yet, or a quick-connect world that can't remember the answer.
            Some(None) | None => self.ask_webview(open, &world, saved.is_some()),
        }
    }

    #[cfg(feature = "webview")]
    fn ask_webview(self: &Rc<Self>, open: familiar_engine::WebViewOpen, world: &str, can_remember: bool) {
        let target = open.url.clone().unwrap_or_else(|| "a page it sent".into());
        let dialog = adw::AlertDialog::new(
            Some("Open a Web View?"),
            Some(&format!(
                "{world} wants to open {target} inside the client. Pages it opens can send commands and read the output, like a script."
            )),
        );
        dialog.add_response("block", "Block");
        dialog.add_response("once", "Allow Once");
        if can_remember {
            dialog.add_response("always", "Always Allow");
            dialog.set_response_appearance("always", adw::ResponseAppearance::Suggested);
        }
        dialog.set_default_response(Some("once"));
        dialog.set_close_response("block");
        let weak = Rc::downgrade(self);
        let world = world.to_string();
        dialog.connect_response(None, move |_, response| {
            let Some(this) = weak.upgrade() else { return };
            match response {
                "once" => this.open_webview(open.clone()),
                "always" => {
                    if let Some(w) = this.state.config.borrow_mut().worlds.iter_mut().find(|w| w.name == world) {
                        w.allow_webviews = Some(true);
                    }
                    if let Err(e) = this.state.save() {
                        this.output.notice(&e);
                    }
                    this.open_webview(open.clone());
                }
                _ => this.output.notice("Blocked a web view from the server."),
            }
        });
        dialog.present(Some(&self.root));
    }

    #[cfg(feature = "webview")]
    fn open_webview(self: &Rc<Self>, open: familiar_engine::WebViewOpen) {
        // Same id: reuse the existing view.
        if let Some(pane) = self.webviews.borrow().iter().find(|p| p.id == open.id) {
            pane.load(&open);
            if let Some(window) = &pane.window {
                window.present();
            } else if let Some(n) = self.side.page_num(&pane.view) {
                self.side.set_current_page(Some(n));
            }
            return;
        }
        let weak = Rc::downgrade(self);
        let id = open.id.clone();
        let mut pane = crate::webview::WebPane::new(&open, move |message| {
            if let Some(this) = weak.upgrade() {
                this.on_page_message(&id, message);
            }
        });
        if open.dock.is_some() {
            self.add_side_tab(&pane.view, &open.id);
            if let Some(n) = self.side.page_num(&pane.view) {
                self.side.set_current_page(Some(n));
            }
        } else {
            let header = adw::HeaderBar::new();
            let toolbar = adw::ToolbarView::new();
            toolbar.add_top_bar(&header);
            toolbar.set_content(Some(&pane.view));
            let window = adw::Window::builder()
                .title(&open.id)
                .default_width(open.width.unwrap_or(900))
                .default_height(open.height.unwrap_or(700))
                .content(&toolbar)
                .build();
            window.set_transient_for(self.root.root().and_downcast::<gtk::Window>().as_ref());
            let weak = Rc::downgrade(self);
            let id = open.id.clone();
            window.connect_close_request(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.webviews.borrow_mut().retain(|p| p.id != id);
                }
                glib::Propagation::Proceed
            });
            window.present();
            pane.window = Some(window);
        }
        self.webviews.borrow_mut().push(pane);
    }

    #[cfg(feature = "webview")]
    fn close_webview(&self, id: &str) {
        let pane = {
            let mut panes = self.webviews.borrow_mut();
            let Some(i) = panes.iter().position(|p| p.id == id) else { return };
            panes.remove(i)
        };
        match &pane.window {
            Some(window) => window.close(),
            None => self.remove_side_tab(&pane.view),
        }
    }

    #[cfg(feature = "webview")]
    fn on_page_message(self: &Rc<Self>, id: &str, message: crate::webview::BridgeMessage) {
        use crate::webview::BridgeMessage;
        let out = match message {
            BridgeMessage::Send { text, aliases } => self.engine.borrow_mut().send_text(&text, aliases),
            BridgeMessage::Gmcp { package, data } => self.engine.borrow_mut().send_gmcp(&package, data.as_deref()),
            BridgeMessage::Receive(text) => self.engine.borrow_mut().inject_line(&text),
            BridgeMessage::Display(text) => {
                self.output.append_line(&familiar_ansi::AnsiParser::new().parse_line(&text));
                Vec::new()
            }
            BridgeMessage::Close => {
                self.close_webview(id);
                Vec::new()
            }
        };
        self.apply(out);
    }

    #[cfg(not(feature = "webview"))]
    fn webview(&self, command: WebViewCommand) {
        let WebViewCommand::Open(open) = command else { return };
        let who = if open.from_server { "The server wants" } else { "You asked" };
        self.output.notice(&format!(
            "{who} to open a web view, but this build doesn't include them. Install webkitgtk-6.0 and build with --features webview."
        ));
        if let Some(url) = open.url {
            let mut line = familiar_ansi::StyledLine::plain(format!("Open {url} in your browser instead"));
            line.linkify();
            self.output.append_line(&line);
        }
    }

    fn play_sound(&self, path: &str) {
        let path = familiar_config::paths::expand(path);
        if !path.is_file() {
            self.output.notice(&format!("Sound file not found: {}", path.display()));
            return;
        }
        let mut sounds = self.sounds.borrow_mut();
        sounds.retain(|clip| !clip.is_finished());
        let output = Rc::downgrade(&self.output);
        let notice: Rc<dyn Fn(String)> = Rc::new(move |text| {
            if let Some(output) = output.upgrade() {
                output.notice(&text);
            }
        });
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(clip) = crate::media_player::Clip::play(&name, &path, 1.0, 1, notice) {
            sounds.push(clip);
        }
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
