//! GTK4 MU* client.

// Optional callback slots (`RefCell<Option<Box<dyn Fn(..)>>>`) are the norm in GTK code.
#![allow(clippy::type_complexity)]

mod app;
mod code_editor;
mod connect_dialog;
mod find_bar;
mod font_row;
mod images;
mod import;
mod input;
mod keyboard_page;
mod keys;
mod map_view;
mod media_player;
mod output;
mod palette;
mod preferences_dialog;
mod puppet_view;
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

pub const APP_NAME: &str = familiar_config::APP_NAME;
pub const APP_ID: &str = "io.github.SpiraMirabilis.Familiar";

/// The tokio runtime that runs network sessions. GTK stays on the main thread.
pub fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("familiar-net")
            .enable_all()
            .build()
            .expect("failed to start the tokio runtime")
    })
}

fn main() -> glib::ExitCode {
    glib::set_application_name(APP_NAME);
    for message in familiar_config::migrate_old_dirs() {
        eprintln!("{message}");
    }
    let mut flags = gtk::gio::ApplicationFlags::empty();
    // Test runs must not hand off to (or take over) a copy the user has open.
    if std::env::var_os("FAMILIAR_SEPARATE_INSTANCE").is_some() {
        flags |= gtk::gio::ApplicationFlags::NON_UNIQUE;
    }
    let app = adw::Application::builder().application_id(APP_ID).flags(flags).build();
    let windows: Rc<RefCell<Vec<Rc<window::MainWindow>>>> = Rc::default();
    // familiar [WORLD [CHARACTER]] connects to a saved world on startup.
    let mut args = std::env::args();
    let program = args.next().unwrap_or_else(|| "familiar".into());
    let connect_to: Vec<String> = args.collect();

    app.connect_activate(move |app| {
        if let Some(window) = windows.borrow().first() {
            window.window.present();
            return;
        }
        let state = Rc::new(app::AppState::load());
        let window = window::MainWindow::new(app, state);
        window.window.present();
        if let Some(world) = connect_to.first() {
            window.open_world(world, connect_to.get(1).map(String::as_str));
        }
        windows.borrow_mut().push(window);
    });

    // GTK doesn't need to see our arguments.
    app.run_with_args(&[program])
}
