//! Colours and glyphs. Server text uses the same standard 16 colours as the GTK client, so red
//! is red whatever the terminal's theme does to it (or the theme's own, if the user asks); the
//! chrome uses a few truecolor accents.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;

use ratatui::style::{Color, Modifier, Style};

pub const ACCENT: Color = Color::Rgb(0xc0, 0x9a, 0xf0);
pub const ACCENT_2: Color = Color::Rgb(0x7d, 0xcf, 0xff);
pub const GOOD: Color = Color::Rgb(0x9e, 0xce, 0x6a);
pub const WARN: Color = Color::Rgb(0xe0, 0xaf, 0x68);
pub const BAD: Color = Color::Rgb(0xf7, 0x76, 0x8e);
pub const MUTED: Color = Color::Rgb(0x8a, 0x8f, 0xa8);
pub const FAINT: Color = Color::Rgb(0x56, 0x5f, 0x89);
pub const SELECTED_BG: Color = Color::Rgb(0x2f, 0x33, 0x4d);

/// Claude Code-style "working" star, for connecting.
pub const SPINNER: [&str; 6] = ["·", "✢", "✳", "✶", "✻", "✽"];

pub fn accent() -> Style {
    Style::new().fg(ACCENT)
}

pub fn muted() -> Style {
    Style::new().fg(MUTED)
}

pub fn faint() -> Style {
    Style::new().fg(FAINT)
}

pub fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

/// The `terminal_colors` setting: leave the 16 basic colours to the terminal's theme.
static TERMINAL_COLORS: AtomicBool = AtomicBool::new(false);
static PALETTE: LazyLock<familiar_ansi::Palette> = LazyLock::new(familiar_ansi::Palette::default);

pub fn set_terminal_colors(on: bool) {
    TERMINAL_COLORS.store(on, Ordering::Relaxed);
}

/// A server colour as the terminal should draw it. `themed` leaves the 16 basic colours to the
/// terminal; otherwise they're sent as exact colours.
fn color_with(c: familiar_ansi::Color, bold: bool, themed: bool) -> Option<Color> {
    match c {
        familiar_ansi::Color::Default => None,
        familiar_ansi::Color::Indexed(n) if n < 16 && !themed => {
            // As in the GTK client: bold text in a basic colour is drawn in the bright one.
            let n = if bold && PALETTE.bold_is_bright && n < 8 { n + 8 } else { n };
            let rgb = PALETTE.ansi[n as usize];
            Some(Color::Rgb(rgb.0, rgb.1, rgb.2))
        }
        familiar_ansi::Color::Indexed(n) => Some(Color::Indexed(n)),
        familiar_ansi::Color::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    }
}

/// A server text style as a ratatui style.
pub fn style(s: &familiar_ansi::Style) -> Style {
    style_with(s, TERMINAL_COLORS.load(Ordering::Relaxed))
}

fn style_with(s: &familiar_ansi::Style, themed: bool) -> Style {
    let mut out = Style::new();
    if let Some(fg) = color_with(s.fg, s.bold, themed) {
        out = out.fg(fg);
    }
    if let Some(bg) = color_with(s.bg, false, themed) {
        out = out.bg(bg);
    }
    let mut m = Modifier::empty();
    for (on, flag) in [
        (s.bold, Modifier::BOLD),
        (s.faint, Modifier::DIM),
        (s.italic, Modifier::ITALIC),
        (s.underline || s.link.is_some(), Modifier::UNDERLINED),
        (s.blink, Modifier::SLOW_BLINK),
        (s.reverse, Modifier::REVERSED),
        (s.conceal, Modifier::HIDDEN),
        (s.strike, Modifier::CROSSED_OUT),
    ] {
        if on {
            m |= flag;
        }
    }
    if s.link.is_some() && s.fg == familiar_ansi::Color::Default {
        out = out.fg(ACCENT_2);
    }
    out.add_modifier(m)
}

/// A Beip.Stats colour.
pub fn stat_color(c: Option<familiar_engine::stats::StatColor>) -> Option<Color> {
    match c? {
        familiar_engine::stats::StatColor::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
        familiar_engine::stats::StatColor::Transparent => None,
    }
}

/// "#rrggbb" as a colour.
pub fn hex(s: &str) -> Option<Color> {
    let v = u32::from_str_radix(s.strip_prefix('#')?, 16).ok()?;
    (s.len() == 7).then_some(Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

#[cfg(test)]
mod tests {
    use super::*;
    use familiar_ansi::Color as C;

    fn fg(n: u8) -> familiar_ansi::Style {
        familiar_ansi::Style { fg: C::Indexed(n), ..Default::default() }
    }

    #[test]
    fn basic_colours_are_exact_unless_the_theme_is_wanted() {
        let red = PALETTE.ansi[1];
        assert_eq!(style_with(&fg(1), false).fg, Some(Color::Rgb(red.0, red.1, red.2)));
        assert_eq!(style_with(&fg(1), true).fg, Some(Color::Indexed(1)));
        // The 256-colour cube is the same everywhere, so it's left to the terminal.
        assert_eq!(style_with(&fg(196), false).fg, Some(Color::Indexed(196)));
        let bg = familiar_ansi::Style { bg: C::Indexed(4), ..Default::default() };
        let blue = PALETTE.ansi[4];
        assert_eq!(style_with(&bg, false).bg, Some(Color::Rgb(blue.0, blue.1, blue.2)));
    }

    #[test]
    fn bold_follows_the_gui_palette() {
        let bold_red = familiar_ansi::Style { bold: true, ..fg(1) };
        let expected = PALETTE.ansi[if PALETTE.bold_is_bright { 9 } else { 1 }];
        assert_eq!(style_with(&bold_red, false).fg, Some(Color::Rgb(expected.0, expected.1, expected.2)));
    }
}
