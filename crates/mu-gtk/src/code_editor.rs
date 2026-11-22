//! A small code editor for MU* programs: GtkSourceView with highlighting, line numbers,
//! language-aware auto-indent, search, and compile errors marked on their lines.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Once;

use adw::prelude::*;
use gtk::{gdk, glib};
use mu_code::Lang;
use mu_engine::EditRequest;
use sourceview5::prelude::*;

/// Language definitions we ship. GtkSourceView's own (Lua, Python, ...) are built in.
const LANGUAGE_SPECS: [(&str, &str); 2] = [
    ("moo.lang", include_str!("../data/language-specs/moo.lang")),
    ("muf.lang", include_str!("../data/language-specs/muf.lang")),
];

/// Languages offered in the picker, if GtkSourceView has them. "" is plain text.
const LANGUAGES: [(&str, &str); 7] = [
    ("", "Plain Text"),
    ("moo", "MOO (Stunt)"),
    ("muf", "MUF"),
    ("lua", "Lua"),
    ("js", "JavaScript"),
    ("python3", "Python"),
    ("c", "C"),
];

const INDENT: &str = "  ";

/// Gives a GtkSourceView buffer the light or dark Adwaita scheme, following the app. Holds only
/// a weak reference, so closed editors aren't kept alive.
pub fn follow_dark_mode(buffer: &sourceview5::Buffer) {
    let apply = |style: &adw::StyleManager, buffer: &sourceview5::Buffer| {
        let id = if style.is_dark() { "Adwaita-dark" } else { "Adwaita" };
        if let Some(scheme) = sourceview5::StyleSchemeManager::default().scheme(id) {
            buffer.set_style_scheme(Some(&scheme));
        }
    };
    let style = adw::StyleManager::default();
    apply(&style, buffer);
    let weak = buffer.downgrade();
    style.connect_dark_notify(move |style| {
        if let Some(buffer) = weak.upgrade() {
            apply(style, &buffer);
        }
    });
}

/// Where users can add their own `.lang` files. Searched before ours, so they can override.
pub fn user_language_dir() -> PathBuf {
    mu_config::data_dir().join("language-specs")
}

/// Writes our language definitions where GtkSourceView can find them and adds the folders to
/// its search path. Runs once.
fn install_languages() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        sourceview5::init();
        let dir = mu_engine::media::cache_dir().with_file_name("language-specs");
        if std::fs::create_dir_all(&dir).is_ok() {
            for (name, spec) in LANGUAGE_SPECS {
                let _ = std::fs::write(dir.join(name), spec);
            }
        }
        let manager = sourceview5::LanguageManager::default();
        let mut path: Vec<String> = vec![user_language_dir().to_string_lossy().into_owned(), dir.to_string_lossy().into_owned()];
        path.extend(manager.search_path().iter().map(|p| p.to_string()));
        let refs: Vec<&str> = path.iter().map(String::as_str).collect();
        manager.set_search_path(&refs);
    });
}

pub struct CodeEditor {
    pub window: adw::Window,
    view: sourceview5::View,
    buffer: sourceview5::Buffer,
    title: adw::WindowTitle,
    name: String,
    language: RefCell<Option<String>>,
    search_bar: gtk::SearchBar,
    search: sourceview5::SearchContext,
    errors: gtk::ListBox,
    errors_revealer: gtk::Revealer,
    on_send: Box<dyn Fn(String)>,
}

