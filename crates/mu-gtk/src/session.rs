//! One connection tab: output, input, and the network session behind them.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use mu_ansi::{AnsiParser, Palette};
use mu_config::{Character, Settings, World};
use mu_engine::Logger;
use mu_net::{ConnectOptions, Encoding, SessionEvent, SessionHandle, SessionOptions, spawn_session};

use crate::input::InputView;
use crate::output::OutputView;

/// The longest a single batch of events may run before yielding to the UI.
const BATCH_LIMIT: usize = 500;

pub struct SessionView {
    pub root: gtk::Box,
    pub output: Rc<OutputView>,
    pub input: Rc<InputView>,
    world: World,
    character: Option<Character>,
    settings: Settings,
    page: RefCell<Option<adw::TabPage>>,
    handle: RefCell<Option<SessionHandle>>,
    /// Incremented on each connect, so events from an old session are ignored.
    generation: Cell<u64>,
    ansi: RefCell<AnsiParser>,
    logger: RefCell<Option<Logger>>,
    server_echo: Cell<bool>,
    window_size: Cell<Option<(u16, u16)>>,
    on_open_uri: RefCell<Option<Box<dyn Fn(&str)>>>,
}

impl SessionView {
    pub fn new(world: World, character: Option<Character>, settings: Settings) -> Rc<Self> {
        let output = OutputView::new(Palette::default(), settings.scrollback_lines);
        let input = InputView::new(settings.input_history_size);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&output.scrolled);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        root.append(&input.root);

