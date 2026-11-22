//! Editor for triggers, aliases, keyboard macros and variables, at any scope.
//!
//! One dialog with a navigation view: the lists are the root page, and editing an item pushes a
//! page instead of stacking another dialog.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use familiar_config::{Alias, Config, Highlight, Macro, Pattern, Rules, Scope, Trigger};
use familiar_engine::KeyChord;

use crate::app::AppState;

const TABS: [(&str, &str, &str); 4] = [
    ("triggers", "Triggers", "edit-find-symbolic"),
    ("aliases", "Aliases", "mail-forward-symbolic"),
    ("macros", "Macros", "input-keyboard-symbolic"),
    ("variables", "Variables", "text-x-generic-symbolic"),
];

struct RulesDialog {
    dialog: adw::Dialog,
    nav: adw::NavigationView,
    toasts: adw::ToastOverlay,
    state: Rc<AppState>,
    scopes: Vec<Scope>,
    scope: Cell<usize>,
    pages: Vec<adw::PreferencesPage>,
    groups: RefCell<Vec<(adw::PreferencesPage, adw::PreferencesGroup)>>,
    on_changed: Box<dyn Fn()>,
}

/// Every scope in the config: all worlds, then each world followed by its characters.
fn all_scopes(config: &Config) -> Vec<Scope> {
    let mut scopes = vec![Scope::Global];
    for world in &config.worlds {
        scopes.push(Scope::World { world: world.name.clone() });
        for c in &world.characters {
            scopes.push(Scope::Character { world: world.name.clone(), character: c.name.clone() });
        }
    }
    scopes
}

/// Opens the editor showing `preferred` (or the global rules). `on_changed` runs after every
/// saved change, so open sessions can reload.
pub fn show(parent: &impl IsA<gtk::Widget>, state: Rc<AppState>, preferred: Option<Scope>, on_changed: impl Fn() + 'static) {
    let scopes = all_scopes(&state.config.borrow());
    let selected = preferred.and_then(|p| scopes.iter().position(|s| *s == p)).unwrap_or(0);

    let stack = adw::ViewStack::new();
    let pages: Vec<adw::PreferencesPage> = TABS
        .iter()
        .map(|(name, title, icon)| {
            let page = adw::PreferencesPage::new();
            stack.add_titled_with_icon(&page, Some(name), title, icon);
            page
        })
        .collect();
    let switcher = adw::ViewSwitcher::builder().stack(&stack).policy(adw::ViewSwitcherPolicy::Wide).build();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&switcher));

    let labels: Vec<String> = scopes
        .iter()
        .map(|s| match s {
            Scope::Character { character, .. } => format!("    {character}"),
            other => other.label(),
        })
        .collect();
    let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let scope_dropdown = gtk::DropDown::from_strings(&label_refs);
    scope_dropdown.set_selected(selected as u32);
    let scope_bar = gtk::Box::builder()
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    scope_bar.append(&gtk::Label::new(Some("Applies to")));
    scope_bar.append(&scope_dropdown);

    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&stack));
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_top_bar(&scope_bar);
    toolbar.set_content(Some(&toasts));

    let nav = adw::NavigationView::new();
    nav.add(&adw::NavigationPage::builder().title("Rules").tag("root").child(&toolbar).build());
    let dialog = adw::Dialog::builder()
        .title("Triggers, Aliases & Macros")
        .content_width(680)
        .content_height(760)
        .child(&nav)
        .build();

    let this = Rc::new(RulesDialog {
        dialog,
        nav,
        toasts,
        state,
        scopes,
        scope: Cell::new(selected),
        pages,
        groups: RefCell::default(),
        on_changed: Box::new(on_changed),
    });
    this.refresh();

    let weak = Rc::downgrade(&this);
    scope_dropdown.connect_selected_notify(move |dd| {
        if let Some(this) = weak.upgrade() {
            this.scope.set(dd.selected() as usize);
            this.refresh();
        }
    });

    let keep = RefCell::new(Some(this.clone()));
    this.dialog.connect_closed(move |_| {
        keep.borrow_mut().take();
    });
    this.dialog.present(Some(parent));
}

