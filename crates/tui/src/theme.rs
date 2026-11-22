//! Colours and glyphs. Server text keeps the terminal's own 16-colour palette, so the client
//! looks right in any colour scheme; the chrome uses a few truecolor accents.

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

/// A server colour as the terminal should draw it.
pub fn color(c: familiar_ansi::Color) -> Option<Color> {
    match c {
        familiar_ansi::Color::Default => None,
        familiar_ansi::Color::Indexed(n) => Some(Color::Indexed(n)),
        familiar_ansi::Color::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    }
}

/// A server text style as a ratatui style.
pub fn style(s: &familiar_ansi::Style) -> Style {
    let mut out = Style::new();
    if let Some(fg) = color(s.fg) {
        out = out.fg(fg);
    }
    if let Some(bg) = color(s.bg) {
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
