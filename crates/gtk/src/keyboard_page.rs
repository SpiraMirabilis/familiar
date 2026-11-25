//! The Keyboard page of Preferences: every shortcut, grouped as the engine's catalog groups
//! them, with keys to add, remove and reset. Changes apply (and are saved) as they're made.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use familiar_engine::KeyChord;
use familiar_engine::keymap::{self, ActionView, Platform};

use crate::app::AppState;

/// The input box's own keys, which can't be changed.
const FIXED: &[(&str, &str)] = &[
    ("Enter", "Send"),
    ("Shift+Enter", "New line"),
    ("Up / Down", "Input history (Ctrl+Up and Ctrl+Down in a multi-line input)"),
    ("Page Up / Page Down", "Scroll the output"),
    ("Escape", "Clear the input"),
];

/// One action's rows.
struct ActionRows {
    id: &'static str,
    label: &'static str,
    expander: adw::ExpanderRow,
    /// Its keys, on the right.
    summary: gtk::Label,
    warning: gtk::Image,
    reset: gtk::Button,
    /// The rows inside the expander, rebuilt when its keys change.
    rows: RefCell<Vec<gtk::Widget>>,
}

struct KeyboardPage {
    page: adw::PreferencesPage,
    dialog: glib::WeakRef<adw::PreferencesDialog>,
    state: Rc<AppState>,
    actions: Vec<ActionRows>,
    on_change: Rc<dyn Fn()>,
}

/// The page, for `dialog`. `on_change` runs after each saved change.
pub fn page(dialog: &adw::PreferencesDialog, state: Rc<AppState>, on_change: Rc<dyn Fn()>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .name("keyboard")
        .title("Keyboard")
        .icon_name("input-keyboard-symbolic")
        .description("Shortcuts come before keyboard macros: a macro on a key a shortcut uses doesn't run.")
        .build();

    let mut groups: Vec<(&str, adw::PreferencesGroup)> = Vec::new();
    let mut actions = Vec::new();
    for action in keymap::actions(Platform::Linux) {
        let group = match groups.iter().find(|(name, _)| *name == action.group) {
            Some((_, group)) => group.clone(),
            None => {
                let group = adw::PreferencesGroup::builder().title(action.group).build();
                page.add(&group);
                groups.push((action.group, group.clone()));
                group
            }
        };
        let summary = gtk::Label::builder().css_classes(["dim-label"]).ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(28).build();
        let warning = gtk::Image::builder().icon_name("dialog-warning-symbolic").css_classes(["warning"]).visible(false).build();
        let reset = gtk::Button::builder().icon_name("edit-undo-symbolic").valign(gtk::Align::Center).css_classes(["flat"]).visible(false).build();
        let expander = adw::ExpanderRow::builder().title(action.label).use_markup(false).build();
        // Each suffix goes before the last: this shows the warning, the keys, then reset.
        expander.add_suffix(&reset);
        expander.add_suffix(&summary);
        expander.add_suffix(&warning);
        group.add(&expander);
        actions.push(ActionRows { id: action.id, label: action.label, expander, summary, warning, reset, rows: RefCell::default() });
    }

    let fixed = adw::PreferencesGroup::builder().title("Input Box").description("These keys can't be changed.").build();
    for (keys, what) in FIXED {
        let row = adw::ActionRow::builder().title(*what).use_markup(false).build();
        row.add_suffix(&gtk::Label::builder().label(*keys).css_classes(["dim-label"]).build());
        fixed.add(&row);
    }
    page.add(&fixed);

    let reset_all = adw::ButtonRow::builder().title("Reset All Shortcuts").css_classes(["destructive-action"]).build();
    let reset_group = adw::PreferencesGroup::new();
    reset_group.add(&reset_all);
    page.add(&reset_group);

    let this = Rc::new(KeyboardPage { page: page.clone(), dialog: dialog.downgrade(), state, actions, on_change });
    for rows in &this.actions {
        let weak = Rc::downgrade(&this);
        let id = rows.id;
        rows.reset.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                this.set(id, None);
            }
        });
    }
    let weak = Rc::downgrade(&this);
    reset_all.connect_activated(move |_| {
        if let Some(this) = weak.upgrade() {
            this.confirm_reset_all();
        }
    });
    this.refresh();

    let keep = RefCell::new(Some(this.clone()));
    dialog.connect_closed(move |_| {
        keep.borrow_mut().take();
    });
    page
}

impl KeyboardPage {
    fn toast(&self, text: &str) {
        if let Some(dialog) = self.dialog.upgrade() {
            let toast = adw::Toast::new(text);
            toast.set_use_markup(false);
            dialog.add_toast(toast);
        }
    }

    /// Shows every action's keys and what they clash with.
    fn refresh(self: &Rc<Self>) {
        let views = self.state.keymap().describe(&self.state.config.borrow());
        for rows in &self.actions {
            if let Some(view) = views.iter().find(|v| v.id == rows.id) {
                self.show(rows, view);
            }
        }
    }

