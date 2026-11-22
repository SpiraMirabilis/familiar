//! The Preferences dialog: the default font and the other settings in the config file's
//! `[settings]` table. Changes apply (and are saved) as they're made.

use std::rc::Rc;

use adw::prelude::*;
use familiar_config::Settings;

use crate::app::AppState;
use crate::font_row::FontRow;

/// Opens the dialog. `on_change` runs after each saved change, so open sessions can reload.
pub fn show(parent: &impl IsA<gtk::Widget>, state: Rc<AppState>, on_change: impl Fn() + 'static) {
    let settings = state.config.borrow().settings.clone();
    let defaults = Settings::default();

    let font = Rc::new(FontRow::new("Font", &settings.font, &defaults.font, false));
    font.row.set_subtitle("For output and input. A world can have its own, in its settings.");

    let switch = |title: &str, subtitle: &str, on: bool| adw::SwitchRow::builder().title(title).subtitle(subtitle).active(on).build();
    let spin = |title: &str, subtitle: &str, min: f64, max: f64, step: f64, value: f64| {
        let row = adw::SpinRow::with_range(min, max, step);
        row.set_title(title);
        row.set_subtitle(subtitle);
        row.set_value(value);
        row
    };
    let local_echo = switch("Show what you send", "Echo commands in the output", settings.local_echo);
    let inline_images = switch(
        "Show linked images",
        "A thumbnail under lines that link to an image. Fetching it tells that site your IP address.",
        settings.inline_images,
    );
    let scrollback = spin("Scrollback lines", "Applies to tabs opened from now on", 100.0, 1_000_000.0, 1000.0, settings.scrollback_lines as f64);
    let prompt_timeout = spin(
        "Prompt delay (ms)",
        "How long an unfinished line waits before it's shown as a prompt",
        0.0,
        5000.0,
        50.0,
        settings.prompt_timeout_ms as f64,
    );
    let spellcheck = switch("Check spelling", "Underline misspellings in the input. Applies to tabs opened from now on.", settings.spellcheck);
    let history = spin("Input history", "Commands remembered per tab. Applies to tabs opened from now on.", 10.0, 100_000.0, 100.0, settings.input_history_size as f64);
    let speak = switch("Speak output", "Read every line that arrives aloud", settings.speak_output);
    let timestamps = switch("Timestamps in logs", "Start each logged line with the time", settings.log_timestamps);

    let appearance = adw::PreferencesGroup::builder().title("Output").build();
    for row in [font.row.upcast_ref::<gtk::Widget>(), local_echo.upcast_ref(), inline_images.upcast_ref(), scrollback.upcast_ref(), prompt_timeout.upcast_ref()] {
        appearance.add(row);
    }
    let input = adw::PreferencesGroup::builder().title("Input").build();
    input.add(&spellcheck);
    input.add(&history);
    let other = adw::PreferencesGroup::builder().title("Speech and Logs").build();
    other.add(&speak);
    other.add(&timestamps);

    let page = adw::PreferencesPage::new();
    page.add(&appearance);
    page.add(&input);
    page.add(&other);
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&page));
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&toasts));
    let dialog = adw::Dialog::builder().title("Preferences").content_width(520).content_height(640).child(&toolbar).build();

    let apply: Rc<dyn Fn()> = {
        let (font, local_echo, inline_images, scrollback, prompt_timeout, spellcheck, history, speak, timestamps) = (
            font.clone(),
            local_echo.clone(),
            inline_images.clone(),
            scrollback.clone(),
            prompt_timeout.clone(),
            spellcheck.clone(),
            history.clone(),
            speak.clone(),
            timestamps.clone(),
        );
        Rc::new(move || {
            let chosen = font.font();
            let new = Settings {
                font: if chosen.is_empty() { Settings::default().font } else { chosen },
                scrollback_lines: scrollback.value() as usize,
                local_echo: local_echo.is_active(),
                input_history_size: history.value() as usize,
                log_timestamps: timestamps.is_active(),
                prompt_timeout_ms: prompt_timeout.value() as u64,
                inline_images: inline_images.is_active(),
                speak_output: speak.is_active(),
                spellcheck: spellcheck.is_active(),
            };
            if state.config.borrow().settings == new {
                return;
            }
            state.config.borrow_mut().settings = new;
            match state.save() {
                Ok(()) => on_change(),
                Err(e) => toasts.add_toast(adw::Toast::new(&e)),
            }
        })
    };
    {
        let apply = apply.clone();
        font.connect_changed(move || apply());
    }
    for row in [&local_echo, &inline_images, &spellcheck, &speak, &timestamps] {
        let apply = apply.clone();
        row.connect_active_notify(move |_| apply());
    }
    for row in [&scrollback, &prompt_timeout, &history] {
        let apply = apply.clone();
        row.connect_value_notify(move |_| apply());
    }
    dialog.present(Some(parent));
}
