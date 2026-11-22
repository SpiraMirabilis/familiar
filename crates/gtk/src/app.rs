//! Shared application state and styling.

use std::cell::RefCell;
use std::path::PathBuf;

use gtk::{gdk, pango};
use familiar_ansi::Palette;
use familiar_config::Config;

pub struct AppState {
    pub config: RefCell<Config>,
    /// One speaker for the whole app, so sessions queue their speech rather than talk over
    /// each other.
    pub speaker: familiar_speech::Speaker,
    path: PathBuf,
    /// Set if the config file exists but couldn't be read. We then refuse to save, so we don't
    /// overwrite the user's file with defaults.
    load_error: Option<String>,
}

impl AppState {
    pub fn load() -> Self {
        let path = familiar_config::default_config_path();
        match Config::load(&path) {
            Ok(config) => Self { config: RefCell::new(config), speaker: familiar_speech::Speaker::new(crate::APP_NAME), path, load_error: None },
            Err(e) => Self {
                config: RefCell::default(),
                speaker: familiar_speech::Speaker::new(crate::APP_NAME),
                path,
                load_error: Some(e.to_string()),
            },
        }
    }

    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    pub fn save(&self) -> Result<(), String> {
        if let Some(e) = &self.load_error {
            return Err(format!("Not saving: the config file couldn't be read ({e})"));
        }
        self.config.borrow().save(&self.path).map_err(|e| e.to_string())
    }
}

/// CSS for the output and input views, from the palette and the configured font.
pub fn css(palette: &Palette, font: &str) -> String {
    let desc = pango::FontDescription::from_string(font);
    let family = desc.family().map(|f| f.to_string()).unwrap_or_else(|| "monospace".into());
    let size = if desc.size() > 0 { desc.size() as f64 / pango::SCALE as f64 } else { 11.0 };
    let unit = if desc.is_size_absolute() { "px" } else { "pt" };
    format!(
        "textview.mu-output, textview.mu-output > text {{ background-color: {bg}; color: {fg}; }}\n\
         textview.mu-output, textview.mu-input {{ font-family: \"{family}\", monospace; font-size: {size}{unit}; }}\n",
        bg = palette.background.to_hex(),
        fg = palette.foreground.to_hex(),
    )
}

pub fn install_css(palette: &Palette, font: &str) {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&css(palette, font));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}