    fn show(self: &Rc<Self>, rows: &ActionRows, view: &ActionView) {
        let labels: Vec<String> = view.keys.iter().map(|k| label(&k.key)).collect();
        rows.summary.set_label(&if labels.is_empty() { "None".to_string() } else { labels.join(", ") });
        let all_notes: Vec<String> = view.keys.iter().filter_map(|k| notes(k).map(|n| format!("{}: {n}", label(&k.key)))).collect();
        rows.warning.set_visible(!all_notes.is_empty());
        rows.warning.set_tooltip_text(Some(&all_notes.join("\n")));
        rows.reset.set_visible(view.changed);
        let defaults: Vec<String> = view.defaults.iter().map(|k| label(k)).collect();
        rows.reset.set_tooltip_text(Some(&if defaults.is_empty() {
            "Reset (no key)".to_string()
        } else {
            format!("Reset to {}", defaults.join(", "))
        }));

        for row in rows.rows.borrow_mut().drain(..) {
            rows.expander.remove(&row);
        }
        let mut added: Vec<gtk::Widget> = Vec::new();
        for key in &view.keys {
            let row = adw::ActionRow::builder().title(label(&key.key)).use_markup(false).build();
            if let Some(notes) = notes(key) {
                row.set_subtitle(&notes);
                row.add_prefix(&gtk::Image::builder().icon_name("dialog-warning-symbolic").css_classes(["warning"]).build());
            }
            let remove = gtk::Button::builder()
                .icon_name("list-remove-symbolic")
                .tooltip_text("Remove This Key")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            let weak = Rc::downgrade(self);
            let (id, removed) = (rows.id, key.key.clone());
            let keys: Vec<String> = view.keys.iter().map(|k| k.key.clone()).collect();
            remove.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    let rest: Vec<String> = keys.iter().filter(|k| **k != removed).cloned().collect();
                    this.set(id, Some(rest));
                }
            });
            row.add_suffix(&remove);
            added.push(row.upcast());
        }
        let add = adw::ButtonRow::builder().title("Add a Key…").start_icon_name("list-add-symbolic").build();
        let weak = Rc::downgrade(self);
        let (id, name) = (rows.id, rows.label);
        let keys: Vec<String> = view.keys.iter().map(|k| k.key.clone()).collect();
        add.connect_activated(move |_| {
            if let Some(this) = weak.upgrade() {
                this.record(id, name, keys.clone());
            }
        });
        added.push(add.upcast());
        for row in &added {
            rows.expander.add_row(row);
        }
        *rows.rows.borrow_mut() = added;
    }

    /// Sets an action's keys (None: back to its defaults), saves, and shows the result.
    fn set(self: &Rc<Self>, id: &str, keys: Option<Vec<String>>) {
        let result = keymap::set_keys(&mut self.state.config.borrow_mut().settings, Platform::Linux, id, keys.as_deref());
        if let Err(e) = result {
            return self.toast(&e);
        }
        match self.state.save() {
            Ok(()) => (self.on_change)(),
            Err(e) => self.toast(&e),
        }
        self.refresh();
    }

    /// Asks for a key combination and adds it to the action's keys. While this is open, the
    /// main window's shortcuts don't run (a dialog is showing).
    fn record(self: &Rc<Self>, id: &'static str, label: &str, keys: Vec<String>) {
        let dialog = adw::AlertDialog::new(Some("Press a Key"), Some(&format!("Press the keys for “{label}”, or Escape to cancel.")));
        dialog.add_response("cancel", "Cancel");
        dialog.set_close_response("cancel");
        let controller = gtk::EventControllerKey::new();
        controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        let shown = dialog.downgrade();
        controller.connect_key_pressed(move |controller, key, keycode, state| {
            // A modifier on its own: wait for the rest.
            let Some(chord) = crate::keys::chord(controller, key, keycode, state) else { return glib::Propagation::Stop };
            if let Some(dialog) = shown.upgrade() {
                dialog.close();
            }
            let escape = KeyChord::parse("Escape").is_some_and(|e| e.matches(&chord));
            if let Some(this) = weak.upgrade()
                && !escape
            {
                let mut keys = keys.clone();
                keys.push(chord.to_string());
                this.set(id, Some(keys));
            }
            glib::Propagation::Stop
        });
        dialog.add_controller(controller);
        dialog.present(Some(&self.page));
    }

    fn confirm_reset_all(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::new(Some("Reset All Shortcuts?"), Some("Every shortcut goes back to its default keys."));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("reset", "Reset All");
        dialog.set_response_appearance("reset", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        dialog.connect_response(Some("reset"), move |_, _| {
            let Some(this) = weak.upgrade() else { return };
            this.state.config.borrow_mut().settings.keys.clear();
            match this.state.save() {
                Ok(()) => (this.on_change)(),
                Err(e) => this.toast(&e),
            }
            this.refresh();
        });
        dialog.present(Some(&self.page));
    }
}

/// A key as GTK shows it ("Ctrl+Page Down"), from its config spelling.
fn label(key: &str) -> String {
    KeyChord::parse(key).map_or_else(|| key.to_string(), |chord| crate::keys::label(&chord))
}

/// What else is on a key: other shortcuts, and macros that won't run because of it.
fn notes(key: &keymap::KeyView) -> Option<String> {
    let mut notes: Vec<String> = key.clashes.iter().map(|other| format!("Also: {other}")).collect();
    notes.extend(key.macros.iter().map(|whose| format!("Macro: {whose} (it won't run while this shortcut has the key)")));
    (!notes.is_empty()).then(|| notes.join(" · "))
}
