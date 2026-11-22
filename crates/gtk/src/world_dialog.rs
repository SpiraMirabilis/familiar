//! Dialog for adding or editing a world and its characters.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use familiar_config::{Character, Puppet, World};

const ENCODINGS: [&str; 4] = ["Auto", "UTF-8", "Windows-1252", "CP437"];
/// Language ids for the code editor, and their names.
const PROGRAM_LANGUAGES: [(&str, &str); 3] = [("moo", "MOO (Stunt)"), ("muf", "MUF"), ("lua", "Lua")];

struct CharacterRows {
    /// Kept so fields the dialog doesn't edit (like rules) survive a save.
    original: Character,
    expander: adw::ExpanderRow,
    name: adw::EntryRow,
    login: adw::PasswordEntryRow,
    puppets: Rc<RefCell<Vec<PuppetRows>>>,
}

struct PuppetRows {
    expander: adw::ExpanderRow,
    name: adw::EntryRow,
    receive_prefix: adw::EntryRow,
    regex: adw::SwitchRow,
    hide_prefix: adw::SwitchRow,
    send_prefix: adw::EntryRow,
    auto_open: adw::SwitchRow,
    open_on_connect: adw::SwitchRow,
    remove_accidental_prefix: adw::SwitchRow,
}

impl PuppetRows {
    fn puppet(&self) -> Puppet {
        Puppet {
            name: self.name.text().trim().to_string(),
            receive_prefix: self.receive_prefix.text().to_string(),
            regex: self.regex.is_active(),
            hide_prefix: self.hide_prefix.is_active(),
            send_prefix: self.send_prefix.text().to_string(),
            auto_open: self.auto_open.is_active(),
            open_on_connect: self.open_on_connect.is_active(),
            remove_accidental_prefix: self.remove_accidental_prefix.is_active(),
        }
    }
}

/// Adds a puppet's rows under its character.
fn add_puppet(parent: &adw::ExpanderRow, list: &Rc<RefCell<Vec<PuppetRows>>>, p: &Puppet) {
    let entry = |title: &str, text: &str| adw::EntryRow::builder().title(title).text(text).build();
    let switch = |title: &str, subtitle: &str, on: bool| adw::SwitchRow::builder().title(title).subtitle(subtitle).active(on).build();
    let name = entry("Puppet name", &p.name);
    let receive_prefix = entry("Receive prefix (lines starting with this are the puppet's)", &p.receive_prefix);
    let regex = switch("Receive prefix is a regular expression", "With a group, the group is what's hidden", p.regex);
    let hide_prefix = switch("Hide the prefix", "", p.hide_prefix);
    let send_prefix = entry("Send prefix (put before what you type, e.g. \"z1 \")", &p.send_prefix);
    let auto_open = switch("Open its tab when it hears something", "Otherwise its lines stay in the main window", p.auto_open);
    let open_on_connect = switch("Open its tab when the character connects", "", p.open_on_connect);
    let remove_accidental_prefix = switch("Don't double the send prefix", "If you type it anyway", p.remove_accidental_prefix);
    let remove = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Remove puppet")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    let expander = adw::ExpanderRow::builder()
        .title(if p.name.is_empty() { "New puppet" } else { &p.name })
        .subtitle("Puppet")
        .expanded(p.name.is_empty())
        .use_markup(false)
        .build();
    expander.add_suffix(&remove);
    for row in [name.upcast_ref::<gtk::Widget>(), receive_prefix.upcast_ref(), regex.upcast_ref(), hide_prefix.upcast_ref(), send_prefix.upcast_ref(), auto_open.upcast_ref(), open_on_connect.upcast_ref(), remove_accidental_prefix.upcast_ref()] {
        expander.add_row(row);
    }
    name.bind_property("text", &expander, "title")
        .transform_to(|_, text: String| Some(if text.is_empty() { "New puppet".to_string() } else { text }))
        .build();
    parent.add_row(&expander);
    let weak_list = Rc::downgrade(list);
    let (parent_row, row) = (parent.clone(), expander.clone());
    remove.connect_clicked(move |_| {
        parent_row.remove(&row);
        if let Some(list) = weak_list.upgrade() {
            list.borrow_mut().retain(|p| p.expander != row);
        }
    });
    list.borrow_mut().push(PuppetRows { expander, name, receive_prefix, regex, hide_prefix, send_prefix, auto_open, open_on_connect, remove_accidental_prefix });
}