fn summary(t: &Trigger) -> String {
    let mut parts = Vec::new();
    if t.matcher.regex {
        parts.push("regex".to_string());
    }
    if t.gag {
        parts.push("hide".into());
    }
    if t.highlight.is_some() {
        parts.push("highlight".into());
    }
    if !t.send.is_empty() {
        parts.push(format!("send \"{}\"", t.send.lines().next().unwrap_or("")));
    }
    if !t.spawn.is_empty() {
        parts.push(format!("→ {}", t.spawn));
    }
    if !t.sound.is_empty() {
        parts.push("sound".into());
    }
    if t.notify {
        parts.push("notify".into());
    }
    if !t.script.is_empty() {
        parts.push(format!("{}()", t.script));
    }
    if !t.speak.is_empty() {
        parts.push("speak".into());
    }
    if t.stop {
        parts.push("stop".into());
    }
    parts.join(" · ")
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    button.update_property(&[gtk::accessible::Property::Label(tooltip)]);
    button
}

/// A multi-line text box for a preferences group.
fn text_area(text: &str) -> (gtk::Frame, gtk::TextView) {
    let view = gtk::TextView::builder()
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .build();
    view.buffer().set_text(text);
    let scrolled = gtk::ScrolledWindow::builder().child(&view).min_content_height(72).build();
    let frame = gtk::Frame::builder().child(&scrolled).css_classes(["view"]).build();
    (frame, view)
}

fn text_of(view: &gtk::TextView) -> String {
    let buffer = view.buffer();
    buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).trim_end().to_string()
}

fn hex_to_rgba(hex: Option<&str>, fallback: &str) -> gdk::RGBA {
    gdk::RGBA::parse(hex.unwrap_or(fallback)).unwrap_or(gdk::RGBA::RED)
}

fn rgba_to_hex(c: &gdk::RGBA) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c.red()), b(c.green()), b(c.blue()))
}

impl RulesDialog {
    fn toast(&self, text: &str) {
        let toast = adw::Toast::new(text);
        toast.set_use_markup(false);
        self.toasts.add_toast(toast);
    }

    fn scope(&self) -> &Scope {
        &self.scopes[self.scope.get()]
    }

    fn rules(&self) -> Rules {
        self.state.config.borrow().rules(self.scope()).cloned().unwrap_or_default()
    }

    /// Changes the rules at the current scope, saves, and tells open sessions.
    fn update(self: &Rc<Self>, f: impl FnOnce(&mut Rules)) {
        {
            let mut config = self.state.config.borrow_mut();
            let Some(rules) = config.rules_mut(self.scope()) else {
                drop(config);
                self.toast("That world or character no longer exists");
                return;
            };
            f(rules);
        }
        if let Err(e) = self.state.save() {
            self.toast(&e);
        }
        (self.on_changed)();
        self.refresh();
    }

    fn refresh(self: &Rc<Self>) {
        for (page, group) in self.groups.borrow_mut().drain(..) {
            page.remove(&group);
        }
        let rules = self.rules();
        let groups = vec![
            (self.pages[0].clone(), self.trigger_list(&rules)),
            (self.pages[1].clone(), self.alias_list(&rules)),
            (self.pages[2].clone(), self.macro_list(&rules)),
            (self.pages[3].clone(), self.variable_list(&rules)),
            (self.pages[3].clone(), self.variable_form()),
        ];
        for (page, group) in &groups {
            page.add(group);
        }
        *self.groups.borrow_mut() = groups;
    }

