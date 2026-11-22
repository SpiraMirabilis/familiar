//! A window for editing text the server sent with MCP simpleedit (descriptions, programs, ...).

use adw::prelude::*;
use mu_engine::EditRequest;

/// Opens an editor for `request`. `on_save` gets the edited text.
pub fn show(parent: Option<&gtk::Window>, request: &EditRequest, on_save: impl Fn(String) + 'static) {
    let view = gtk::TextView::builder()
        .monospace(true)
        .wrap_mode(if request.kind == "moo-code" { gtk::WrapMode::None } else { gtk::WrapMode::WordChar })
        .top_margin(12)
        .bottom_margin(12)
        .left_margin(12)
        .right_margin(12)
        .build();
    view.buffer().set_text(&request.content);
    view.buffer().place_cursor(&view.buffer().start_iter());
    let scrolled = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();

    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::builder().label("Send to Server").css_classes(["suggested-action"]).build();
    let title = adw::WindowTitle::new(&request.name, &format!("{} · {}", request.reference, request.kind));
    let header = adw::HeaderBar::builder().title_widget(&title).show_end_title_buttons(false).show_start_title_buttons(false).build();
    header.pack_start(&cancel);
    header.pack_end(&save);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&scrolled));

    let window = adw::Window::builder()
        .title(format!("Editing {}", request.name))
        .default_width(720)
        .default_height(560)
        .content(&toolbar)
        .build();
    window.set_transient_for(parent);

    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    {
        let window = window.clone();
        save.connect_clicked(move |_| {
            let buffer = view.buffer();
            on_save(buffer.text(&buffer.start_iter(), &buffer.end_iter(), false).to_string());
            window.close();
        });
    }
    window.present();
}
