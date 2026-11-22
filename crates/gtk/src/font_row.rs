//! A preferences row for choosing a font, used for the default font and for a world's own.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::pango;

pub struct FontRow {
    pub row: adw::ActionRow,
    button: gtk::FontDialogButton,
    /// False while a world is using the default font.
    chosen: Rc<Cell<bool>>,
}

impl FontRow {
    /// `font` is what's set now; an empty one means "use `default`", which is then what the
    /// button shows. Pass `optional` for rows where going back to the default makes sense.
    pub fn new(title: &str, font: &str, default: &str, optional: bool) -> Self {
        let chosen = Rc::new(Cell::new(!font.trim().is_empty()));
        let shown = if chosen.get() { font } else { default };
        let dialog = gtk::FontDialog::builder().title(title).modal(true).build();
        let button = gtk::FontDialogButton::builder()
            .dialog(&dialog)
            .font_desc(&pango::FontDescription::from_string(shown))
            .use_font(true)
            .valign(gtk::Align::Center)
            .build();
        let row = adw::ActionRow::builder().title(title).activatable_widget(&button).build();
        if optional {
            let default = default.to_string();
            let subtitle = |chosen: bool| if chosen { "Only for this world" } else { "Using the default font" };
            row.set_subtitle(subtitle(chosen.get()));
            let reset = gtk::Button::builder()
                .icon_name("edit-undo-symbolic")
                .tooltip_text("Use the default font")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .sensitive(chosen.get())
                .build();
            row.add_suffix(&reset);
            {
                let (chosen, row, reset) = (chosen.clone(), row.clone(), reset.clone());
                button.connect_font_desc_notify(move |_| {
                    chosen.set(true);
                    row.set_subtitle(subtitle(true));
                    reset.set_sensitive(true);
                });
            }
            let (chosen, row, button) = (chosen.clone(), row.clone(), button.clone());
            reset.connect_clicked(move |reset| {
                button.set_font_desc(&pango::FontDescription::from_string(&default));
                // Setting the font above counts as choosing one; undo that.
                chosen.set(false);
                row.set_subtitle(subtitle(false));
                reset.set_sensitive(false);
            });
        }
        row.add_suffix(&button);
        Self { row, button, chosen }
    }

    /// The chosen font as a Pango description, or an empty string for the default.
    pub fn font(&self) -> String {
        match self.button.font_desc() {
            Some(desc) if self.chosen.get() => desc.to_string(),
            _ => String::new(),
        }
    }

    pub fn connect_changed(&self, f: impl Fn() + 'static) {
        self.button.connect_font_desc_notify(move |_| f());
    }
}