        let this = Rc::new(Self {
            root,
            output,
            input,
            world,
            character,
            settings,
            page: RefCell::default(),
            handle: RefCell::default(),
            generation: Cell::new(0),
            ansi: RefCell::new(AnsiParser::new()),
            logger: RefCell::default(),
            server_echo: Cell::new(false),
            window_size: Cell::new(None),
            on_open_uri: RefCell::default(),
        });
        this.setup();
        this
    }

    pub fn title(&self) -> String {
        match &self.character {
            Some(c) if !c.name.is_empty() => format!("{} @ {}", c.name, self.world.name),
            _ if self.world.name.is_empty() => format!("{}:{}", self.world.host, self.world.port),
            _ => self.world.name.clone(),
        }
    }

    pub fn set_page(&self, page: &adw::TabPage) {
        page.set_title(&self.title());
        page.set_tooltip(&format!("{}:{}{}", self.world.host, self.world.port, if self.world.tls { " (TLS)" } else { "" }));
        *self.page.borrow_mut() = Some(page.clone());
    }

    /// Called for http(s) links. send: and prompt: links are handled here.
    pub fn connect_open_uri<F: Fn(&str) + 'static>(&self, f: F) {
        *self.on_open_uri.borrow_mut() = Some(Box::new(f));
    }

    pub fn is_connected(&self) -> bool {
        self.handle.borrow().as_ref().is_some_and(|h| !h.is_closed())
    }

    fn setup(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.input.connect_submit(move |text, secret| {
            if let Some(this) = weak.upgrade() {
                this.submit(&text, secret);
            }
        });

        let output = Rc::downgrade(&self.output);
        self.input.connect_page(move |up| {
            if let Some(output) = output.upgrade() {
                output.scroll_page(up);
            }
        });

        let weak = Rc::downgrade(self);
        self.output.connect_link(move |uri| {
            if let Some(this) = weak.upgrade() {
                this.follow_link(uri);
            }
        });

        // Typing while the output has focus goes to the input.
        let keys = gtk::EventControllerKey::new();
        let input = Rc::downgrade(&self.input);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(input) = input.upgrade() else { return glib::Propagation::Proceed };
            if state.intersects(gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::ALT_MASK) {
                return glib::Propagation::Proceed;
            }
            if let Some(c) = key.to_unicode().filter(|c| !c.is_control()) {
                input.insert(&c.to_string());
                glib::Propagation::Stop
            } else if matches!(key, gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter) {
                input.grab_focus();
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
            if let Some(size) = this.output.grid_size()
                && this.window_size.get() != Some(size)
            {
                this.window_size.set(Some(size));
                if let Some(handle) = this.handle.borrow().as_ref() {
                    handle.set_window_size(size.0, size.1);
                }
            }
            glib::ControlFlow::Continue
        });
    }

    pub fn connect(self: &Rc<Self>) {
        if self.is_connected() {
            return;
        }
        let encoding = Encoding::from_name(&self.world.encoding).unwrap_or_default();
        let mut connect = ConnectOptions::new(&self.world.host, self.world.port);
        connect.tls = self.world.tls;
        connect.verify_certificate = self.world.verify_certificate;
        let mut opts = SessionOptions::new(connect);
        opts.encoding = encoding;
        opts.prompt_timeout = Some(Duration::from_millis(self.settings.prompt_timeout_ms));
        opts.telnet.client_name = crate::APP_NAME.into();

        self.output.notice(&format!(
            "Connecting to {}:{}{}...",
            self.world.host,
            self.world.port,
            if self.world.tls { " with TLS" } else { "" }
        ));
        self.set_status(true, None);
        *self.ansi.borrow_mut() = AnsiParser::new();

        let (handle, mut events) = {
            let _guard = crate::runtime().enter();
            spawn_session(opts)
        };
        if let Some(size) = self.window_size.get() {
            handle.set_window_size(size.0, size.1);
        }
        *self.handle.borrow_mut() = Some(handle);
        let generation = self.generation.get() + 1;
        self.generation.set(generation);

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            while let Some(event) = events.recv().await {
                let Some(this) = weak.upgrade() else { return };
                if this.generation.get() != generation {
                    return;
                }
                let mut done = this.handle_event(event);
                // Handle whatever else has already arrived before letting GTK redraw.
                for _ in 0..BATCH_LIMIT {
                    if done {
                        break;
                    }
                    match events.try_recv() {
                        Ok(event) => done = this.handle_event(event),
                        Err(_) => break,
                    }
                }
                if done {
                    return;
                }
            }
        });
    }

    pub fn disconnect(&self) {
        if let Some(handle) = self.handle.borrow().as_ref() {
            handle.disconnect();
        }
    }

    /// Shows connection state on the tab: a spinner while connecting, an icon once disconnected.
    fn set_status(&self, connecting: bool, disconnected: Option<&str>) {
        if let Some(page) = self.page.borrow().as_ref() {
            page.set_loading(connecting);
            match disconnected {
                Some(reason) => {
                    page.set_indicator_icon(Some(&gtk::gio::ThemedIcon::new("network-offline-symbolic")));
                    page.set_indicator_tooltip(reason);
                }
                None => page.set_indicator_icon(None::<&gtk::gio::Icon>),
            }
        }
    }

    /// Handles one event. Returns true once the session has ended.
    fn handle_event(&self, event: SessionEvent) -> bool {
        match event {
            SessionEvent::Connected { addr } => {
                self.set_status(false, None);
                self.output.notice(&format!("Connected to {addr}"));
                if let Some(login) = self.character.as_ref().map(|c| c.login.as_str()).filter(|l| !l.is_empty())
                    && let Some(handle) = self.handle.borrow().as_ref()
                {
                    for line in login.lines() {
                        handle.send_line(line);
                    }
                }
            }
            SessionEvent::Line(text) => {
                let mut line = self.ansi.borrow_mut().parse_line(&text);
                line.linkify();
                self.output.append_line(&line);
                self.log(&line.text);
                if line.bell {
                    self.output.view.error_bell();
                }
                self.mark_activity();
            }
            SessionEvent::Prompt(text) => {
                let mut line = self.ansi.borrow_mut().parse_line(&text);
                line.linkify();
                self.output.set_prompt(&line, true);
                self.log(&line.text);
                self.mark_activity();
            }
            SessionEvent::PartialLine(text) => {
                // Parse with a copy of the state: the real line will be parsed again later.
                let line = self.ansi.borrow().clone().parse_line(&text);
                self.output.set_prompt(&line, false);
                self.mark_activity();
            }
            SessionEvent::ServerEcho(on) => {
                self.server_echo.set(on);
                self.input.set_password_mode(on);
            }
            SessionEvent::EncodingChanged(e) => self.output.notice(&format!("Server switched encoding to {}", e.name())),
            SessionEvent::Notice(text) => self.output.notice(&text),
            SessionEvent::Gmcp { .. } | SessionEvent::Mssp(_) | SessionEvent::Compression(_) => {}
            SessionEvent::Disconnected { error } => {
                let reason = match &error {
                    Some(e) => format!("Disconnected: {e}"),
                    None => "Disconnected".to_string(),
                };
                self.output.notice(&format!("{reason}. Press Ctrl+R to reconnect."));
                self.log(&format!("--- {reason} ---"));
                self.set_status(false, Some(&reason));
                self.server_echo.set(false);
                self.input.set_password_mode(false);
                *self.handle.borrow_mut() = None;
                return true;
            }
        }
        false
    }

    fn mark_activity(&self) {
        if let Some(page) = self.page.borrow().as_ref()
            && !page.is_selected()
        {
            page.set_needs_attention(true);
        }
    }

    fn submit(&self, text: &str, secret: bool) {
        let Some(handle) = self.handle.borrow().clone().filter(|h| !h.is_closed()) else {
            self.output.notice("Not connected. Press Ctrl+R to reconnect.");
            return;
        };
        self.output.scroll_to_end();
        // Enter on an empty input still sends an empty line, which many MUDs use.
        let lines: Vec<&str> = if text.is_empty() { vec![""] } else { text.lines().collect() };
        for line in lines {
            handle.send_line(line);
            if !secret && self.settings.local_echo {
                self.output.echo(line);
            }
        }
    }

    fn follow_link(&self, uri: &str) {
        if let Some(command) = uri.strip_prefix("send:") {
            let command = percent_decode(command);
            if let Some(handle) = self.handle.borrow().as_ref() {
                handle.send_line(&command);
                if self.settings.local_echo {
                    self.output.echo(&command);
                }
            }
        } else if let Some(text) = uri.strip_prefix("prompt:") {
            self.input.set_text(&percent_decode(text));
            self.input.grab_focus();
        } else if let Some(f) = self.on_open_uri.borrow().as_ref() {
            f(uri);
        }
    }

    fn log(&self, text: &str) {
        let mut logger = self.logger.borrow_mut();
        if let Some(log) = logger.as_mut()
            && let Err(e) = log.line(text)
        {
            *logger = None;
            drop(logger);
            self.output.notice(&format!("Logging stopped: {e}"));
        }
    }

    pub fn is_logging(&self) -> bool {
        self.logger.borrow().is_some()
    }

    /// Starts logging, or stops it if it's already running. Returns the log path when started.
    pub fn toggle_log(&self) -> Result<Option<PathBuf>, String> {
        if let Some(log) = self.logger.borrow_mut().take() {
            log.close().map_err(|e| e.to_string())?;
            self.output.notice("Logging stopped");
            return Ok(None);
        }
        let log = Logger::open(&mu_config::log_dir(), &self.title(), self.settings.log_timestamps)
            .map_err(|e| e.to_string())?;
        let path = log.path().to_path_buf();
        *self.logger.borrow_mut() = Some(log);
        self.output.notice(&format!("Logging to {}", path.display()));
        Ok(Some(path))
    }
}

/// Decodes %XX escapes. Invalid escapes are kept as they are.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = s.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::percent_decode;

    #[test]
    fn decodes_percent_escapes() {
        assert_eq!(percent_decode("look%20at%20me"), "look at me");
        assert_eq!(percent_decode("caf%C3%A9"), "café");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }
}
