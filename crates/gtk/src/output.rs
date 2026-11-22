//! The scrollback view: a read-only TextView that shows styled lines.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gtk::glib::translate::IntoGlib;
use gtk::prelude::*;
use gtk::{gdk, glib, pango};
use familiar_ansi::{Palette, Rgb, Style, StyledLine};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TagKey {
    fg: Option<Rgb>,
    bg: Option<Rgb>,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    link: Option<Arc<str>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prompt {
    None,
    /// A prompt ended by GA/EOR. It stays, and the next line starts below it.
    Terminated,
    /// An unterminated line shown after a timeout. The next output replaces it.
    Partial,
}

fn rgba(c: Rgb) -> gdk::RGBA {
    gdk::RGBA::new(c.0 as f32 / 255.0, c.1 as f32 / 255.0, c.2 as f32 / 255.0, 1.0)
}

pub struct OutputView {
    pub scrolled: gtk::ScrolledWindow,
    pub view: gtk::TextView,
    buffer: gtk::TextBuffer,
    palette: Palette,
    max_lines: usize,
    tags: RefCell<HashMap<TagKey, gtk::TextTag>>,
    links: RefCell<HashMap<gtk::TextTag, Arc<str>>>,
    notice_tag: gtk::TextTag,
    echo_tag: gtk::TextTag,
    /// Start of the current prompt, including the newline before it.
    prompt_mark: gtk::TextMark,
    prompt: Cell<Prompt>,
    /// Keep the view scrolled to the bottom as text arrives.
    follow: Rc<Cell<bool>>,
    on_link: RefCell<Option<Box<dyn Fn(&str)>>>,
    /// A widget (an avatar) to put at the start of the next line.
    next_prefix: RefCell<Option<gtk::Widget>>,
}

impl OutputView {
    pub fn new(palette: Palette, max_lines: usize) -> Rc<Self> {
        let buffer = gtk::TextBuffer::new(None);
        let view = gtk::TextView::builder()
            .buffer(&buffer)
            .editable(false)
            .cursor_visible(false)
            .wrap_mode(gtk::WrapMode::WordChar)
            .left_margin(8)
            .right_margin(8)
            .top_margin(6)
            .bottom_margin(6)
            .monospace(true)
            .css_classes(["mu-output"])
            .build();
        let scrolled = gtk::ScrolledWindow::builder()
            .child(&view)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();

        let notice_tag = gtk::TextTag::builder()
            .foreground_rgba(&rgba(palette.foreground.blend(palette.background, 0.45)))
            .style(pango::Style::Italic)
            .build();
        let echo_tag = gtk::TextTag::builder().foreground_rgba(&rgba(palette.ansi[3])).build();
        buffer.tag_table().add(&notice_tag);
        buffer.tag_table().add(&echo_tag);
        let prompt_mark = buffer.create_mark(None, &buffer.end_iter(), true);

        let this = Rc::new(Self {
            scrolled,
            view,
            buffer,
            palette,
            max_lines: max_lines.max(100),
            tags: RefCell::default(),
            links: RefCell::default(),
            notice_tag,
            echo_tag,
            prompt_mark,
            prompt: Cell::new(Prompt::None),
            follow: Rc::new(Cell::new(true)),
            on_link: RefCell::default(),
            next_prefix: RefCell::default(),
        });
        this.setup_scrolling();
        this.setup_links();
        this
    }

    pub fn connect_link<F: Fn(&str) + 'static>(&self, f: F) {
        *self.on_link.borrow_mut() = Some(Box::new(f));
    }

    /// Keeps the view pinned to the bottom while `follow` is set.
    ///
    /// `follow` only changes in response to user input (wheel, scrollbar, selection drags, Page
    /// Up/Down). The adjustment's value can't be used to decide it: GtkTextView measures wrapped
    /// lines lazily and moves the adjustment itself as it does, which would look like the user
    /// scrolling up.
    fn setup_scrolling(&self) {
        let adj = self.scrolled.vadjustment();
        let follow = self.follow.clone();
        let pointer_down = Rc::new(Cell::new(false));

        let at_bottom = |adj: &gtk::Adjustment| adj.value() >= adj.upper() - adj.page_size() - 4.0;
        let to_bottom = |adj: &gtk::Adjustment| adj.set_value(adj.upper() - adj.page_size());

        {
            let follow = follow.clone();
            let pointer_down = pointer_down.clone();
            adj.connect_value_changed(move |adj| {
                if pointer_down.get() {
                    // Dragging the scrollbar, or a selection that auto-scrolls.
                    follow.set(at_bottom(adj));
                } else if follow.get() && !at_bottom(adj) {
                    to_bottom(adj);
                }
            });
        }
        for notify in ["upper", "page-size"] {
            let follow = follow.clone();
            adj.connect_notify_local(Some(notify), move |adj, _| {
                if follow.get() {
                    to_bottom(adj);
                }
            });
        }

        // Wheel and touchpad. This runs before the scrolled window moves, so scrolling up stops
        // following before the value changes.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let follow = follow.clone();
            let adj = adj.clone();
            scroll.connect_scroll(move |_, _, dy| {
                if dy < 0.0 {
                    follow.set(false);
                } else {
                    let follow = follow.clone();
                    let adj = adj.clone();
                    glib::idle_add_local_once(move || follow.set(at_bottom(&adj)));
                }
                glib::Propagation::Proceed
            });
        }
        self.scrolled.add_controller(scroll);

        // Mouse button held on the scrollbar or the text (selecting). While it's down, the user
        // is in control; on release, follow again if they left the view at the bottom.
        let legacy = gtk::EventControllerLegacy::new();
        legacy.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let adj = adj.clone();
            legacy.connect_event(move |_, event| {
                match event.event_type() {
                    gdk::EventType::ButtonPress => pointer_down.set(true),
                    gdk::EventType::ButtonRelease => {
                        pointer_down.set(false);
                        follow.set(at_bottom(&adj));
                    }
                    _ => {}
                }
                glib::Propagation::Proceed
            });
        }
        self.scrolled.add_controller(legacy);
    }

    fn link_at(&self, x: f64, y: f64) -> Option<Arc<str>> {
        let (bx, by) = self.view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        let iter = self.view.iter_at_location(bx, by)?;
        let links = self.links.borrow();
        iter.tags().iter().find_map(|tag| links.get(tag).cloned())
    }

    fn setup_links(self: &Rc<Self>) {
        let click = gtk::GestureClick::builder().button(gdk::BUTTON_PRIMARY).build();
        let weak = Rc::downgrade(self);
        click.connect_released(move |gesture, _, x, y| {
            let Some(this) = weak.upgrade() else { return };
            // Don't follow the link when the user finished selecting text.
            if this.buffer.has_selection() {
                return;
            }
            if let Some(link) = this.link_at(x, y) {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                if let Some(f) = this.on_link.borrow().as_ref() {
                    f(&link);
                }
            }
        });
        self.view.add_controller(click);

        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        motion.connect_motion(move |_, x, y| {
            let Some(this) = weak.upgrade() else { return };
            let link = this.link_at(x, y);
            this.view.set_cursor_from_name(Some(if link.is_some() { "pointer" } else { "text" }));
            this.view.set_tooltip_text(link.as_deref());
        });
        self.view.add_controller(motion);
    }

    fn tag_for(&self, style: &Style) -> Option<gtk::TextTag> {
        let (fg, bg) = self.palette.resolve(style);
        let key = TagKey {
            fg: (fg != self.palette.foreground).then_some(fg),
            bg: (bg != self.palette.background).then_some(bg),
            bold: style.bold,
            italic: style.italic,
            underline: style.underline || style.link.is_some(),
            strike: style.strike,
            link: style.link.clone(),
        };
        if key == (TagKey { fg: None, bg: None, bold: false, italic: false, underline: false, strike: false, link: None })
        {
            return None;
        }
        let mut tags = self.tags.borrow_mut();
        if let Some(tag) = tags.get(&key) {
            return Some(tag.clone());
        }
        let tag = gtk::TextTag::new(None);
        if let Some(fg) = key.fg {
            tag.set_foreground_rgba(Some(&rgba(fg)));
        }
        if let Some(bg) = key.bg {
            tag.set_background_rgba(Some(&rgba(bg)));
        }
        if key.bold {
            tag.set_weight(pango::Weight::Bold.into_glib());
        }
        if key.italic {
            tag.set_style(pango::Style::Italic);
        }
        if key.underline {
            tag.set_underline(pango::Underline::Single);
        }
        if key.strike {
            tag.set_strikethrough(true);
        }
        self.buffer.tag_table().add(&tag);
        if let Some(link) = &key.link {
            self.links.borrow_mut().insert(tag.clone(), link.clone());
        }
        tags.insert(key, tag.clone());
        Some(tag)
    }

    /// Removes a partial-line prompt, if one is shown.
    fn clear_partial(&self) {
        if self.prompt.get() == Prompt::Partial {
            let mut start = self.buffer.iter_at_mark(&self.prompt_mark);
            let mut end = self.buffer.end_iter();
            self.buffer.delete(&mut start, &mut end);
        }
        self.prompt.set(Prompt::None);
    }

    /// Inserts a newline if the buffer isn't empty, so every line after the first starts on its
    /// own row without leaving a blank row at the bottom.
    fn start_line(&self) -> gtk::TextIter {
        let mut iter = self.buffer.end_iter();
        if self.buffer.char_count() > 0 {
            self.buffer.insert(&mut iter, "\n");
        }
        iter
    }

    fn insert_styled(&self, iter: &mut gtk::TextIter, line: &StyledLine) {
        for span in &line.spans {
            let text = line.span_text(span);
            match self.tag_for(&span.style) {
                Some(tag) => self.buffer.insert_with_tags(iter, text, &[&tag]),
                None => self.buffer.insert(iter, text),
            }
        }
    }

    pub fn append_line(&self, line: &StyledLine) {
        self.clear_partial();
        let mut iter = self.start_line();
        if let Some(widget) = self.next_prefix.borrow_mut().take() {
            self.insert_widget(&mut iter, &widget);
            self.buffer.insert(&mut iter, " ");
        }
        self.insert_styled(&mut iter, line);
        self.trim();
    }

    fn insert_widget(&self, iter: &mut gtk::TextIter, widget: &impl IsA<gtk::Widget>) {
        let anchor = self.buffer.create_child_anchor(iter);
        self.view.add_child_at_anchor(widget, &anchor);
    }

    /// Puts `widget` (an avatar) at the start of the next line.
    pub fn set_next_prefix(&self, widget: &impl IsA<gtk::Widget>) {
        *self.next_prefix.borrow_mut() = Some(widget.clone().upcast());
    }

    /// Adds a line holding just `widget` (an inline image).
    pub fn append_widget(&self, widget: &impl IsA<gtk::Widget>) {
        self.clear_partial();
        let mut iter = self.start_line();
        self.insert_widget(&mut iter, widget);
        self.trim();
    }

    pub fn set_prompt(&self, line: &StyledLine, terminated: bool) {
        self.clear_partial();
        self.buffer.move_mark(&self.prompt_mark, &self.buffer.end_iter());
        let mut iter = self.start_line();
        self.insert_styled(&mut iter, line);
        self.prompt.set(if terminated { Prompt::Terminated } else { Prompt::Partial });
    }

    fn append_tagged(&self, text: &str, tag: &gtk::TextTag) {
        self.clear_partial();
        let mut iter = self.start_line();
        self.buffer.insert_with_tags(&mut iter, text, &[tag]);
        self.trim();
    }

    /// A client message, shown dimmed.
    pub fn notice(&self, text: &str) {
        self.append_tagged(text, &self.notice_tag);
    }

    /// A command the user sent. It goes on the prompt's line, if there is a prompt.
    pub fn echo(&self, text: &str) {
        if self.prompt.get() == Prompt::None {
            self.append_tagged(text, &self.echo_tag);
        } else {
            self.prompt.set(Prompt::None);
            let mut iter = self.buffer.end_iter();
            self.buffer.insert_with_tags(&mut iter, text, &[&self.echo_tag]);
        }
    }

    pub fn clear(&self) {
        self.buffer.set_text("");
        self.prompt.set(Prompt::None);
    }

    fn trim(&self) {
        // Trim in chunks so we aren't deleting a line for every line added.
        let lines = self.buffer.line_count() as usize;
        if lines > self.max_lines + 200 {
            let mut start = self.buffer.start_iter();
            if let Some(mut end) = self.buffer.iter_at_line((lines - self.max_lines) as i32) {
                self.buffer.delete(&mut start, &mut end);
            }
        }
    }

    pub fn scroll_page(&self, up: bool) {
        let adj = self.scrolled.vadjustment();
        let step = adj.page_size() * 0.9;
        let bottom = adj.upper() - adj.page_size();
        let value = (if up { adj.value() - step } else { adj.value() + step }).clamp(adj.lower(), bottom);
        // Set follow first, so the value-changed handler doesn't snap back to the bottom.
        self.follow.set(value >= bottom - 4.0);
        adj.set_value(value);
    }

    pub fn scroll_to_end(&self) {
        let adj = self.scrolled.vadjustment();
        self.follow.set(true);
        adj.set_value(adj.upper() - adj.page_size());
    }

    /// The visible size in character cells, for NAWS.
    pub fn grid_size(&self) -> Option<(u16, u16)> {
        let width = self.view.width() - self.view.left_margin() - self.view.right_margin();
        let height = self.scrolled.height() - self.view.top_margin() - self.view.bottom_margin();
        if width <= 0 || height <= 0 {
            return None;
        }
        let metrics = self.view.pango_context().metrics(None, None);
        let char_width = metrics.approximate_char_width() as f64 / pango::SCALE as f64;
        let line_height = metrics.height() as f64 / pango::SCALE as f64;
        if char_width <= 0.0 || line_height <= 0.0 {
            return None;
        }
        let cols = (width as f64 / char_width).floor().clamp(1.0, u16::MAX as f64) as u16;
        let rows = (height as f64 / line_height).floor().clamp(1.0, u16::MAX as f64) as u16;
        Some((cols, rows))
    }
}