impl CodeEditor {
    /// Opens an editor for `request`. `on_send` gets the text each time the user sends it.
    pub fn open(parent: Option<&gtk::Window>, request: &EditRequest, on_send: impl Fn(String) + 'static) -> Rc<Self> {
        install_languages();
        let buffer = sourceview5::Buffer::new(None);
        buffer.set_highlight_matching_brackets(true);
        let view = sourceview5::View::builder()
            .buffer(&buffer)
            .monospace(true)
            .show_line_numbers(true)
            .highlight_current_line(true)
            .auto_indent(true)
            .indent_width(2)
            .tab_width(4)
            .insert_spaces_instead_of_tabs(true)
            .smart_backspace(true)
            .show_line_marks(true)
            .top_margin(6)
            .bottom_margin(6)
            .left_margin(6)
            .vexpand(true)
            .build();
        let attributes = sourceview5::MarkAttributes::new();
        attributes.set_icon_name("dialog-error-symbolic");
        attributes.set_background(&gdk::RGBA::new(0.9, 0.2, 0.2, 0.18));
        view.set_mark_attributes("error", &attributes, 10);
        attributes.connect_query_tooltip_text(|_, mark| mark.name().map(|n| n.to_string()).unwrap_or_default());

        let settings = sourceview5::SearchSettings::new();
        settings.set_wrap_around(true);
        settings.set_case_sensitive(false);
        let search = sourceview5::SearchContext::new(&buffer, Some(&settings));
        let search_entry = gtk::SearchEntry::builder().placeholder_text("Find").hexpand(true).build();
        let search_bar = gtk::SearchBar::builder().child(&search_entry).show_close_button(true).build();
        search_bar.connect_entry(&search_entry);

        let errors = gtk::ListBox::builder().css_classes(["boxed-list"]).selection_mode(gtk::SelectionMode::None).build();
        let errors_box = gtk::ScrolledWindow::builder()
            .child(&errors)
            .max_content_height(140)
            .propagate_natural_height(true)
            .margin_start(8)
            .margin_end(8)
            .margin_bottom(8)
            .build();
        let errors_revealer = gtk::Revealer::builder().child(&errors_box).reveal_child(false).build();

        let scrolled = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.append(&search_bar);
        body.append(&scrolled);
        body.append(&errors_revealer);

        let title = adw::WindowTitle::new(&request.name, "");
        let close = gtk::Button::with_label("Close");
        let send = gtk::Button::builder().label("Send").tooltip_text("Send to the server (Ctrl+S)").css_classes(["suggested-action"]).build();
        let language_names: Vec<&str> = LANGUAGES.iter().map(|(_, name)| *name).collect();
        let picker = gtk::DropDown::from_strings(&language_names);
        picker.set_tooltip_text(Some("Language"));
        let menu = gio_menu();
        let more = gtk::MenuButton::builder().icon_name("view-more-symbolic").menu_model(&menu).tooltip_text("More").build();
        let header = adw::HeaderBar::builder().title_widget(&title).show_end_title_buttons(false).show_start_title_buttons(false).build();
        header.pack_start(&close);
        header.pack_end(&send);
        header.pack_end(&more);
        header.pack_end(&picker);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&body));

        let window = adw::Window::builder().default_width(820).default_height(640).content(&toolbar).build();
        window.set_transient_for(parent);

        let this = Rc::new(Self {
            window,
            view,
            buffer,
            title,
            name: request.name.clone(),
            language: RefCell::new(None),
            search_bar,
            search,
            errors,
            errors_revealer,
            on_send: Box::new(on_send),
        });

        this.set_language(request.language.as_deref());
        let selected = LANGUAGES.iter().position(|(id, _)| Some(*id) == request.language.as_deref()).unwrap_or(0);
        picker.set_selected(selected as u32);
        this.load(&request.content);
        follow_dark_mode(&this.buffer);
        this.setup_actions(&send, &close, &picker);
        this.setup_search(&search_entry);
        this.setup_keys();

        let subtitle = if request.is_local() {
            "Sent line by line".to_string()
        } else {
            format!("{} · {}", request.reference, request.kind)
        };
        this.title.set_subtitle(&subtitle);
        this.update_title();
        this.window.present();
        this.view.grab_focus();
        this
    }

    fn set_language(&self, id: Option<&str>) {
        let id = id.filter(|id| !id.is_empty());
        let language = id.and_then(|id| sourceview5::LanguageManager::default().language(id));
        self.buffer.set_language(language.as_ref());
        // With our own rules, Enter is handled in setup_keys; otherwise keep the previous indent.
        let has_rules = id.and_then(Lang::from_id).is_some();
        self.view.set_auto_indent(!has_rules);
        *self.language.borrow_mut() = id.map(str::to_string);
    }

    fn lang(&self) -> Option<Lang> {
        self.language.borrow().as_deref().and_then(Lang::from_id)
    }

    /// Replaces the text without marking it as changed.
    pub fn load(&self, content: &str) {
        self.buffer.begin_irreversible_action();
        self.buffer.set_text(content);
        self.buffer.end_irreversible_action();
        self.buffer.place_cursor(&self.buffer.start_iter());
        self.buffer.set_modified(false);
    }

    pub fn is_modified(&self) -> bool {
        self.buffer.is_modified()
    }

    pub fn present(&self) {
        self.window.present();
    }

    fn update_title(&self) {
        let dirty = if self.buffer.is_modified() { "• " } else { "" };
        self.title.set_title(&format!("{dirty}{}", self.name));
        self.window.set_title(Some(&format!("{dirty}{}", self.name)));
    }

    fn send(&self) {
        self.clear_errors();
        let text = self.buffer.text(&self.buffer.start_iter(), &self.buffer.end_iter(), false).to_string();
        (self.on_send)(text);
        self.buffer.set_modified(false);
    }

    /// Marks a compile error. `line` counts from 1.
    pub fn add_error(self: &Rc<Self>, line: u32, message: &str) {
        let Some(iter) = self.buffer.iter_at_line(line.saturating_sub(1) as i32) else { return };
        self.buffer.create_source_mark(Some(&format!("Line {line}: {message}")), "error", &iter);
        let row = adw::ActionRow::builder()
            .title(format!("Line {line}"))
            .subtitle(message)
            .use_markup(false)
            .activatable(true)
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("dialog-error-symbolic"));
        let weak = Rc::downgrade(self);
        row.connect_activated(move |_| {
            if let Some(this) = weak.upgrade() {
                this.go_to_line(line);
            }
        });
        self.errors.append(&row);
        self.errors_revealer.set_reveal_child(true);
        if self.errors.observe_children().n_items() == 1 {
            self.go_to_line(line);
        }
    }

    pub fn clear_errors(&self) {
        self.buffer.remove_source_marks(&self.buffer.start_iter(), &self.buffer.end_iter(), Some("error"));
        self.errors.remove_all();
        self.errors_revealer.set_reveal_child(false);
    }

    fn go_to_line(&self, line: u32) {
        if let Some(mut iter) = self.buffer.iter_at_line(line.saturating_sub(1) as i32) {
            self.buffer.place_cursor(&iter);
            self.view.scroll_to_iter(&mut iter, 0.1, true, 0.0, 0.3);
            self.view.grab_focus();
        }
    }

    /// Enter with indentation rules: re-indent the finished line, then indent the new one.
    fn smart_enter(&self, lang: Lang) {
        let buffer = &self.buffer;
        buffer.begin_user_action();
        buffer.delete_selection(true, true);
        let cursor = buffer.iter_at_mark(&buffer.get_insert());
        let before = buffer.text(&buffer.start_iter(), &cursor, false);
        let (current, next) = mu_code::on_enter(&before, lang);

        // Replace the finished line's leading whitespace (but not past the cursor).
        let line = cursor.line();
        if let Some(mut start) = buffer.iter_at_line(line) {
            let mut end = start;
            while end < cursor && matches!(end.char(), ' ' | '\t') {
                end.forward_char();
            }
            buffer.delete(&mut start, &mut end);
            buffer.insert(&mut start, &INDENT.repeat(current));
        }
        // Text after the cursor moves to the new line, without its leading spaces.
        let mut cursor = buffer.iter_at_mark(&buffer.get_insert());
        let mut after = cursor;
        while matches!(after.char(), ' ' | '\t') {
            after.forward_char();
        }
        buffer.delete(&mut cursor, &mut after);
        buffer.insert(&mut cursor, &format!("\n{}", INDENT.repeat(next)));
        buffer.end_user_action();
        self.view.scroll_mark_onscreen(&buffer.get_insert());
    }

    /// Re-indents the whole program, keeping the cursor on its line.
    fn reindent(&self) {
        let Some(lang) = self.lang() else { return };
        let buffer = &self.buffer;
        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string();
        let new = mu_code::reindent(&text, lang, INDENT);
        if new == text {
            return;
        }
        let line = buffer.iter_at_mark(&buffer.get_insert()).line();
        buffer.begin_user_action();
        let (mut start, mut end) = buffer.bounds();
        buffer.delete(&mut start, &mut end);
        buffer.insert(&mut start, &new);
        buffer.end_user_action();
        if let Some(mut iter) = buffer.iter_at_line(line) {
            while matches!(iter.char(), ' ' | '\t') {
                iter.forward_char();
            }
            buffer.place_cursor(&iter);
        }
    }

    fn find(&self, backward: bool) {
        let cursor = self.buffer.iter_at_mark(&self.buffer.get_insert());
        let found = if backward {
            self.search.backward(&cursor)
        } else {
            // Start after the current selection, so repeated searches move on.
            let start = self.buffer.selection_bounds().map_or(cursor, |(_, end)| end);
            self.search.forward(&start)
        };
        if let Some((mut start, end, _)) = found {
            self.buffer.select_range(&start, &end);
            self.view.scroll_to_iter(&mut start, 0.1, false, 0.0, 0.0);
        }
    }

    fn setup_actions(self: &Rc<Self>, send: &gtk::Button, close: &gtk::Button, picker: &gtk::DropDown) {
        let weak = Rc::downgrade(self);
        send.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.send();
            }
        });
        let window = self.window.clone();
        close.connect_clicked(move |_| {
            window.close();
        });
        let weak = Rc::downgrade(self);
        picker.connect_selected_notify(move |picker| {
            if let Some(this) = weak.upgrade() {
                let id = LANGUAGES.get(picker.selected() as usize).map(|(id, _)| *id);
                this.set_language(id);
            }
        });
        let weak = Rc::downgrade(self);
        self.buffer.connect_modified_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.update_title();
            }
        });

        let actions = gtk::gio::SimpleActionGroup::new();
        let add = |name: &str, f: Box<dyn Fn(&Rc<Self>)>| {
            let action = gtk::gio::SimpleAction::new(name, None);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, _| {
                if let Some(this) = weak.upgrade() {
                    f(&this);
                }
            });
            actions.add_action(&action);
        };
        add("reindent", Box::new(|this| this.reindent()));
        add("find", Box::new(|this| this.search_bar.set_search_mode(true)));
        add("clear-errors", Box::new(|this| this.clear_errors()));
        self.window.insert_action_group("editor", Some(&actions));

        // Ask before throwing away unsent changes.
        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |window| {
            let Some(this) = weak.upgrade() else { return glib::Propagation::Proceed };
            if !this.is_modified() {
                return glib::Propagation::Proceed;
            }
            let dialog = adw::AlertDialog::new(Some("Discard Changes?"), Some("Your changes haven't been sent to the server."));
            dialog.add_response("cancel", "Keep Editing");
            dialog.add_response("discard", "Discard");
            dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
            dialog.set_close_response("cancel");
            let window = window.clone();
            let buffer = this.buffer.clone();
            dialog.connect_response(None, move |_, response| {
                if response == "discard" {
                    buffer.set_modified(false);
                    window.close();
                }
            });
            dialog.present(Some(&this.window));
            glib::Propagation::Stop
        });
    }

    fn setup_search(self: &Rc<Self>, entry: &gtk::SearchEntry) {
        let settings = self.search.settings();
        entry.connect_search_changed(move |entry| {
            let text = entry.text();
            settings.set_search_text((!text.is_empty()).then_some(text.as_str()));
        });
        let weak = Rc::downgrade(self);
        entry.connect_activate(move |_| {
            if let Some(this) = weak.upgrade() {
                this.find(false);
            }
        });
        let weak = Rc::downgrade(self);
        entry.connect_previous_match(move |_| {
            if let Some(this) = weak.upgrade() {
                this.find(true);
            }
        });
        let weak = Rc::downgrade(self);
        entry.connect_next_match(move |_| {
            if let Some(this) = weak.upgrade() {
                this.find(false);
            }
        });
        let weak = Rc::downgrade(self);
        entry.connect_stop_search(move |_| {
            if let Some(this) = weak.upgrade() {
                this.search_bar.set_search_mode(false);
                this.view.grab_focus();
            }
        });
    }

    fn setup_keys(self: &Rc<Self>) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(this) = weak.upgrade() else { return glib::Propagation::Proceed };
            let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
            match key {
                gdk::Key::s if ctrl => this.send(),
                gdk::Key::f if ctrl => this.search_bar.set_search_mode(true),
                gdk::Key::I | gdk::Key::i if ctrl && shift => this.reindent(),
                gdk::Key::g if ctrl => this.find(false),
                gdk::Key::G if ctrl => this.find(true),
                gdk::Key::Return | gdk::Key::KP_Enter if this.view.has_focus() && !ctrl => {
                    let Some(lang) = this.lang() else { return glib::Propagation::Proceed };
                    this.smart_enter(lang);
                }
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        self.window.add_controller(keys);
    }
}

fn gio_menu() -> gtk::gio::Menu {
    let menu = gtk::gio::Menu::new();
    menu.append(Some("Re-indent (Ctrl+Shift+I)"), Some("editor.reindent"));
    menu.append(Some("Find (Ctrl+F)"), Some("editor.find"));
    menu.append(Some("Clear Error Marks"), Some("editor.clear-errors"));
    menu
}
