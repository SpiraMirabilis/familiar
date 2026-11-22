//! The Connect dialog: saved worlds, quick connect, and the built-in world directory.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use familiar_config::{Character, World, directory};

use crate::app::AppState;
use crate::world_dialog;

type OnConnect = Rc<dyn Fn(World, Option<Character>)>;

struct ConnectDialog {
    dialog: adw::Dialog,
    page: adw::PreferencesPage,
    worlds_group: RefCell<Option<adw::PreferencesGroup>>,
    directory_group: RefCell<Option<adw::PreferencesGroup>>,
    state: Rc<AppState>,
    on_connect: OnConnect,
    toasts: adw::ToastOverlay,
}

pub fn show(parent: &impl IsA<gtk::Widget>, state: Rc<AppState>, on_connect: impl Fn(World, Option<Character>) + 'static) {
    let page = adw::PreferencesPage::new();
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&page));
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&toasts));
    let dialog = adw::Dialog::builder()
        .title("Connect")
        .content_width(560)
        .content_height(680)
        .child(&toolbar)
        .build();

    let this = Rc::new(ConnectDialog {
        dialog,
        page,
        worlds_group: RefCell::default(),
        directory_group: RefCell::default(),
        state,
        on_connect: Rc::new(on_connect),
        toasts,
    });
    this.build_quick_connect();
    this.rebuild_worlds();
    this.build_directory();

    let keep = RefCell::new(Some(this.clone()));
    this.dialog.connect_closed(move |_| {
        keep.borrow_mut().take();
    });
    this.dialog.present(Some(parent));
}

impl ConnectDialog {
    fn connect(&self, world: World, character: Option<Character>) {
        // Close first: closing returns focus to whatever had it before the dialog opened, which
        // would otherwise steal it back from the new tab's input.
        self.dialog.close();
        (self.on_connect)(world, character);
    }

    fn save(&self) {
        if let Err(e) = self.state.save() {
            self.toasts.add_toast(adw::Toast::new(&e));
        }
    }

    fn build_quick_connect(self: &Rc<Self>) {
        let entry = adw::EntryRow::builder().title("host:port").show_apply_button(true).build();
        let tls = gtk::ToggleButton::builder()
            .icon_name("channel-secure-symbolic")
            .tooltip_text("Use TLS")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        entry.add_suffix(&tls);
        let group = adw::PreferencesGroup::builder().title("Quick Connect").build();
        group.add(&entry);
        self.page.add(&group);
        self.dialog.set_focus(Some(&entry));

        let weak = Rc::downgrade(self);
        entry.connect_apply(move |entry| {
            let Some(this) = weak.upgrade() else { return };
            match familiar_config::Address::parse(&entry.text()) {
                // Typing "tls" after the address works as well as the switch.
                Some(address) => {
                    let mut world = address.into_world(23);
                    world.tls |= tls.is_active();
                    this.connect(world, None)
                }
                None => this.toasts.add_toast(adw::Toast::new("Enter an address like example.com:4000")),
            }
        });
    }

    fn rebuild_worlds(self: &Rc<Self>) {
        if let Some(old) = self.worlds_group.borrow_mut().take() {
            self.page.remove(&old);
        }
        let add = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("Add world")
            .css_classes(["flat"])
            .build();
        let group = adw::PreferencesGroup::builder().title("Worlds").header_suffix(&add).build();

        let weak = Rc::downgrade(self);
        add.connect_clicked(move |button| {
            let Some(this) = weak.upgrade() else { return };
            let weak = Rc::downgrade(&this);
            let default_font = this.state.config.borrow().settings.font.clone();
            world_dialog::show(
                button,
                None,
                &default_font,
                move |world| {
                    let Some(this) = weak.upgrade() else { return };
                    this.state.config.borrow_mut().worlds.push(world);
                    this.save();
                    this.rebuild_worlds();
                },
                None,
            );
        });

        let worlds = self.state.config.borrow().worlds.clone();
        if worlds.is_empty() {
            group.set_description(Some("No saved worlds yet. Add one with +, or pick one from the directory below."));
        }
        for (index, world) in worlds.into_iter().enumerate() {
            group.add(&self.world_row(index, world));
        }

        self.page.add(&group);
        *self.worlds_group.borrow_mut() = Some(group);
        // PreferencesPage can only append, so move the directory back below the worlds.
        if let Some(directory) = self.directory_group.borrow().as_ref() {
            self.page.remove(directory);
            self.page.add(directory);
        }
    }

