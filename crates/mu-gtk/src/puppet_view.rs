//! A puppet's tab: its own output and input, sharing the character's connection.

use std::rc::Rc;

use gtk::prelude::*;
use mu_ansi::Palette;

use crate::input::InputView;
use crate::output::OutputView;

pub struct PuppetView {
    pub root: gtk::Box,
    pub output: Rc<OutputView>,
    pub input: Rc<InputView>,
}

impl PuppetView {
    /// `on_submit` gets what the user typed; `on_link` gets clicked links.
    pub fn new(
        settings: &mu_config::Settings,
        on_submit: impl Fn(String) + 'static,
        on_link: impl Fn(&str) + 'static,
    ) -> Rc<Self> {
        let output = OutputView::new(Palette::default(), settings.scrollback_lines);
        let input = InputView::new(settings.input_history_size, settings.spellcheck);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&output.scrolled);
        root.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        root.append(&input.root);

        input.connect_submit(move |text, _secret| on_submit(text));
        let pager = Rc::downgrade(&output);
        input.connect_page(move |up| {
            if let Some(output) = pager.upgrade() {
                output.scroll_page(up);
            }
        });
        output.connect_link(on_link);
        Rc::new(Self { root, output, input })
    }
}
