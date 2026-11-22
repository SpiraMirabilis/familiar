//! Turns [`Style`] colours into concrete RGB values for rendering.

use crate::{Color, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Moves the colour `amount` (0.0-1.0) of the way toward `other`.
    pub fn blend(self, other: Rgb, amount: f32) -> Rgb {
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
        Rgb(mix(self.0, other.0), mix(self.1, other.1), mix(self.2, other.2))
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    pub ansi: [Rgb; 16],
    pub foreground: Rgb,
    pub background: Rgb,
    /// Bold text with a standard colour (0-7) is shown in the bright version (8-15).
    pub bold_is_bright: bool,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            ansi: [
                Rgb(0, 0, 0),
                Rgb(205, 49, 49),
                Rgb(13, 188, 121),
                Rgb(229, 229, 16),
                Rgb(36, 114, 200),
                Rgb(188, 63, 188),
                Rgb(17, 168, 205),
                Rgb(204, 204, 204),
                Rgb(102, 102, 102),
                Rgb(241, 76, 76),
                Rgb(35, 209, 139),
                Rgb(245, 245, 67),
                Rgb(59, 142, 234),
                Rgb(214, 112, 214),
                Rgb(41, 184, 219),
                Rgb(242, 242, 242),
            ],
            foreground: Rgb(204, 204, 204),
            background: Rgb(16, 16, 16),
            bold_is_bright: true,
        }
    }
}

impl Palette {
    pub fn indexed(&self, n: u8) -> Rgb {
        match n {
            0..=15 => self.ansi[n as usize],
            16..=231 => {
                let n = n - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                Rgb(level(n / 36), level((n / 6) % 6), level(n % 6))
            }
            232..=255 => {
                let v = 8 + (n - 232) * 10;
                Rgb(v, v, v)
            }
        }
    }

    fn color(&self, color: Color, default: Rgb, bold: bool) -> Rgb {
        match color {
            Color::Default => default,
            Color::Indexed(n) if bold && self.bold_is_bright && n < 8 => self.ansi[n as usize + 8],
            Color::Indexed(n) => self.indexed(n),
            Color::Rgb(r, g, b) => Rgb(r, g, b),
        }
    }

    /// Returns the (foreground, background) to draw a span with. This applies bold-is-bright,
    /// faint, reverse and conceal.
    pub fn resolve(&self, style: &Style) -> (Rgb, Rgb) {
        let mut fg = self.color(style.fg, self.foreground, style.bold);
        let mut bg = self.color(style.bg, self.background, false);
        if style.reverse {
            std::mem::swap(&mut fg, &mut bg);
        }
        if style.faint {
            fg = fg.blend(bg, 0.4);
        }
        if style.conceal {
            fg = bg;
        }
        (fg, bg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_and_greys() {
        let p = Palette::default();
        assert_eq!(p.indexed(16), Rgb(0, 0, 0));
        assert_eq!(p.indexed(196), Rgb(255, 0, 0));
        assert_eq!(p.indexed(231), Rgb(255, 255, 255));
        assert_eq!(p.indexed(232), Rgb(8, 8, 8));
        assert_eq!(p.indexed(255), Rgb(238, 238, 238));
    }

    #[test]
    fn bold_bright_and_reverse() {
        let p = Palette::default();
        let style = Style { fg: Color::Indexed(1), bold: true, reverse: true, ..Style::default() };
        let (fg, bg) = p.resolve(&style);
        assert_eq!(fg, p.background);
        assert_eq!(bg, p.ansi[9]);
    }
}
