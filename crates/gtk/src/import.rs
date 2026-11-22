//! Importing BeipMU's Config.txt from the main menu.

use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;

use crate::app::AppState;

/// Asks for a BeipMU Config.txt, imports it and shows what happened. `on_done` runs after the
/// config has changed, so open sessions can pick up new rules.
pub fn choose_and_import(parent: &gtk::Window, state: Rc<AppState>, on_done: impl Fn() + 'static) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("BeipMU configuration (Config.txt)"));
    filter.add_pattern("*.txt");
    filter.add_pattern("*.TXT");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder().title("Import BeipMU Configuration").filters(&filters).default_filter(&filter).build();
    let parent_ref = parent.clone();
    dialog.open(Some(parent), None::<&gio::Cancellable>, move |result| {
        let Ok(file) = result else { return };
        let Some(path) = file.path() else { return };
        let (heading, body, warnings) = import(&path, &state);
        on_done();
        show_result(&parent_ref, &heading, &body, &warnings);
    });
}

/// Reads, converts and merges the file. Returns a heading, a summary and the warnings.
fn import(path: &Path, state: &AppState) -> (String, String, Vec<String>) {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return ("Couldn't Read the File".into(), format!("{}: {e}", path.display()), Vec::new()),
    };
    // BeipMU writes UTF-8; very old configs may be Windows-1252.
    let text = String::from_utf8(bytes).unwrap_or_else(|e| e.into_bytes().iter().map(|&b| b as char).collect());
    let imported = familiar_import::config::import(&text);
    let summary = familiar_import::config::merge(&mut state.config.borrow_mut(), &imported);
    let mut body = summary.describe();
    body.push('.');
    if let Some(script) = &imported.startup_script {
        body.push_str("\n\n");
        body.push_str(&startup_script(script, path));
    }
    if let Err(e) = state.save() {
        return ("Couldn't Save".into(), e, imported.warnings);
    }
    ("BeipMU Configuration Imported".into(), body, imported.warnings)
}

/// Copies BeipMU's startup script to ours when it can be found next to Config.txt (or at its
/// path, if that's a Linux one) and we don't have one yet.
fn startup_script(written: &str, config: &Path) -> String {
    let name = written.rsplit(['\\', '/']).next().unwrap_or(written);
    let candidates = [Some(Path::new(written).to_path_buf()), config.parent().map(|dir| dir.join(name))];
    let found = candidates.into_iter().flatten().find(|p| p.is_file());
    let ours = familiar_config::startup_script_path();
    match found {
        Some(_) if ours.exists() => {
            format!("BeipMU ran {name} at startup. You already have {}, so it was left alone; add BeipMU's code there if you want it.", ours.display())
        }
        Some(source) => {
            let copied = ours.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::copy(&source, &ours));
            match copied {
                Ok(_) => format!("BeipMU's startup script was copied to {}; it runs when a world opens.", ours.display()),
                Err(e) => format!("Couldn't copy the startup script {}: {e}", source.display()),
            }
        }
        None => format!("BeipMU ran a startup script ({written}). Copy it to {} to use it here.", ours.display()),
    }
}

fn show_result(parent: &gtk::Window, heading: &str, body: &str, warnings: &[String]) {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.add_response("close", "Close");
    if !warnings.is_empty() {
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
        for w in warnings {
            let label = gtk::Label::builder().label(w).wrap(true).xalign(0.0).selectable(true).margin_top(6).margin_bottom(6).margin_start(8).margin_end(8).build();
            list.append(&label);
        }
        let title = gtk::Label::builder()
            .label(format!("Not carried over or changed ({})", warnings.len()))
            .xalign(0.0)
            .css_classes(["heading"])
            .margin_bottom(6)
            .build();
        let scrolled = gtk::ScrolledWindow::builder().child(&list).min_content_height(220).max_content_height(320).propagate_natural_height(true).build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&title);
        content.append(&scrolled);
        dialog.set_extra_child(Some(&content));
    }
    dialog.present(Some(parent));
}
