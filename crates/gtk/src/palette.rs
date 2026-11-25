//! The pickers: the command palette and the input history search. A search box over a list,
//! filtered as you type; Up and Down move, Enter picks, Escape closes.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};

use crate::fuzzy;

/// The most rows shown at once. The history can hold far more than anyone scrolls through.
const MAX_ROWS: usize = 300;

/// One entry in a picker.
pub struct Item {
    /// Shown as a heading over the entries while nothing is typed.
    pub group: &'static str,
    pub label: String,
    /// Dimmed, on the right: a shortcut, an address, a description.
    pub detail: String,
    pub run: Rc<dyn Fn()>,
}

struct Picker {
    dialog: adw::Dialog,
    entry: gtk::SearchEntry,
    list: gtk::ListBox,
    scrolled: gtk::ScrolledWindow,
    items: Vec<Item>,
    /// Indexes into `items` of the rows shown, in order.
    shown: RefCell<Vec<usize>>,
}

/// Opens a picker over `parent`'s window.
pub fn show(parent: &impl IsA<gtk::Widget>, title: &str, placeholder: &str, items: Vec<Item>) {
    let entry = gtk::SearchEntry::builder().placeholder_text(placeholder).hexpand(true).build();
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).css_classes(["navigation-sidebar"]).build();
    let scrolled = gtk::ScrolledWindow::builder().child(&list).hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).build();
    let search = gtk::Box::builder().margin_start(12).margin_end(12).margin_bottom(6).build();
    search.append(&entry);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.add_top_bar(&search);
    toolbar.set_content(Some(&scrolled));
    let dialog = adw::Dialog::builder().title(title).content_width(560).content_height(520).child(&toolbar).build();

    let this = Rc::new(Picker { dialog, entry, list, scrolled, items, shown: RefCell::default() });
    this.setup();
    this.refilter();

    let keep = RefCell::new(Some(this.clone()));
    this.dialog.connect_closed(move |_| {
        keep.borrow_mut().take();
    });
    this.dialog.set_focus(Some(&this.entry));
    this.dialog.present(Some(parent));
}