    fn world_row(self: &Rc<Self>, index: usize, world: World) -> gtk::Widget {
        let subtitle = format!("{}:{}{}", world.host, world.port, if world.tls { " · TLS" } else { "" });
        let edit = gtk::Button::builder()
            .icon_name("document-edit-symbolic")
            .tooltip_text("Edit world")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let weak = Rc::downgrade(self);
        let edited = world.clone();
        edit.connect_clicked(move |button| {
            let Some(this) = weak.upgrade() else { return };
            let on_save = {
                let weak = Rc::downgrade(&this);
                move |world| {
                    let Some(this) = weak.upgrade() else { return };
                    if let Some(slot) = this.state.config.borrow_mut().worlds.get_mut(index) {
                        *slot = world;
                    }
                    this.save();
                    this.rebuild_worlds();
                }
            };
            let on_delete = {
                let weak = Rc::downgrade(&this);
                Box::new(move || {
                    let Some(this) = weak.upgrade() else { return };
                    {
                        let mut config = this.state.config.borrow_mut();
                        if index < config.worlds.len() {
                            config.worlds.remove(index);
                        }
                    }
                    this.save();
                    this.rebuild_worlds();
                })
            };
            let default_font = this.state.config.borrow().settings.font.clone();
            world_dialog::show(button, Some(&edited), &default_font, on_save, Some(on_delete));
        });

        if world.characters.is_empty() {
            let row = adw::ActionRow::builder()
                .title(&world.name)
                .subtitle(&subtitle)
                .use_markup(false)
                .activatable(true)
                .build();
            row.add_suffix(&edit);
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            let weak = Rc::downgrade(self);
            row.connect_activated(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.connect(world.clone(), None);
                }
            });
            return row.upcast();
        }

        let expander = adw::ExpanderRow::builder().title(&world.name).subtitle(&subtitle).use_markup(false).build();
        expander.add_suffix(&edit);
        for character in world.characters.iter().cloned().map(Some).chain([None]) {
            let title = match &character {
                Some(c) => c.name.clone(),
                None => "Connect without a character".into(),
            };
            let row = adw::ActionRow::builder().title(&title).use_markup(false).activatable(true).build();
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
            let weak = Rc::downgrade(self);
            let world = world.clone();
            row.connect_activated(move |_| {
                if let Some(this) = weak.upgrade() {
                    this.connect(world.clone(), character.clone());
                }
            });
            expander.add_row(&row);
        }
        expander.upcast()
    }

    fn build_directory(self: &Rc<Self>) {
        let entries = directory::entries();
        let group = adw::PreferencesGroup::builder()
            .title("Directory")
            .description("Public worlds from the BeipMU world list")
            .build();
        let expander = adw::ExpanderRow::builder()
            .title("Browse public worlds")
            .subtitle(format!("{} worlds", entries.len()))
            .build();
        for entry in entries {
            let subtitle = format!(
                "{}:{}{}{}",
                entry.host,
                entry.port,
                if entry.tls { " · TLS" } else { "" },
                if entry.keywords.is_empty() { String::new() } else { format!(" · {}", entry.keywords.join(", ")) }
            );
            let row = adw::ActionRow::builder()
                .title(&entry.name)
                .subtitle(&subtitle)
                .use_markup(false)
                .activatable(true)
                .build();
            if !entry.description.is_empty() {
                let text: String = entry.description.chars().take(600).collect();
                row.set_tooltip_text(Some(&text));
            }
            let add = gtk::Button::builder()
                .icon_name("list-add-symbolic")
                .tooltip_text("Save to my worlds")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            row.add_suffix(&add);

            let weak = Rc::downgrade(self);
            let world = entry.to_world();
            add.connect_clicked(move |_| {
                let Some(this) = weak.upgrade() else { return };
                this.state.config.borrow_mut().worlds.push(world.clone());
                this.save();
                this.rebuild_worlds();
                this.toasts.add_toast(adw::Toast::new(&format!("Added {}", world.name)));
            });

            let weak = Rc::downgrade(self);
            row.connect_activated(move |_| {
                if let Some(this) = weak.upgrade() {
                    let world = entry.to_world();
                    let guest = world.characters.first().cloned();
                    this.connect(world, guest);
                }
            });
            expander.add_row(&row);
        }
        group.add(&expander);
        self.page.add(&group);
        *self.directory_group.borrow_mut() = Some(group);
    }
}
