//! Shared application state and styling.

use std::cell::{Ref, RefCell};
use std::path::PathBuf;

use gtk::{gdk, pango};
use familiar_ansi::Palette;
use familiar_config::Config;
use familiar_engine::keymap::{Keymap, Platform};

pub struct AppState {
    pub config: RefCell<Config>,
    /// One speaker for the whole app, so sessions queue their speech rather than talk over
    /// each other.
    pub speaker: familiar_speech::Speaker,
    path: PathBuf,
    /// The app's stylesheet, rebuilt whenever a font changes.
    css: gtk::CssProvider,
    /// Set if the config file exists but couldn't be read. We then refuse to save, so we don't
    /// overwrite the user's file with defaults.
    load_error: Option<String>,
    /// The keyboard shortcuts, from `[settings.keys]`. Rebuilt when a save changes them.
    keymap: RefCell<Keymap>,
    /// Called after the shortcuts change, so menus and tooltips can show the new keys. Each
    /// returns false once what it updates has gone.
    on_keys_changed: RefCell<Vec<Box<dyn Fn() -> bool>>>,
    /// LLM agents beside worlds (/agent): the MCP server, and the worlds it answers for.
    #[cfg(feature = "agents")]
    pub agents: std::rc::Rc<crate::session::AgentHost>,
}

impl AppState {
    pub fn load() -> Self {
        let path = familiar_config::default_config_path();
        let (config, load_error) = match Config::load(&path) {
            Ok(config) => (config, None),
            Err(e) => (Config::default(), Some(e.to_string())),
        };
        let css = gtk::CssProvider::new();
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        let keymap = Keymap::new(&config.settings, Platform::Linux);
        let state = Self {
            keymap: RefCell::new(keymap),
            on_keys_changed: RefCell::default(),
            config: RefCell::new(config),
            speaker: familiar_speech::Speaker::new(crate::APP_NAME),
            path,
            css,
            load_error,
            #[cfg(feature = "agents")]
            agents: crate::session::AgentHost::new(),
        };
        state.apply_css();
        state
    }

    /// Restyles every output and input view from the fonts in the config.
    pub fn apply_css(&self) {
        self.css.load_from_string(&css(&Palette::default(), &self.config.borrow()));
    }

    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    pub fn keymap(&self) -> Ref<'_, Keymap> {
        self.keymap.borrow()
    }

    /// The first key for an action, as GTK shows it ("Ctrl+O"), for a tooltip.
    pub fn key_label(&self, id: &str) -> Option<String> {
        self.keymap().keys(id).first().map(crate::keys::label)
    }

    /// `f` runs whenever the shortcuts change, until it returns false.
    pub fn connect_keys_changed(&self, f: impl Fn() -> bool + 'static) {
        self.on_keys_changed.borrow_mut().push(Box::new(f));
    }

    /// Rebuilds the keymap if the config's shortcuts changed, and tells whoever shows them.
    fn update_keymap(&self) {
        let keymap = Keymap::new(&self.config.borrow().settings, Platform::Linux);
        let same = {
            let old = self.keymap.borrow();
            familiar_engine::keymap::actions(Platform::Linux).all(|a| old.keys(a.id) == keymap.keys(a.id))
        };
        if same {
            return;
        }
        *self.keymap.borrow_mut() = keymap;
        // Taken out while they run, in case one adds another.
        let mut listeners = std::mem::take(&mut *self.on_keys_changed.borrow_mut());
        listeners.retain(|f| f());
        let mut slot = self.on_keys_changed.borrow_mut();
        listeners.append(&mut slot);
        *slot = listeners;
    }

    /// Saves the config. Fonts or shortcuts may be what changed, so this restyles and
    /// rebuilds the keymap too.
    pub fn save(&self) -> Result<(), String> {
        self.apply_css();
        self.update_keymap();
        if let Some(e) = &self.load_error {
            return Err(format!("Not saving: the config file couldn't be read ({e})"));
        }
        self.config.borrow().save(&self.path).map_err(|e| e.to_string())
    }
}

/// CSS for the output and input views: the palette, the default font, and a rule for each
/// world that has a font of its own (see [`world_class`]).
pub fn css(palette: &Palette, config: &Config) -> String {
    let mut css = format!(
        "textview.mu-output, textview.mu-output > text {{ background-color: {bg}; color: {fg}; }}\n\
         textview.mu-output, textview.mu-input {{ {font} }}\n",
        bg = palette.background.to_hex(),
        fg = palette.foreground.to_hex(),
        font = font_css(&config.settings.font),
    );
    // After the default, so they win.
    for world in config.worlds.iter().filter(|w| !w.font.trim().is_empty()) {
        css.push_str(&format!("textview.{} {{ {} }}\n", world_class(&world.name), font_css(&world.font)));
    }
    css
}

/// A Pango font description ("Fira Mono Bold 12") as CSS declarations.
fn font_css(font: &str) -> String {
    let desc = pango::FontDescription::from_string(font);
    let family = desc.family().map(|f| f.replace(['"', '\\'], "")).unwrap_or_else(|| "monospace".into());
    let size = if desc.size() > 0 { desc.size() as f64 / pango::SCALE as f64 } else { 11.0 };
    let unit = if desc.is_size_absolute() { "px" } else { "pt" };
    let style = match desc.style() {
        pango::Style::Italic => "italic",
        pango::Style::Oblique => "oblique",
        _ => "normal",
    };
    let weight = match desc.weight() {
        pango::Weight::Thin => 100,
        pango::Weight::Ultralight => 200,
        pango::Weight::Light | pango::Weight::Semilight => 300,
        pango::Weight::Medium => 500,
        pango::Weight::Semibold => 600,
        pango::Weight::Bold => 700,
        pango::Weight::Ultrabold => 800,
        pango::Weight::Heavy | pango::Weight::Ultraheavy => 900,
        _ => 400,
    };
    format!("font-family: \"{family}\", monospace; font-size: {size}{unit}; font-style: {style}; font-weight: {weight};")
}

/// The CSS class that carries a world's own font. Views belonging to that world's tabs get it
/// whether or not the world has a font, so setting one later reaches tabs that are already open.
pub fn world_class(world: &str) -> String {
    // FNV-1a: world names can hold anything, class names can't.
    let hash = world.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    format!("mu-world-{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use familiar_config::World;

    #[test]
    fn worlds_with_fonts_get_their_own_rule() {
        let mut config = Config::default();
        config.settings.font = "Monospace 11".into();
        config.worlds.push(World { name: "Plain".into(), ..World::default() });
        config.worlds.push(World { name: "Fancy \"World\"".into(), font: "Fira Mono Bold Italic 14".into(), ..World::default() });
        let css = css(&Palette::default(), &config);
        assert!(css.contains("font-family: \"Monospace\", monospace; font-size: 11pt;"), "{css}");
        let rule = css.lines().find(|l| l.contains(&world_class("Fancy \"World\""))).expect("a rule for the world");
        assert!(rule.contains("\"Fira Mono\"") && rule.contains("14pt") && rule.contains("italic") && rule.contains("700"), "{rule}");
        assert!(!css.contains(&world_class("Plain")));
        // The world's rule comes after the default so it takes precedence.
        assert!(css.find("mu-world-").unwrap() > css.find("mu-input").unwrap());
    }
}