impl Picker {
    fn setup(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.entry.connect_search_changed(move |_| {
            if let Some(this) = weak.upgrade() {
                this.refilter();
            }
        });
        let weak = Rc::downgrade(self);
        self.entry.connect_activate(move |_| {
            if let Some(this) = weak.upgrade() {
                this.pick(this.list.selected_row());
            }
        });
        let weak = Rc::downgrade(self);
        self.entry.connect_stop_search(move |_| {
            if let Some(this) = weak.upgrade() {
                this.dialog.close();
            }
        });
        let weak = Rc::downgrade(self);
        self.list.connect_row_activated(move |_, row| {
            if let Some(this) = weak.upgrade() {
                this.pick(Some(row.clone()));
            }
        });

        // Up and Down move through the list while typing goes on in the search box.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(this) = weak.upgrade() else { return glib::Propagation::Proceed };
            let step = match key {
                gdk::Key::Up | gdk::Key::KP_Up => -1,
                gdk::Key::Down | gdk::Key::KP_Down => 1,
                gdk::Key::Page_Up | gdk::Key::KP_Page_Up => -10,
                gdk::Key::Page_Down | gdk::Key::KP_Page_Down => 10,
                _ => return glib::Propagation::Proceed,
            };
            this.move_selection(step);
            glib::Propagation::Stop
        });
        self.entry.add_controller(keys);

        // Headings over the groups, while the list is in its own order.
        let weak = Rc::downgrade(self);
        self.list.set_header_func(move |row, before| {
            let Some(this) = weak.upgrade() else { return };
            let group = |row: &gtk::ListBoxRow| {
                let shown = this.shown.borrow();
                shown.get(row.index() as usize).map(|&i| this.items[i].group)
            };
            let heading = (this.entry.text().trim().is_empty() && group(row) != before.and_then(group)).then(|| group(row)).flatten();
            row.set_header(heading.map(|g| {
                gtk::Label::builder().label(g).xalign(0.0).margin_start(12).margin_top(12).margin_bottom(3).css_classes(["heading", "dim-label"]).build()
            }).as_ref());
        });
    }

    fn refilter(&self) {
        let query = self.entry.text().trim().to_string();
        let mut scored: Vec<(i64, usize, Vec<usize>)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                fuzzy::score(&query, &item.label)
                    .or_else(|| fuzzy::score(&query, &item.detail).map(|(s, _)| (s - 40, Vec::new())))
                    .map(|(s, pos)| (s, i, pos))
            })
            .collect();
        if !query.is_empty() {
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }
        scored.truncate(MAX_ROWS);

        self.list.remove_all();
        for (_, i, positions) in &scored {
            self.list.append(&row(&self.items[*i], positions));
        }
        *self.shown.borrow_mut() = scored.iter().map(|(_, i, _)| *i).collect();
        self.list.invalidate_headers();
        if let Some(first) = self.list.row_at_index(0) {
            self.list.select_row(Some(&first));
        }
        self.scrolled.vadjustment().set_value(0.0);
    }

    fn move_selection(&self, step: i32) {
        let count = self.shown.borrow().len() as i32;
        if count == 0 {
            return;
        }
        let at = self.list.selected_row().map_or(-1, |r| r.index());
        let Some(row) = self.list.row_at_index((at + step).clamp(0, count - 1)) else { return };
        self.list.select_row(Some(&row));
        // Keep it in view.
        if let Some(bounds) = row.compute_bounds(&self.list) {
            let adj = self.scrolled.vadjustment();
            let (top, bottom) = (bounds.y() as f64, (bounds.y() + bounds.height()) as f64);
            if top < adj.value() {
                adj.set_value(top);
            } else if bottom > adj.value() + adj.page_size() {
                adj.set_value(bottom - adj.page_size());
            }
        }
    }

    fn pick(&self, row: Option<gtk::ListBoxRow>) {
        let Some(row) = row else { return };
        let Some(run) = self.shown.borrow().get(row.index() as usize).map(|&i| self.items[i].run.clone()) else { return };
        // Close first, so focus goes back to the window before the item moves it.
        self.dialog.close();
        run();
    }
}

/// A row: the label, with the letters the search matched in bold, and the detail.
fn row(item: &Item, positions: &[usize]) -> gtk::ListBoxRow {
    let mut markup = String::new();
    for (i, c) in item.label.chars().enumerate() {
        let escaped = glib::markup_escape_text(&c.to_string());
        if positions.contains(&i) {
            markup.push_str(&format!("<b>{escaped}</b>"));
        } else {
            markup.push_str(&escaped);
        }
    }
    let label = gtk::Label::builder()
        .label(markup)
        .use_markup(true)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .xalign(0.0)
        .hexpand(true)
        .build();
    let detail = gtk::Label::builder()
        .label(&item.detail)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(36)
        .xalign(1.0)
        .css_classes(["dim-label"])
        .build();
    let content = gtk::Box::builder().spacing(12).margin_top(6).margin_bottom(6).margin_start(6).margin_end(6).build();
    content.append(&label);
    content.append(&detail);
    gtk::ListBoxRow::builder().child(&content).build()
}

/// The input text for picking a command from its usage: everything up to the first
/// placeholder or choice, plus a space if arguments follow. (As the terminal client does.)
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

#[cfg(test)]
mod tests {
    use super::insert_for;

    #[test]
    fn commands_insert_up_to_their_arguments() {
        assert_eq!(insert_for("/log"), "/log");
        assert_eq!(insert_for("/set [<name> [<value>]]"), "/set ");
        assert_eq!(insert_for("/script reload"), "/script reload");
        assert_eq!(insert_for("/script load <path>"), "/script load ");
        assert_eq!(insert_for("/@<code>"), "/@");
        assert_eq!(insert_for("/images on|off"), "/images ");
    }
}
