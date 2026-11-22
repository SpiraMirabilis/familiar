//! The command input: a multi-line TextView with history, plus a password entry that replaces
//! it while the server has echo turned on.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use mu_engine::History;

pub struct InputView {
    pub root: gtk::Stack,
    /// A GtkSourceView (with no source features on) so libspelling can check it.
    pub view: sourceview5::View,
    /// Keeps spellchecking running for as long as the input exists.
    _spelling: Option<libspelling::TextBufferAdapter>,
    password: gtk::PasswordEntry,
    history: RefCell<History>,
    on_submit: RefCell<Option<Box<dyn Fn(String, bool)>>>,
    on_page: RefCell<Option<Box<dyn Fn(bool)>>>,
    on_key: RefCell<Option<Box<dyn Fn(gdk::Key, gdk::ModifierType) -> bool>>>,
}

impl InputView {
    pub fn new(history_size: usize, spellcheck: bool) -> Rc<Self> {
        let buffer = sourceview5::Buffer::new(None);
        crate::code_editor::follow_dark_mode(&buffer);
        // Underline misspellings; right-click a word for suggestions.
        let spelling = spellcheck.then(|| {
            let adapter = libspelling::TextBufferAdapter::new(&buffer, &libspelling::Checker::default());
            adapter.set_enabled(true);
            adapter
        });
        let view = sourceview5::View::builder()
            .buffer(&buffer)
            .wrap_mode(gtk::WrapMode::WordChar)
            .accepts_tab(false)
            .left_margin(8)
            .right_margin(8)
            .top_margin(6)
            .bottom_margin(6)
            .monospace(true)
            .css_classes(["mu-input"])
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .child(&view)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(160)
            .build();
        let password = gtk::PasswordEntry::builder()
            .show_peek_icon(true)
            .placeholder_text("Password (the server turned off echo)")
            .margin_start(6)
            .margin_end(6)
            .margin_top(6)
            .margin_bottom(6)
            .build();

        if let Some(adapter) = &spelling {
            view.set_extra_menu(Some(&adapter.menu_model()));
            view.insert_action_group("spelling", Some(adapter));
        }
        let root = gtk::Stack::new();
        root.add_named(&scrolled, Some("text"));
        root.add_named(&password, Some("password"));

        let this = Rc::new(Self {
            root,
            view,
            _spelling: spelling,
            password,
            history: RefCell::new(History::new(history_size)),
            on_submit: RefCell::default(),
            on_page: RefCell::default(),
            on_key: RefCell::default(),
        });
        this.setup_keys();

        let weak = Rc::downgrade(&this);
        this.password.connect_activate(move |entry| {
            let Some(this) = weak.upgrade() else { return };
            let text = entry.text().to_string();
            entry.set_text("");
            if let Some(f) = this.on_submit.borrow().as_ref() {
                f(text, true);
            }
        });
        this
    }

    /// `f(text, secret)` is called when the user presses Enter. `secret` is true for password
    /// entry, which must not be echoed or logged.
    pub fn connect_submit<F: Fn(String, bool) + 'static>(&self, f: F) {
        *self.on_submit.borrow_mut() = Some(Box::new(f));
    }

    /// `f(key, modifiers)` sees every key press first, for keyboard macros. Returning true
    /// consumes the key.
    pub fn connect_key<F: Fn(gdk::Key, gdk::ModifierType) -> bool + 'static>(&self, f: F) {
        *self.on_key.borrow_mut() = Some(Box::new(f));
    }

    /// `f(up)` is called for Page Up / Page Down, to scroll the output.
    pub fn connect_page<F: Fn(bool) + 'static>(&self, f: F) {
        *self.on_page.borrow_mut() = Some(Box::new(f));
    }

    pub fn set_password_mode(&self, on: bool) {
        let focused = self.has_focus();
        self.root.set_visible_child_name(if on { "password" } else { "text" });
        if focused {
            self.grab_focus();
        }
    }

    pub fn has_focus(&self) -> bool {
        self.view.has_focus() || self.password.has_focus() || self.password.focus_child().is_some()
    }

    pub fn grab_focus(&self) {
        if self.root.visible_child_name().as_deref() == Some("password") {
            self.password.grab_focus();
        } else {
            self.view.grab_focus();
        }
    }

    /// Types text at the cursor. Used when the user starts typing while the output has focus.
    pub fn insert(&self, text: &str) {
        self.grab_focus();
        if self.root.visible_child_name().as_deref() == Some("password") {
            let mut pos = self.password.position();
            self.password.insert_text(text, &mut pos);
            self.password.set_position(pos);
        } else {
            self.view.buffer().insert_at_cursor(text);
        }
    }

    pub fn set_text(&self, text: &str) {
        let buffer = self.view.buffer();
        buffer.set_text(text);
        buffer.place_cursor(&buffer.end_iter());
    }

    fn text(&self) -> String {
        let buffer = self.view.buffer();
        buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string()
    }

    fn cursor_line(&self) -> (i32, i32) {
        let buffer = self.view.buffer();
        let cursor = buffer.iter_at_mark(&buffer.get_insert());
        (cursor.line(), buffer.line_count())
    }

    fn submit(&self) {
        let text = self.text();
        self.view.buffer().set_text("");
        self.history.borrow_mut().push(&text);
        if let Some(f) = self.on_submit.borrow().as_ref() {
            f(text, false);
        }
    }

    fn setup_keys(self: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(this) = weak.upgrade() else { return glib::Propagation::Proceed };
            if this.on_key.borrow().as_ref().is_some_and(|f| f(key, state)) {
                return glib::Propagation::Stop;
            }
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            match key {
                gdk::Key::Return | gdk::Key::KP_Enter if !shift => {
                    this.submit();
                    glib::Propagation::Stop
                }
                gdk::Key::Up if ctrl || this.cursor_line().0 == 0 => {
                    let current = this.text();
                    if let Some(text) = this.history.borrow_mut().older(&current) {
                        this.set_text(&text);
                    }
                    glib::Propagation::Stop
                }
                gdk::Key::Down if ctrl || {
                    let (line, count) = this.cursor_line();
                    line == count - 1
                } =>
                {
                    if let Some(text) = this.history.borrow_mut().newer() {
                        this.set_text(&text);
                    }
                    glib::Propagation::Stop
                }
                gdk::Key::Page_Up | gdk::Key::Page_Down => {
                    if let Some(f) = this.on_page.borrow().as_ref() {
                        f(key == gdk::Key::Page_Up);
                    }
                    glib::Propagation::Stop
                }
                gdk::Key::Escape => {
                    this.history.borrow_mut().reset();
                    this.view.buffer().set_text("");
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        self.view.add_controller(keys);
    }
}