    fn list_group(self: &Rc<Self>, title: &str, description: &str, add_tooltip: &str, on_add: impl Fn(&Rc<Self>) + 'static) -> adw::PreferencesGroup {
        let add = icon_button("list-add-symbolic", add_tooltip);
        let weak = Rc::downgrade(self);
        add.connect_clicked(move |_| {
            if let Some(this) = weak.upgrade() {
                on_add(&this);
            }
        });
        adw::PreferencesGroup::builder().title(title).description(description).header_suffix(&add).build()
    }

    /// A list row with an enabled switch that opens `on_open` when activated.
    fn list_row(
        self: &Rc<Self>,
        title: &str,
        subtitle: &str,
        enabled: Option<(bool, Box<dyn Fn(&Rc<Self>, bool)>)>,
        on_open: impl Fn(&Rc<Self>) + 'static,
    ) -> adw::ActionRow {
        let row = adw::ActionRow::builder()
            .title(title)
            .subtitle(subtitle)
            .use_markup(false)
            .activatable(true)
            .build();
        if let Some((active, on_toggle)) = enabled {
            let switch = gtk::Switch::builder().active(active).valign(gtk::Align::Center).tooltip_text("Enabled").build();
            let weak = Rc::downgrade(self);
            switch.connect_active_notify(move |s| {
                if let Some(this) = weak.upgrade() {
                    on_toggle(&this, s.is_active());
                }
            });
            row.add_prefix(&switch);
        }
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let weak = Rc::downgrade(self);
        row.connect_activated(move |_| {
            if let Some(this) = weak.upgrade() {
                on_open(&this);
            }
        });
        row
    }

    fn trigger_list(self: &Rc<Self>, rules: &Rules) -> adw::PreferencesGroup {
        let group = self.list_group(
            "Triggers",
            "Run on each line from the server. Character triggers run first, then world, then all-world ones.",
            "Add trigger",
            |this| this.edit_trigger(None),
        );
        for (i, t) in rules.triggers.iter().enumerate() {
            let title = if t.name.is_empty() { &t.matcher.pattern } else { &t.name };
            let toggle: Box<dyn Fn(&Rc<Self>, bool)> = Box::new(move |this, on| {
                this.update(|r| {
                    if let Some(t) = r.triggers.get_mut(i) {
                        t.enabled = on;
                    }
                })
            });
            group.add(&self.list_row(title, &summary(t), Some((t.enabled, toggle)), move |this| this.edit_trigger(Some(i))));
        }
        group
    }

    fn alias_list(self: &Rc<Self>, rules: &Rules) -> adw::PreferencesGroup {
        let group = self.list_group(
            "Aliases",
            "Replace what you type before it is sent. The first matching alias wins.",
            "Add alias",
            |this| this.edit_alias(None),
        );
        for (i, a) in rules.aliases.iter().enumerate() {
            let toggle: Box<dyn Fn(&Rc<Self>, bool)> = Box::new(move |this, on| {
                this.update(|r| {
                    if let Some(a) = r.aliases.get_mut(i) {
                        a.enabled = on;
                    }
                })
            });
            let subtitle = a.replacement.replace('\n', " ⏎ ");
            group.add(&self.list_row(&a.matcher.pattern, &subtitle, Some((a.enabled, toggle)), move |this| this.edit_alias(Some(i))));
        }
        group
    }

    fn macro_list(self: &Rc<Self>, rules: &Rules) -> adw::PreferencesGroup {
        let group = self.list_group(
            "Keyboard Macros",
            "Send a command when you press a key. Aliases apply to it.",
            "Add macro",
            |this| this.edit_macro(None),
        );
        for (i, m) in rules.macros.iter().enumerate() {
            group.add(&self.list_row(&m.key, &m.command, None, move |this| this.edit_macro(Some(i))));
        }
        group
    }

