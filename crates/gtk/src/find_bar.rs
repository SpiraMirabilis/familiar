//! Finding text in a tab's output: a search bar over it that highlights every match and
//! scrolls to the current one. A search starts at the newest match; Previous goes up the
//! scrollback, Next goes down, and both wrap around.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::output::OutputView;

/// Matches past this many aren't highlighted or counted, so a one-letter search on a long
/// scrollback stays quick.
const MAX_MATCHES: usize = 10_000;

const FLAGS: gtk::TextSearchFlags = gtk::TextSearchFlags::CASE_INSENSITIVE.union(gtk::TextSearchFlags::TEXT_ONLY);

pub struct FindBar {
    pub bar: gtk::SearchBar,
    entry: gtk::SearchEntry,
    count: gtk::Label,
    output: Rc<OutputView>,
    /// Every match, and the current one over it.
    match_tag: gtk::TextTag,
    current_tag: gtk::TextTag,
    /// The current match, as marks so that new output and trimming don't move it.
    start: gtk::TextMark,
    end: gtk::TextMark,
    has_current: Cell<bool>,
    on_close: RefCell<Option<Box<dyn Fn()>>>,
}

impl FindBar {
    pub fn new(output: Rc<OutputView>) -> Rc<Self> {
        let entry = gtk::SearchEntry::builder().placeholder_text("Find in the output").hexpand(true).build();
        let count = gtk::Label::builder().css_classes(["dim-label", "numeric"]).width_chars(10).xalign(1.0).build();
        let previous = gtk::Button::builder().icon_name("go-up-symbolic").tooltip_text("Previous Match").css_classes(["flat"]).build();
        let next = gtk::Button::builder().icon_name("go-down-symbolic").tooltip_text("Next Match").css_classes(["flat"]).build();
        let content = gtk::Box::builder().spacing(6).width_request(420).build();
        content.append(&entry);
        content.append(&count);
        content.append(&previous);
        content.append(&next);
        let bar = gtk::SearchBar::builder().child(&content).show_close_button(true).build();
        bar.connect_entry(&entry);

        let buffer = output.buffer();
        let match_tag = gtk::TextTag::builder().background_rgba(&gdk::RGBA::new(0.96, 0.83, 0.18, 0.35)).build();
        let current_tag = gtk::TextTag::builder()
            .background_rgba(&gdk::RGBA::new(1.0, 0.6, 0.0, 1.0))
            .foreground_rgba(&gdk::RGBA::BLACK)
            .build();
        buffer.tag_table().add(&match_tag);
        buffer.tag_table().add(&current_tag);
        let start = buffer.create_mark(None, &buffer.start_iter(), true);
        let end = buffer.create_mark(None, &buffer.start_iter(), false);

        let this = Rc::new(Self {
            bar,
            entry,
            count,
            output,
            match_tag,
            current_tag,
            start,
            end,
            has_current: Cell::new(false),
            on_close: RefCell::default(),
        });

        let weak = Rc::downgrade(&this);
        this.entry.connect_search_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.has_current.set(false);
                this.step(true);
            }
        });
        // Enter goes up the scrollback, as the search started at the bottom; Shift+Enter
        // comes back down.
        let weak = Rc::downgrade(&this);
        this.entry.connect_activate(move |_| {
            if let Some(this) = weak.upgrade() {
                this.step(true);
            }
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&this);
        keys.connect_key_pressed(move |_, key, _, state| {
            let Some(this) = weak.upgrade() else { return glib::Propagation::Proceed };
            if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter) && state.contains(gdk::ModifierType::SHIFT_MASK) {
                this.step(false);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        this.entry.add_controller(keys);
        for (backward, button) in [(true, &previous), (false, &next)] {
            let weak = Rc::downgrade(&this);
            button.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.step(backward);
                }
            });
        }
        let weak = Rc::downgrade(&this);
        this.entry.connect_previous_match(move |_| {
            if let Some(this) = weak.upgrade() {
                this.step(true);
            }
        });
        let weak = Rc::downgrade(&this);
        this.entry.connect_next_match(move |_| {
            if let Some(this) = weak.upgrade() {
                this.step(false);
            }
        });
        let weak = Rc::downgrade(&this);
        this.entry.connect_stop_search(move |_| {
            if let Some(this) = weak.upgrade() {
                this.bar.set_search_mode(false);
            }
        });
        let weak = Rc::downgrade(&this);
        this.bar.connect_search_mode_enabled_notify(move |bar| {
            let Some(this) = weak.upgrade() else { return };
            if !bar.is_search_mode() {
                this.unhighlight();
                if let Some(f) = this.on_close.borrow().as_ref() {
                    f();
                }
            }
        });
        this
    }

    /// `f` runs when the bar closes, to go back to the input.
    pub fn connect_close<F: Fn() + 'static>(&self, f: F) {
        *self.on_close.borrow_mut() = Some(Box::new(f));
    }

    /// Shows the bar with its text selected, ready for a new search.
    pub fn open(&self) {
        let reopened = !self.bar.is_search_mode();
        self.bar.set_search_mode(true);
        self.entry.grab_focus();
        self.entry.select_region(0, -1);
        // Closing took the highlights away.
        if reopened && !self.entry.text().is_empty() {
            self.highlight();
            self.show_current();
        }
    }

    /// Moves to the previous (`backward`) or next match. With the bar closed this opens it,
    /// carrying on with the last search if there was one.
    pub fn find(&self, backward: bool) {
        if !self.bar.is_search_mode() {
            self.open();
            if self.entry.text().is_empty() {
                return;
            }
        }
        self.step(backward);
    }

    fn step(&self, backward: bool) {
        let text = self.entry.text();
        if text.is_empty() {
            self.has_current.set(false);
            self.unhighlight();
            self.count.set_label("");
            return;
        }
        let buffer = self.output.buffer();
        let current = self.current();
        let found = match (current, backward) {
            (Some((start, _)), true) => start.backward_search(&text, FLAGS, None).or_else(|| buffer.end_iter().backward_search(&text, FLAGS, None)),
            (Some((_, end)), false) => end.forward_search(&text, FLAGS, None).or_else(|| buffer.start_iter().forward_search(&text, FLAGS, None)),
            // A new search starts at the newest match.
            (None, _) => buffer.end_iter().backward_search(&text, FLAGS, None),
        };
        match found {
            Some((start, end)) => {
                buffer.move_mark(&self.start, &start);
                buffer.move_mark(&self.end, &end);
                self.has_current.set(true);
            }
            None => self.has_current.set(false),
        }
        self.highlight();
        self.show_current();
    }

    /// The current match, unless the text it was on has gone.
    fn current(&self) -> Option<(gtk::TextIter, gtk::TextIter)> {
        let buffer = self.output.buffer();
        let (start, end) = (buffer.iter_at_mark(&self.start), buffer.iter_at_mark(&self.end));
        (self.has_current.get() && start < end).then_some((start, end))
    }

    fn unhighlight(&self) {
        let buffer = self.output.buffer();
        let (start, end) = buffer.bounds();
        buffer.remove_tag(&self.match_tag, &start, &end);
        buffer.remove_tag(&self.current_tag, &start, &end);
    }

    /// Marks every match and the current one, and says which of how many it is.
    fn highlight(&self) {
        self.unhighlight();
        let text = self.entry.text();
        let buffer = self.output.buffer();
        // Over the output's own colours, which may have been added since the last search.
        let table = buffer.tag_table();
        self.match_tag.set_priority(table.size() - 1);
        self.current_tag.set_priority(table.size() - 1);

        let current = self.current().map(|(start, _)| start.offset());
        let mut total = 0;
        let mut index = None;
        let mut at = buffer.start_iter();
        while let Some((start, end)) = at.forward_search(&text, FLAGS, None) {
            if total == MAX_MATCHES {
                break;
            }
            total += 1;
            buffer.apply_tag(&self.match_tag, &start, &end);
            if Some(start.offset()) == current {
                index = Some(total);
            }
            at = end;
        }
        let more = if total == MAX_MATCHES { "+" } else { "" };
        self.count.set_label(&match (total, index) {
            (0, _) => "No matches".to_string(),
            (_, Some(i)) => format!("{i} of {total}{more}"),
            (_, None) => format!("{total}{more}"),
        });
        if let Some((start, end)) = self.current() {
            buffer.apply_tag(&self.current_tag, &start, &end);
        }
        if total == 0 {
            self.entry.add_css_class("error");
        } else {
            self.entry.remove_css_class("error");
        }
    }

    fn show_current(&self) {
        if self.current().is_some() {
            self.output.show_mark(&self.start);
        }
    }
}
