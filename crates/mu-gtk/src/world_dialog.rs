//! Dialog for adding or editing a world and its characters.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use mu_config::{Character, World};

const ENCODINGS: [&str; 3] = ["UTF-8", "Windows-1252", "CP437"];

struct CharacterRows {
    expander: adw::ExpanderRow,
    name: adw::EntryRow,
    login: adw::PasswordEntryRow,
}

pub struct WorldDialog {
    dialog: adw::Dialog,
    name: adw::EntryRow,
    host: adw::EntryRow,
    port: adw::SpinRow,
    tls: adw::SwitchRow,
    verify: adw::SwitchRow,
    encoding: adw::ComboRow,
    characters_group: adw::PreferencesGroup,
    characters: RefCell<Vec<CharacterRows>>,
    save: gtk::Button,
}

/// Opens the dialog. `on_save` gets the edited world. `on_delete` is only offered when editing an
/// existing world.
pub fn show(
    parent: &impl IsA<gtk::Widget>,
    world: Option<&World>,
    on_save: impl Fn(World) + 'static,
    on_delete: Option<Box<dyn Fn()>>,
) {
    let editing = world.is_some();
    let world = world.cloned().unwrap_or_default();

    let name = adw::EntryRow::builder().title("Name").text(&world.name).build();
    let host = adw::EntryRow::builder().title("Host").text(&world.host).build();
    let port = adw::SpinRow::with_range(1.0, 65535.0, 1.0);
    port.set_title("Port");
    port.set_value(world.port as f64);
    let tls = adw::SwitchRow::builder().title("Use TLS").active(world.tls).build();
    let verify = adw::SwitchRow::builder()
        .title("Verify certificate")
        .subtitle("Turn off for servers with self-signed certificates")
        .active(world.verify_certificate)
        .build();
    tls.bind_property("active", &verify, "sensitive").sync_create().build();
    let encoding = adw::ComboRow::builder()
        .title("Encoding")
        .model(&gtk::StringList::new(&ENCODINGS))
        .selected(
            mu_net::Encoding::from_name(&world.encoding)
                .and_then(|e| ENCODINGS.iter().position(|n| *n == e.name()))
                .unwrap_or(0) as u32,
        )
        .build();

    let server = adw::PreferencesGroup::builder().title("Server").build();
    for row in [name.upcast_ref::<gtk::Widget>(), host.upcast_ref(), port.upcast_ref(), tls.upcast_ref(), verify.upcast_ref(), encoding.upcast_ref()] {
        server.add(row);
    }

    let add_character = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add character")
        .css_classes(["flat"])
        .build();
    let characters_group = adw::PreferencesGroup::builder()
        .title("Characters")
        .description("The login command is sent after connecting, e.g. \"connect name password\". It is stored in plain text.")
        .header_suffix(&add_character)
        .build();

    let page = adw::PreferencesPage::new();
    page.add(&server);
    page.add(&characters_group);

    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::builder().label("Save").css_classes(["suggested-action"]).build();
    let header = adw::HeaderBar::builder().show_start_title_buttons(false).show_end_title_buttons(false).build();
    header.pack_start(&cancel);
    header.pack_end(&save);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&page));

    let dialog = adw::Dialog::builder()
        .title(if editing { "Edit World" } else { "Add World" })
        .content_width(480)
        .content_height(640)
        .child(&toolbar)
        .build();

    let this = Rc::new(WorldDialog {
        dialog,
        name,
        host,
        port,
        tls,
        verify,
        encoding,
        characters_group,
        characters: RefCell::default(),
        save,
    });
    for character in &world.characters {
        this.add_character(character);
    }

    if let Some(on_delete) = on_delete {
        let delete = adw::ButtonRow::builder().title("Delete World").css_classes(["destructive-action"]).build();
        let group = adw::PreferencesGroup::new();
        group.add(&delete);
        page.add(&group);
        let dialog = this.dialog.clone();
        delete.connect_activated(move |_| {
            on_delete();
            dialog.close();
        });
    }

    let weak = Rc::downgrade(&this);
    add_character.connect_clicked(move |_| {
        if let Some(this) = weak.upgrade() {
            this.add_character(&Character::default());
        }
    });

    let dialog = this.dialog.clone();
    cancel.connect_clicked(move |_| {
        dialog.close();
    });

    // A world needs a host. The name falls back to the host if left empty.
    this.save.set_sensitive(!this.host.text().trim().is_empty());
    let weak = Rc::downgrade(&this);
    this.host.connect_changed(move |host| {
        if let Some(this) = weak.upgrade() {
            this.save.set_sensitive(!host.text().trim().is_empty());
        }
    });

    let weak = Rc::downgrade(&this);
    this.save.connect_clicked(move |_| {
        let Some(this) = weak.upgrade() else { return };
        on_save(this.world());
        this.dialog.close();
    });

    // The closures only hold weak references, so keep `this` alive until the dialog closes.
    let keep = RefCell::new(Some(this.clone()));
    this.dialog.connect_closed(move |_| {
        keep.borrow_mut().take();
    });
    this.dialog.present(Some(parent));
    // Without this the header's Cancel button gets focus, so a Space that opened the dialog
    // from the keyboard would close it again on release.
    this.dialog.set_focus(Some(if editing { &this.host } else { &this.name }));
}

impl WorldDialog {
    fn add_character(self: &Rc<Self>, character: &Character) {
        let name = adw::EntryRow::builder().title("Name").text(&character.name).build();
        let login = adw::PasswordEntryRow::builder().title("Login command").text(&character.login).build();
        let remove = adw::ButtonRow::builder().title("Remove Character").css_classes(["destructive-action"]).build();
        let expander = adw::ExpanderRow::builder()
            .title(if character.name.is_empty() { "New character" } else { &character.name })
            .expanded(character.name.is_empty())
            .use_markup(false)
            .build();
        expander.add_row(&name);
        expander.add_row(&login);
        expander.add_row(&remove);
        name.bind_property("text", &expander, "title")
            .transform_to(|_, text: String| Some(if text.is_empty() { "New character".to_string() } else { text }))
            .build();
        self.characters_group.add(&expander);

        let weak = Rc::downgrade(self);
        let row = expander.clone();
        remove.connect_activated(move |_| {
            let Some(this) = weak.upgrade() else { return };
            this.characters_group.remove(&row);
            this.characters.borrow_mut().retain(|c| c.expander != row);
        });
        self.characters.borrow_mut().push(CharacterRows { expander, name, login });
    }

    fn world(&self) -> World {
        let host = self.host.text().trim().to_string();
        let name = self.name.text().trim().to_string();
        World {
            name: if name.is_empty() { host.clone() } else { name },
            host,
            port: self.port.value() as u16,
            tls: self.tls.is_active(),
            verify_certificate: self.verify.is_active(),
            encoding: ENCODINGS[(self.encoding.selected() as usize).min(ENCODINGS.len() - 1)].to_string(),
            characters: self
                .characters
                .borrow()
                .iter()
                .map(|c| Character { name: c.name.text().trim().to_string(), login: c.login.text().to_string() })
                .filter(|c| !c.name.is_empty() || !c.login.is_empty())
                .collect(),
        }
    }
}