    fn variable_list(self: &Rc<Self>, rules: &Rules) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::builder()
            .title("Variables")
            .description("Use as %name% in trigger sends and alias replacements. You can also type /set name value.")
            .build();
        if rules.variables.is_empty() {
            group.add(&adw::ActionRow::builder().title("No variables here").css_classes(["dim-label"]).build());
        }
        for (name, value) in &rules.variables {
            let row = adw::ActionRow::builder()
                .title(format!("%{name}%"))
                .subtitle(value)
                .use_markup(false)
                .build();
            let delete = icon_button("user-trash-symbolic", "Remove variable");
            let weak = Rc::downgrade(self);
            let name = name.clone();
            delete.connect_clicked(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.update(|r| {
                        r.variables.remove(&name);
                    });
                }
            });
            row.add_suffix(&delete);
            group.add(&row);
        }
        group
    }

    fn variable_form(self: &Rc<Self>) -> adw::PreferencesGroup {
        let name = adw::EntryRow::builder().title("Name").build();
        let value = adw::EntryRow::builder().title("Value").build();
        let set = adw::ButtonRow::builder().title("Set Variable").build();
        let group = adw::PreferencesGroup::builder().title("Set a Variable").build();
        group.add(&name);
        group.add(&value);
        group.add(&set);
        let weak = Rc::downgrade(self);
        set.connect_activated(move |_| {
            let Some(this) = weak.upgrade() else { return };
            let n = name.text().trim().trim_matches('%').to_string();
            if n.is_empty() || !n.chars().all(|c| c.is_alphanumeric() || c == '_') {
                this.toast("Variable names can only contain letters, digits and _");
                return;
            }
            let v = value.text().to_string();
            this.update(|r| {
                r.variables.insert(n, v);
            });
        });
        group
    }

    /// Pushes an editor page with Save in the header and an optional Delete button.
    fn push_editor(
        self: &Rc<Self>,
        title: &str,
        content: &adw::PreferencesPage,
        on_save: impl Fn(&Rc<Self>) -> Result<(), String> + 'static,
        on_delete: Option<Box<dyn Fn(&Rc<Self>)>>,
    ) -> adw::NavigationPage {
        if let Some(on_delete) = on_delete {
            let delete = adw::ButtonRow::builder().title(format!("Delete {}", title.trim_start_matches("Edit "))).css_classes(["destructive-action"]).build();
            let group = adw::PreferencesGroup::new();
            group.add(&delete);
            content.add(&group);
            let weak = Rc::downgrade(self);
            delete.connect_activated(move |_| {
                if let Some(this) = weak.upgrade() {
                    on_delete(&this);
                    this.nav.pop();
                }
            });
        }
        let save = gtk::Button::builder().label("Save").css_classes(["suggested-action"]).build();
        let header = adw::HeaderBar::new();
        header.pack_end(&save);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(content));
        let page = adw::NavigationPage::builder().title(title).child(&toolbar).build();
        let weak = Rc::downgrade(self);
        save.connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else { return };
            match on_save(&this) {
                Ok(()) => {
                    this.nav.pop();
                }
                Err(e) => this.toast(&e),
            }
        });
        self.nav.push(&page);
        page
    }

    fn edit_trigger(self: &Rc<Self>, index: Option<usize>) {
        let t = index.and_then(|i| self.rules().triggers.get(i).cloned()).unwrap_or_default();

        let name = adw::EntryRow::builder().title("Name (optional)").text(&t.name).build();
        let pattern = adw::EntryRow::builder().title("Pattern").text(&t.matcher.pattern).build();
        let regex = adw::SwitchRow::builder().title("Regular expression").active(t.matcher.regex).build();
        let case = adw::SwitchRow::builder().title("Match case").active(t.matcher.case_sensitive).build();
        let whole = adw::SwitchRow::builder().title("Whole words only").active(t.matcher.whole_word).build();
        regex.bind_property("active", &whole, "sensitive").invert_boolean().sync_create().build();
        let enabled = adw::SwitchRow::builder().title("Enabled").active(t.enabled).build();
        let stop = adw::SwitchRow::builder()
            .title("Stop here")
            .subtitle("Don't run later triggers on lines this one matches")
            .active(t.stop)
            .build();
        let matching = adw::PreferencesGroup::builder().title("Match").build();
        for row in [name.upcast_ref::<gtk::Widget>(), pattern.upcast_ref(), regex.upcast_ref(), case.upcast_ref(), whole.upcast_ref(), enabled.upcast_ref(), stop.upcast_ref()] {
            matching.add(row);
        }

        let test_input = adw::EntryRow::builder().title("Test with a line from the game").build();
        let test_result = adw::ActionRow::builder().title("Result").subtitle("Type a line above").use_markup(false).build();
        test_result.add_css_class("property");
        let test = adw::PreferencesGroup::builder().title("Test").build();
        test.add(&test_input);
        test.add(&test_result);

        let gag = adw::SwitchRow::builder().title("Hide the line").active(t.gag).build();
        let gag_log = adw::SwitchRow::builder().title("Leave it out of the log").active(t.gag_log).build();
        let h = t.highlight.clone().unwrap_or_default();
        let highlight = adw::ExpanderRow::builder()
            .title("Highlight")
            .show_enable_switch(true)
            .enable_expansion(t.highlight.is_some())
            .expanded(t.highlight.is_some())
            .build();
        let color_row = |title: &str, hex: Option<&str>, fallback: &str| {
            let check = gtk::CheckButton::builder().active(hex.is_some()).valign(gtk::Align::Center).build();
            let button = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::new()));
            button.set_rgba(&hex_to_rgba(hex, fallback));
            button.set_valign(gtk::Align::Center);
            let row = adw::ActionRow::builder().title(title).activatable_widget(&check).build();
            row.add_prefix(&check);
            row.add_suffix(&button);
            highlight.add_row(&row);
            (check, button)
        };
        let (fg_on, fg) = color_row("Text colour", h.fg.as_deref(), "#ffd700");
        let (bg_on, bg) = color_row("Background colour", h.bg.as_deref(), "#3a3a7a");
        let bold = adw::SwitchRow::builder().title("Bold").active(h.bold).build();
        let italic = adw::SwitchRow::builder().title("Italic").active(h.italic).build();
        let underline = adw::SwitchRow::builder().title("Underline").active(h.underline).build();
        let whole_line = adw::SwitchRow::builder()
            .title("Whole line")
            .subtitle("Otherwise only the matching text")
            .active(h.whole_line)
            .build();
        let groups = adw::SwitchRow::builder()
            .title("Only the groups")
            .subtitle("With a regular expression that has groups, style just those")
            .active(h.groups)
            .build();
        for row in [&bold, &italic, &underline, &whole_line, &groups] {
            highlight.add_row(row);
        }
        let replace = adw::ExpanderRow::builder()
            .title("Replace the matching text")
            .subtitle("For example to censor words; $1 and %name% work")
            .show_enable_switch(true)
            .enable_expansion(t.replace.is_some())
            .expanded(t.replace.is_some())
            .build();
        let replace_with = adw::EntryRow::builder().title("With (empty removes it)").text(t.replace.as_deref().unwrap_or("")).build();
        replace.add_row(&replace_with);
        let display = adw::PreferencesGroup::builder().title("Display").build();
        display.add(&gag);
        display.add(&gag_log);
        display.add(&highlight);
        display.add(&replace);

        let (send_box, send) = text_area(&t.send);
        let send_group = adw::PreferencesGroup::builder()
            .title("Send")
            .description("Commands to send, one per line. $1…$9 and ${name} are regex groups, %name% is a variable.")
            .build();
        send_group.add(&send_box);

        let spawn = adw::EntryRow::builder().title("Copy to spawn window").text(&t.spawn).build();
        let sound = adw::EntryRow::builder().title("Play sound file").text(&t.sound).build();
        let browse = icon_button("document-open-symbolic", "Choose a sound file");
        sound.add_suffix(&browse);
        {
            let sound = sound.clone();
            browse.connect_clicked(move |button| {
                let window = button.root().and_downcast::<gtk::Window>();
                let sound = sound.clone();
                gtk::FileDialog::builder().title("Choose a Sound").build().open(
                    window.as_ref(),
                    None::<&gio::Cancellable>,
                    move |result| {
                        if let Ok(file) = result
                            && let Some(path) = file.path()
                        {
                            sound.set_text(&path.to_string_lossy());
                        }
                    },
                );
            });
        }
        let script = adw::EntryRow::builder().title("Call script function").text(&t.script).build();
        let speak = adw::EntryRow::builder().title("Speak aloud ($0 is the whole line)").text(&t.speak).build();
        let notify = adw::SwitchRow::builder().title("Desktop notification").active(t.notify).build();
        let important = adw::SwitchRow::builder()
            .title("Mark the tab as important")
            .subtitle("Shows a star on the tab until you look at it")
            .active(t.important)
            .build();
        let actions = adw::PreferencesGroup::builder().title("Also").build();
        for row in [spawn.upcast_ref::<gtk::Widget>(), sound.upcast_ref(), speak.upcast_ref(), script.upcast_ref(), notify.upcast_ref(), important.upcast_ref()] {
            actions.add(row);
        }

        let content = adw::PreferencesPage::new();
        for group in [&matching, &test, &display, &send_group, &actions] {
            content.add(group);
        }

        // Widgets that change the test result. Cloned before `build` takes the originals.
        let watched_entries = [test_input.clone(), pattern.clone()];
        let watched_switches = [regex.clone(), case.clone(), whole.clone()];
        let send_buffer = send.buffer();

        let build = Rc::new(move || Trigger {
            name: name.text().trim().to_string(),
            enabled: enabled.is_active(),
            matcher: Pattern {
                pattern: pattern.text().to_string(),
                regex: regex.is_active(),
                case_sensitive: case.is_active(),
                whole_word: whole.is_active(),
            },
            stop: stop.is_active(),
            gag: gag.is_active(),
            gag_log: gag_log.is_active(),
            highlight: highlight.enables_expansion().then(|| Highlight {
                fg: fg_on.is_active().then(|| rgba_to_hex(&fg.rgba())),
                bg: bg_on.is_active().then(|| rgba_to_hex(&bg.rgba())),
                bold: bold.is_active(),
                italic: italic.is_active(),
                underline: underline.is_active(),
                whole_line: whole_line.is_active(),
                groups: groups.is_active(),
            }),
            replace: replace.enables_expansion().then(|| replace_with.text().to_string()),
            send: text_of(&send),
            spawn: spawn.text().trim().to_string(),
            sound: sound.text().trim().to_string(),
            notify: notify.is_active(),
            important: important.is_active(),
            script: script.text().trim().to_string(),
            speak: speak.text().trim().to_string(),
        });

        // Update the test result whenever the line or the pattern changes.
        let run_test = {
            let build = build.clone();
            let test_input = test_input.clone();
            let test_result = test_result.clone();
            Rc::new(move || {
                let line = test_input.text();
                let text = if line.is_empty() {
                    "Type a line above".to_string()
                } else {
                    match familiar_engine::rules::test_trigger(&build(), &line) {
                        Ok(Some(description)) => description,
                        Ok(None) => "No match".into(),
                        Err(e) => format!("Invalid pattern: {e}"),
                    }
                };
                test_result.set_subtitle(&text);
            })
        };
        for entry in &watched_entries {
            let run_test = run_test.clone();
            entry.connect_changed(move |_| run_test());
        }
        for switch in &watched_switches {
            let run_test = run_test.clone();
            switch.connect_active_notify(move |_| run_test());
        }
        send_buffer.connect_changed(move |_| run_test());

        let on_delete: Option<Box<dyn Fn(&Rc<Self>)>> = index.map(|i| {
            Box::new(move |this: &Rc<Self>| {
                this.update(|r| {
                    if i < r.triggers.len() {
                        r.triggers.remove(i);
                    }
                })
            }) as Box<dyn Fn(&Rc<Self>)>
        });
        self.push_editor(
            if index.is_some() { "Edit Trigger" } else { "New Trigger" },
            &content,
            move |this| {
                let trigger = build();
                familiar_engine::rules::compile_pattern(&trigger.matcher).map_err(|e| format!("Invalid pattern: {e}"))?;
                this.update(|r| match index {
                    Some(i) if i < r.triggers.len() => r.triggers[i] = trigger,
                    _ => r.triggers.push(trigger),
                });
                Ok(())
            },
            on_delete,
        );
    }

    fn edit_alias(self: &Rc<Self>, index: Option<usize>) {
        let a = index.and_then(|i| self.rules().aliases.get(i).cloned()).unwrap_or_default();
        let pattern = adw::EntryRow::builder().title("Pattern").text(&a.matcher.pattern).build();
        let regex = adw::SwitchRow::builder().title("Regular expression").active(a.matcher.regex).build();
        let case = adw::SwitchRow::builder().title("Match case").active(a.matcher.case_sensitive).build();
        let enabled = adw::SwitchRow::builder().title("Enabled").active(a.enabled).build();
        let substitute = adw::SwitchRow::builder()
            .title("Replace just the matching text")
            .subtitle("Everywhere it appears, then carry on with later aliases (as BeipMU does)")
            .active(a.substitute)
            .build();
        let stop = adw::SwitchRow::builder().title("Then stop").subtitle("Skip later aliases when this one matches").active(a.stop).build();
        substitute.bind_property("active", &stop, "sensitive").sync_create().build();
        let matching = adw::PreferencesGroup::builder()
            .title("Match")
            .description(
                "Plain text matches the first word you type: $1…$9 are the words after it and $* is the rest. \
                 A regular expression can match anywhere (anchor with ^ and $); $1… are its groups.",
            )
            .build();
        for row in [pattern.upcast_ref::<gtk::Widget>(), regex.upcast_ref(), case.upcast_ref(), substitute.upcast_ref(), stop.upcast_ref(), enabled.upcast_ref()] {
            matching.add(row);
        }
        let (replacement_box, replacement) = text_area(&a.replacement);
        let replace_group = adw::PreferencesGroup::builder()
            .title("Send Instead")
            .description("One command per line. %name% is a variable.")
            .build();
        replace_group.add(&replacement_box);

        let test_input = adw::EntryRow::builder().title("Test with something you'd type").build();
        let test_result = adw::ActionRow::builder().title("Sends").subtitle("Type above to try it").use_markup(false).build();
        test_result.add_css_class("property");
        let test = adw::PreferencesGroup::builder().title("Test").build();
        test.add(&test_input);
        test.add(&test_result);

        let content = adw::PreferencesPage::new();
        for group in [&matching, &replace_group, &test] {
            content.add(group);
        }

        let build = Rc::new(move || Alias {
            enabled: enabled.is_active(),
            matcher: Pattern {
                pattern: pattern.text().trim().to_string(),
                regex: regex.is_active(),
                case_sensitive: case.is_active(),
                whole_word: false,
            },
            replacement: text_of(&replacement),
            substitute: substitute.is_active(),
            stop: stop.is_active(),
        });
        {
            let build = build.clone();
            let result = test_result.clone();
            test_input.connect_changed(move |entry| {
                let input = entry.text();
                let text = if input.is_empty() {
                    "Type above to try it".to_string()
                } else {
                    match familiar_engine::rules::test_alias(&build(), &input) {
                        Ok(Some(lines)) => lines.join(" ⏎ "),
                        Ok(None) => "No match, so it's sent as typed".into(),
                        Err(e) => format!("Invalid pattern: {e}"),
                    }
                };
                result.set_subtitle(&text);
            });
        }

        let on_delete: Option<Box<dyn Fn(&Rc<Self>)>> = index.map(|i| {
            Box::new(move |this: &Rc<Self>| {
                this.update(|r| {
                    if i < r.aliases.len() {
                        r.aliases.remove(i);
                    }
                })
            }) as Box<dyn Fn(&Rc<Self>)>
        });
        self.push_editor(
            if index.is_some() { "Edit Alias" } else { "New Alias" },
            &content,
            move |this| {
                let alias = build();
                if alias.matcher.pattern.is_empty() {
                    return Err("Enter a pattern".into());
                }
                if alias.matcher.regex {
                    familiar_engine::rules::compile_pattern(&alias.matcher).map_err(|e| format!("Invalid pattern: {e}"))?;
                }
                this.update(|r| match index {
                    Some(i) if i < r.aliases.len() => r.aliases[i] = alias,
                    _ => r.aliases.push(alias),
                });
                Ok(())
            },
            on_delete,
        );
    }

    fn edit_macro(self: &Rc<Self>, index: Option<usize>) {
        let m = index.and_then(|i| self.rules().macros.get(i).cloned()).unwrap_or_default();
        let chord: Rc<RefCell<Option<KeyChord>>> = Rc::new(RefCell::new(KeyChord::parse(&m.key)));

        let key_row = adw::ActionRow::builder()
            .title("Key")
            .subtitle(chord.borrow().as_ref().map_or("Not set".to_string(), |c| c.to_string()))
            .use_markup(false)
            .build();
        let record = gtk::ToggleButton::builder().label("Record").valign(gtk::Align::Center).build();
        key_row.add_suffix(&record);
        let command = adw::EntryRow::builder().title("Command").text(&m.command).build();
        let group = adw::PreferencesGroup::builder()
            .title("Macro")
            .description("Press Record, then the key or combination you want, e.g. F5, Ctrl+K or a number-pad key.")
            .build();
        let insert = adw::SwitchRow::builder()
            .title("Put it in the input box")
            .subtitle("Instead of sending it")
            .active(m.insert)
            .build();
        group.add(&key_row);
        group.add(&command);
        group.add(&insert);
        let content = adw::PreferencesPage::new();
        content.add(&group);

        {
            let key_row = key_row.clone();
            record.connect_toggled(move |button| {
                if button.is_active() {
                    key_row.set_subtitle("Press a key…");
                }
            });
        }
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        {
            let chord = chord.clone();
            let record = record.clone();
            let key_row = key_row.clone();
            keys.connect_key_pressed(move |_, key, _, state| {
                if !record.is_active() {
                    return glib::Propagation::Proceed;
                }
                let Some(c) = crate::keys::chord(key, state) else { return glib::Propagation::Stop };
                key_row.set_subtitle(&c.to_string());
                *chord.borrow_mut() = Some(c);
                record.set_active(false);
                glib::Propagation::Stop
            });
        }
        content.add_controller(keys);

        let on_delete: Option<Box<dyn Fn(&Rc<Self>)>> = index.map(|i| {
            Box::new(move |this: &Rc<Self>| {
                this.update(|r| {
                    if i < r.macros.len() {
                        r.macros.remove(i);
                    }
                })
            }) as Box<dyn Fn(&Rc<Self>)>
        });
        self.push_editor(
            if index.is_some() { "Edit Macro" } else { "New Macro" },
            &content,
            move |this| {
                let Some(key) = chord.borrow().clone() else { return Err("Record a key first".into()) };
                let new = Macro { key: key.to_string(), command: command.text().to_string(), insert: insert.is_active() };
                this.update(|r| match index {
                    Some(i) if i < r.macros.len() => r.macros[i] = new,
                    _ => {
                        // One macro per key: replace an existing binding.
                        r.macros.retain(|m| KeyChord::parse(&m.key).is_none_or(|k| !k.matches(&key)));
                        r.macros.push(new);
                    }
                });
                Ok(())
            },
            on_delete,
        );
    }
}
