//! GTK4 MU* client.

// Optional callback slots (`RefCell<Option<Box<dyn Fn(..)>>>`) are the norm in GTK code.
#![allow(clippy::type_complexity)]

mod app;
mod code_editor;
mod connect_dialog;
mod input;
mod keys;
mod map_view;
mod media_player;
mod output;
mod rules_dialog;
mod session;
mod stats_pane;
mod tilemap_view;
#[cfg(feature = "webview")]
mod webview;
mod window;
mod world_dialog;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use adw::prelude::*;
use gtk::glib;

pub const APP_NAME: &str = "MU Client";
const APP_ID: &str = "io.github.muclient.MuClient";

/// The tokio runtime that runs network sessions. GTK stays on the main thread.
pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("mu-net")
            .enable_all()
            .build()
            .expect("failed to start the tokio runtime")
    })
}

fn main() -> glib::ExitCode {
    glib::set_application_name(APP_NAME);
    let mut flags = gtk::gio::ApplicationFlags::empty();
    // Test runs must not hand off to (or take over) a copy the user has open.
    if std::env::var_os("MU_CLIENT_SEPARATE_INSTANCE").is_some() {
        flags |= gtk::gio::ApplicationFlags::NON_UNIQUE;
    }
    let app = adw::Application::builder().application_id(APP_ID).flags(flags).build();
    let windows: Rc<RefCell<Vec<Rc<window::MainWindow>>>> = Rc::default();

    app.connect_activate(move |app| {
        if let Some(window) = windows.borrow().first() {
            window.window.present();
            return;
        }
        let state = Rc::new(app::AppState::load());
        app::install_css(&mu_ansi::Palette::default(), &state.config.borrow().settings.font);
        let window = window::MainWindow::new(app, state);
        window.window.present();
        windows.borrow_mut().push(window);
    });

    app.run()
}