pub struct WorldDialog {
    /// Kept so fields the dialog doesn't edit (like rules) survive a save.
    original: World,
    dialog: adw::Dialog,
    name: adw::EntryRow,
    host: adw::EntryRow,
    port: adw::SpinRow,
    tls: adw::SwitchRow,
    verify: adw::SwitchRow,
    encoding: adw::ComboRow,
    script: adw::EntryRow,
    program_language: adw::ComboRow,
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
        .subtitle("Auto reads UTF-8 and falls back to Windows-1252")
        .model(&gtk::StringList::new(&ENCODINGS))
        .selected(
            familiar_net::Encoding::from_name(&world.encoding)
                .and_then(|e| ENCODINGS.iter().position(|n| *n == e.name()))
                .unwrap_or(0) as u32,
        )
        .build();

    let names: Vec<&str> = PROGRAM_LANGUAGES.iter().map(|(_, n)| *n).collect();
    let program_language = adw::ComboRow::builder()
        .title("Program language")
        .subtitle("For the code editor (MCP simpleedit and /edit)")
        .model(&gtk::StringList::new(&names))
        .selected(PROGRAM_LANGUAGES.iter().position(|(id, _)| *id == world.program_language).unwrap_or(0) as u32)
        .build();
    let script = adw::EntryRow::builder().title("Script file (optional)").text(&world.script).build();
    let browse = gtk::Button::builder()
        .icon_name("document-open-symbolic")
        .tooltip_text("Choose a script file")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    script.add_suffix(&browse);
    {
        let script = script.clone();
        browse.connect_clicked(move |button| {
            let window = button.root().and_downcast::<gtk::Window>();
            let script = script.clone();
            gtk::FileDialog::builder().title("Choose a Script").build().open(
                window.as_ref(),
                None::<&gtk::gio::Cancellable>,
                move |result| {
                    if let Ok(file) = result
                        && let Some(path) = file.path()
                    {
                        script.set_text(&path.to_string_lossy());
                    }
                },
            );
        });
    }

    let server = adw::PreferencesGroup::builder().title("Server").build();
    for row in [name.upcast_ref::<gtk::Widget>(), host.upcast_ref(), port.upcast_ref(), tls.upcast_ref(), verify.upcast_ref(), encoding.upcast_ref(), program_language.upcast_ref(), script.upcast_ref()] {
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
        original: world.clone(),
        dialog,
        name,
        host,
        port,
        tls,
        verify,
        encoding,
        script,
        program_language,
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
        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("Remove character")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        let expander = adw::ExpanderRow::builder()
            .title(if character.name.is_empty() { "New character" } else { &character.name })
            .expanded(character.name.is_empty())
            .use_markup(false)
            .build();
        expander.add_suffix(&remove);
        expander.add_row(&name);
        expander.add_row(&login);
        let add_puppet_row = adw::ButtonRow::builder().title("Add Puppet").start_icon_name("list-add-symbolic").build();
        expander.add_row(&add_puppet_row);
        let puppets: Rc<RefCell<Vec<PuppetRows>>> = Rc::default();
        for p in &character.puppets {
            add_puppet(&expander, &puppets, p);
        }
        {
            let (expander, puppets) = (expander.clone(), puppets.clone());
            add_puppet_row.connect_activated(move |_| add_puppet(&expander, &puppets, &Puppet::default()));
        }
        name.bind_property("text", &expander, "title")
            .transform_to(|_, text: String| Some(if text.is_empty() { "New character".to_string() } else { text }))
            .build();
        self.characters_group.add(&expander);

        let weak = Rc::downgrade(self);
        let row = expander.clone();
        remove.connect_clicked(move |_| {
            let Some(this) = weak.upgrade() else { return };
            this.characters_group.remove(&row);
            this.characters.borrow_mut().retain(|c| c.expander != row);
        });
        self.characters.borrow_mut().push(CharacterRows { original: character.clone(), expander, name, login, puppets });
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
            script: self.script.text().trim().to_string(),
            program_language: PROGRAM_LANGUAGES[(self.program_language.selected() as usize).min(PROGRAM_LANGUAGES.len() - 1)].0.to_string(),
            characters: self
                .characters
                .borrow()
                .iter()
                .map(|c| Character {
                    name: c.name.text().trim().to_string(),
                    login: c.login.text().to_string(),
                    puppets: c.puppets.borrow().iter().map(PuppetRows::puppet).filter(|p| !p.name.is_empty()).collect(),
                    ..c.original.clone()
                })
                .filter(|c| !c.name.is_empty() || !c.login.is_empty())
                .collect(),
            ..self.original.clone()
        }
    }
}
