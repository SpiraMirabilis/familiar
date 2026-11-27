//! ANSI/ECMA-48 parsing into styled lines.
//!
//! [`AnsiParser`] turns one decoded line at a time into a [`StyledLine`]. SGR state (and an open
//! OSC 8 hyperlink) carries over between lines, as it does in a terminal. Every CSI, OSC and
//! other escape sequence is fully consumed, so unsupported sequences never leak into the text.

mod linkify;
mod palette;

pub use linkify::find_urls;
pub use palette::{Palette, Rgb};

use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Default,
    /// 0-7 standard, 8-15 bright, 16-255 the xterm cube and greyscale ramp.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub faint: bool,
    pub italic: bool,
    pub underline: bool,
    pub blink: bool,
    pub reverse: bool,
    pub conceal: bool,
    pub strike: bool,
    /// Target of an OSC 8 hyperlink.
    pub link: Option<Arc<str>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// Byte range into [`StyledLine::text`].
    pub start: usize,
    pub end: usize,
    pub style: Style,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StyledLine {
    pub text: String,
    /// Covers the whole of `text` in order, with no gaps. Adjacent spans always differ in style.
    pub spans: Vec<Span>,
    /// The line contained a BEL character.
    pub bell: bool,
}

impl StyledLine {
    pub fn plain(text: impl Into<String>) -> Self {
        let text = text.into();
        let spans = if text.is_empty() {
            Vec::new()
        } else {
            vec![Span { start: 0, end: text.len(), style: Style::default() }]
        };
        Self { text, spans, bell: false }
    }

    pub fn span_text(&self, span: &Span) -> &str {
        &self.text[span.start..span.end]
    }

    /// Applies `f` to the style of the text in the byte range `start..end`, splitting spans as
    /// needed. The range must fall on character boundaries.
    pub fn restyle(&mut self, start: usize, end: usize, f: impl Fn(&mut Style)) {
        let (start, end) = (start.min(self.text.len()), end.min(self.text.len()));
        if start >= end {
            return;
        }
        assert!(self.text.is_char_boundary(start) && self.text.is_char_boundary(end));
        let mut spans: Vec<Span> = Vec::with_capacity(self.spans.len() + 2);
        let mut push = |span: Span| match spans.last_mut() {
            Some(last) if last.style == span.style && last.end == span.start => last.end = span.end,
            _ => spans.push(span),
        };
        for span in std::mem::take(&mut self.spans) {
            let (s, e) = (start.max(span.start), end.min(span.end));
            if s >= e {
                push(span);
                continue;
            }
            if span.start < s {
                push(Span { start: span.start, end: s, style: span.style.clone() });
            }
            let mut style = span.style.clone();
            f(&mut style);
            push(Span { start: s, end: e, style });
            if e < span.end {
                push(Span { start: e, end: span.end, style: span.style });
            }
        }
        self.spans = spans;
    }

    /// Deletes the text in the byte range `start..end` (on character boundaries), keeping the
    /// styles of what remains.
    pub fn remove_range(&mut self, start: usize, end: usize) {
        let (start, end) = (start.min(self.text.len()), end.min(self.text.len()));
        if start >= end {
            return;
        }
        let removed = end - start;
        self.text.replace_range(start..end, "");
        let mut spans: Vec<Span> = Vec::with_capacity(self.spans.len());
        for mut span in std::mem::take(&mut self.spans) {
            let cut = |at: usize| if at <= start { at } else if at >= end { at - removed } else { start };
            span.start = cut(span.start);
            span.end = cut(span.end);
            if span.start >= span.end {
                continue;
            }
            match spans.last_mut() {
                Some(last) if last.style == span.style && last.end == span.start => last.end = span.end,
                _ => spans.push(span),
            }
        }
        self.spans = spans;
    }

    /// Replaces the byte range `start..end` (on character boundaries) with `with`, which takes
    /// the style of the text it replaces.
    pub fn replace_range(&mut self, start: usize, end: usize, with: &str) {
        let (start, end) = (start.min(self.text.len()), end.min(self.text.len()));
        if start > end {
            return;
        }
        let style = self
            .spans
            .iter()
            .find(|s| s.start <= start && start < s.end)
            .or_else(|| self.spans.last())
            .map(|s| s.style.clone())
            .unwrap_or_default();
        self.remove_range(start, end);
        if with.is_empty() {
            return;
        }
        self.text.insert_str(start, with);
        let n = with.len();
        let new = Span { start, end: start + n, style };
        let mut spans: Vec<Span> = Vec::with_capacity(self.spans.len() + 2);
        let mut push = |span: Span| match spans.last_mut() {
            Some(last) if last.style == span.style && last.end == span.start => last.end = span.end,
            _ => spans.push(span),
        };
        let mut inserted = false;
        for span in std::mem::take(&mut self.spans) {
            if span.end <= start {
                push(span);
            } else if span.start >= start {
                if !inserted {
                    push(new.clone());
                    inserted = true;
                }
                push(Span { start: span.start + n, end: span.end + n, style: span.style });
            } else {
                push(Span { start: span.start, end: start, style: span.style.clone() });
                push(new.clone());
                inserted = true;
                push(Span { start: start + n, end: span.end + n, style: span.style });
            }
        }
        if !inserted {
            push(new);
        }
        self.spans = spans;
    }

    fn push(&mut self, c: char, style: &Style) {
        let start = self.text.len();
        self.text.push(c);
        let end = self.text.len();
        match self.spans.last_mut() {
            Some(last) if last.style == *style => last.end = end,
            _ => self.spans.push(Span { start, end, style: style.clone() }),
        }
    }
}

const MAX_CSI_LEN: usize = 1024;
const MAX_OSC_LEN: usize = 8192;

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    /// Inside `ESC [`. Holds the parameter bytes collected so far and whether the sequence is
    /// private (starts with `<`, `=`, `>` or `?`) or has intermediate bytes, so isn't SGR.
    Csi { params: String, private: bool },
    /// `ESC` followed by intermediate bytes, e.g. `ESC ( B`.
    EscIntermediate,
    /// Inside an OSC string.
    Osc { data: String, overflow: bool },
    /// Saw `ESC` inside an OSC, expecting `\` to finish it.
    OscEsc { data: String, overflow: bool },
    /// Inside a DCS/SOS/PM/APC string. Ignored until ST.
    IgnoreString,
    IgnoreStringEsc,
}

#[derive(Debug, Clone)]
pub struct AnsiParser {
    state: State,
    style: Style,
    /// Clear the style at the start of each line, for servers that never send a reset.
    pub reset_each_line: bool,
}

impl Default for AnsiParser {
    fn default() -> Self {
        Self::new()
    }
}

impl AnsiParser {
    pub fn new() -> Self {
        Self { state: State::Ground, style: Style::default(), reset_each_line: false }
    }

    pub fn style(&self) -> &Style {
        &self.style
    }

    pub fn reset(&mut self) {
        self.state = State::Ground;
        self.style = Style::default();
    }

    pub fn parse_line(&mut self, input: &str) -> StyledLine {
        if self.reset_each_line {
            self.style = Style::default();
        }
        let mut line = StyledLine::default();
        for c in input.chars() {
            self.step(c, &mut line);
        }
        // An escape sequence can't continue past a line break. Drop anything left over.
        self.state = State::Ground;
        line
    }

    fn step(&mut self, c: char, line: &mut StyledLine) {
        let state = std::mem::replace(&mut self.state, State::Ground);
        self.state = match state {
            State::Ground => match c {
                '\x1b' => State::Esc,
                '\x07' => {
                    line.bell = true;
                    State::Ground
                }
                '\t' => {
                    line.push('\t', &self.style);
                    State::Ground
                }
                // C1 CSI (U+009B), OSC (U+009D), and DCS/SOS/PM/APC are rare but legal.
                '\u{9b}' => State::Csi { params: String::new(), private: false },
                '\u{9d}' => State::Osc { data: String::new(), overflow: false },
                '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => State::IgnoreString,
                c if c.is_control() => State::Ground,
                c => {
                    line.push(c, &self.style);
                    State::Ground
                }
            },
            State::Esc => match c {
                '[' => State::Csi { params: String::new(), private: false },
                ']' => State::Osc { data: String::new(), overflow: false },
                'P' | 'X' | '^' | '_' => State::IgnoreString,
                '\x1b' => State::Esc,
                ' '..='/' => State::EscIntermediate,
                _ => State::Ground, // Two-character sequence like ESC 7 or ESC c.
            },
            State::EscIntermediate => match c {
                ' '..='/' => State::EscIntermediate,
                _ => State::Ground,
            },
            State::Csi { mut params, mut private } => match c {
                '0'..='9' | ';' | ':' => {
                    if params.len() < MAX_CSI_LEN {
                        params.push(c);
                    } else {
                        private = true;
                    }
                    State::Csi { params, private }
                }
                '<' | '=' | '>' | '?' | ' '..='/' => {
                    private = true;
                    State::Csi { params, private }
                }
                '@'..='~' => {
                    if c == 'm' && !private {
                        apply_sgr(&mut self.style, &params);
                    }
                    State::Ground
                }
                '\x1b' => State::Esc,
                '\u{9b}' => State::Csi { params: String::new(), private: false },
                '\u{9d}' => State::Osc { data: String::new(), overflow: false },
                // Anything else is malformed. Abandon the sequence.
                _ => State::Ground,
            },
            State::Osc { mut data, mut overflow } => match c {
                '\x07' | '\u{9c}' => {
                    if !overflow {
                        self.apply_osc(&data);
                    }
                    State::Ground
                }
                '\x1b' => State::OscEsc { data, overflow },
                _ => {
                    // Cap the length so a missing terminator can't eat memory.
                    if !overflow && data.len() + c.len_utf8() <= MAX_OSC_LEN {
                        data.push(c);
                    } else {
                        overflow = true;
                    }
                    State::Osc { data, overflow }
                }
            },
            State::OscEsc { data, overflow } => {
                if c == '\\' {
                    if !overflow {
                        self.apply_osc(&data);
                    }
                    State::Ground
                } else {
                    // Unterminated OSC interrupted by a new escape. Treat it as that escape.
                    self.state = State::Esc;
                    return self.step(c, line);
                }
            }
            State::IgnoreString => match c {
                '\x1b' => State::IgnoreStringEsc,
                '\x07' | '\u{9c}' => State::Ground,
                _ => State::IgnoreString,
            },
            State::IgnoreStringEsc => match c {
                '\\' => State::Ground,
                '\x1b' => State::IgnoreStringEsc,
                _ => State::IgnoreString,
            },
        };
    }

    fn apply_osc(&mut self, s: &str) {
        // OSC 8 ; params ; URI. An empty URI closes the link.
        if let Some(rest) = s.strip_prefix("8;") {
            let uri = rest.split_once(';').map_or("", |(_, uri)| uri);
            if uri.is_empty() {
                self.style.link = None;
            } else if !uri.chars().any(char::is_control) {
                self.style.link = Some(Arc::from(uri));
            }
        }
    }
}

/// Parses SGR parameters, which are `;`-separated groups whose items may be `:`-separated
/// sub-parameters. Empty values count as 0.
fn parse_params(params: &str) -> Vec<Vec<u32>> {
    if params.is_empty() {
        return vec![vec![0]];
    }
    params
        .split(';')
        .map(|group| {
            group
                .split(':')
                .map(|n| if n.is_empty() { 0 } else { n.parse::<u32>().unwrap_or(u32::MAX).min(65535) })
                .collect()
        })
        .collect()
}

fn apply_sgr(style: &mut Style, params: &str) {
    let groups = parse_params(params);
    let mut i = 0;
    while i < groups.len() {
        let group = &groups[i];
        let code = group[0];
        match code {
            0 => {
                // Reset keeps the hyperlink. Links are only closed by OSC 8.
                let link = style.link.take();
                *style = Style { link, ..Style::default() };
            }
            1 => style.bold = true,
            2 => style.faint = true,
            3 => style.italic = true,
            4 => style.underline = group.get(1).is_none_or(|&s| s != 0),
            5 | 6 => style.blink = true,
            7 => style.reverse = true,
            8 => style.conceal = true,
            9 => style.strike = true,
            21 => style.underline = true,
            22 => {
                style.bold = false;
                style.faint = false;
            }
            23 => style.italic = false,
            24 => style.underline = false,
            25 => style.blink = false,
            27 => style.reverse = false,
            28 => style.conceal = false,
            29 => style.strike = false,
            30..=37 => style.fg = Color::Indexed((code - 30) as u8),
            39 => style.fg = Color::Default,
            40..=47 => style.bg = Color::Indexed((code - 40) as u8),
            49 => style.bg = Color::Default,
            90..=97 => style.fg = Color::Indexed((code - 90 + 8) as u8),
            100..=107 => style.bg = Color::Indexed((code - 100 + 8) as u8),
            38 | 48 | 58 => {
                let (color, used) = if group.len() > 1 {
                    (extended_color(&group[1..]), 0)
                } else {
                    let tail = &groups[i + 1..];
                    let mut rest = [0u32; 4];
                    let n = tail.len().min(4);
                    for (slot, g) in rest[..n].iter_mut().zip(&tail[..n]) {
                        *slot = g[0];
                    }
                    let used = match rest[..n].first() {
                        Some(5) => 2,
                        Some(2) => 4,
                        _ => tail.len(),
                    };
                    (extended_color_semicolon(&rest[..n]), used.min(tail.len()))
                };
                i += used;
                if let Some(color) = color {
                    match code {
                        38 => style.fg = color,
                        48 => style.bg = color,
                        _ => {} // Underline colour isn't supported.
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
}

fn byte(n: u32) -> Option<u8> {
    u8::try_from(n).ok()
}

/// Colon form: `38:5:n` or `38:2:[colorspace]:r:g:b`.
fn extended_color(sub: &[u32]) -> Option<Color> {
    match sub {
        [5, n, ..] => Some(Color::Indexed(byte(*n)?)),
        [2, _cs, r, g, b, ..] => Some(Color::Rgb(byte(*r)?, byte(*g)?, byte(*b)?)),
        [2, r, g, b] => Some(Color::Rgb(byte(*r)?, byte(*g)?, byte(*b)?)),
        _ => None,
    }
}

/// Semicolon form: `38;5;n` or `38;2;r;g;b`.
fn extended_color_semicolon(rest: &[u32]) -> Option<Color> {
    match rest {
        [5, n, ..] => Some(Color::Indexed(byte(*n)?)),
        [2, r, g, b, ..] => Some(Color::Rgb(byte(*r)?, byte(*g)?, byte(*b)?)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> StyledLine {
        AnsiParser::new().parse_line(s)
    }

    fn spans(line: &StyledLine) -> Vec<(&str, Style)> {
        line.spans.iter().map(|s| (line.span_text(s), s.style.clone())).collect()
    }

    #[test]
    fn restyle_splits_and_merges() {
        let mut l = parse("ab\x1b[31mcdef\x1b[0mgh");
        l.restyle(1, 4, |s| s.bold = true);
        let parts: Vec<_> = l.spans.iter().map(|s| (l.span_text(s), s.style.fg, s.style.bold)).collect();
        assert_eq!(
            parts,
            vec![
                ("a", Color::Default, false),
                ("b", Color::Default, true),
                ("cd", Color::Indexed(1), true),
                ("ef", Color::Indexed(1), false),
                ("gh", Color::Default, false),
            ]
        );
        // Restyling everything back merges the spans again.
        l.restyle(0, 8, |s| *s = Style::default());
        assert_eq!(l.spans.len(), 1);
        l.restyle(5, 99, |s| s.italic = true);
        assert_eq!(l.spans.last().map(|s| l.span_text(s)), Some("fgh"));
    }

    #[test]
    fn remove_range_keeps_styles() {
        let mut l = parse("\x1b[32mZed> \x1b[0mhello \x1b[1mthere");
        l.remove_range(0, 5);
        assert_eq!(l.text, "hello there");
        assert_eq!(l.spans.len(), 2);
        assert!(l.spans[1].style.bold);
        let mut l = parse("ab\x1b[31mcd\x1b[0mef");
        l.remove_range(1, 5);
        assert_eq!(l.text, "af");
        assert_eq!(l.spans.len(), 1, "the plain pieces on each side merge");
    }

    #[test]
    fn replace_range_takes_the_replaced_style() {
        let mut l = parse("a \x1b[31mnaughty\x1b[0m word");
        l.replace_range(2, 9, "****");
        assert_eq!(l.text, "a **** word");
        let texts: Vec<&str> = l.spans.iter().map(|s| l.span_text(s)).collect();
        assert_eq!(texts, ["a ", "****", " word"]);
        let mut l = parse("ab");
        l.replace_range(2, 2, "c");
        assert_eq!((l.text.as_str(), l.spans.len()), ("abc", 1), "appending merges");
        l.replace_range(0, 3, "");
        assert!(l.text.is_empty() && l.spans.is_empty());
    }

    #[test]
    fn plain_text() {
        let l = parse("hello");
        assert_eq!(l, StyledLine::plain("hello"));
    }

    #[test]
    fn basic_colors_and_reset() {
        let l = parse("\x1b[1;31mred\x1b[0m plain\x1b[mx");
        let red = Style { fg: Color::Indexed(1), bold: true, ..Style::default() };
        assert_eq!(spans(&l), vec![("red", red), (" plainx", Style::default())]);
    }

    #[test]
    fn bright_and_background() {
        let l = parse("\x1b[92;104mx");
        assert_eq!(l.spans[0].style.fg, Color::Indexed(10));
        assert_eq!(l.spans[0].style.bg, Color::Indexed(12));
    }

    #[test]
    fn extended_colors_both_syntaxes() {
        let l = parse("\x1b[38;5;208ma\x1b[38;2;1;2;3mb\x1b[38:2::4:5:6mc\x1b[48:5:17md");
        let st: Vec<_> = l.spans.iter().map(|s| (s.style.fg, s.style.bg)).collect();
        assert_eq!(
            st,
            vec![
                (Color::Indexed(208), Color::Default),
                (Color::Rgb(1, 2, 3), Color::Default),
                (Color::Rgb(4, 5, 6), Color::Default),
                (Color::Rgb(4, 5, 6), Color::Indexed(17)),
            ]
        );
    }

    #[test]
    fn params_after_extended_color_still_apply() {
        let l = parse("\x1b[38;5;1;1mx");
        assert_eq!(l.spans[0].style.fg, Color::Indexed(1));
        assert!(l.spans[0].style.bold);
    }

    #[test]
    fn truncated_extended_color_is_ignored() {
        let l = parse("\x1b[38;2;10mx");
        assert_eq!(l.spans[0].style.fg, Color::Default);
    }

    #[test]
    fn style_carries_across_lines() {
        let mut p = AnsiParser::new();
        p.parse_line("\x1b[32mgreen");
        let l = p.parse_line("still");
        assert_eq!(l.spans[0].style.fg, Color::Indexed(2));
        p.reset_each_line = true;
        let l = p.parse_line("reset");
        assert_eq!(l.spans[0].style.fg, Color::Default);
    }

    #[test]
    fn unsupported_sequences_are_swallowed() {
        let l = parse("a\x1b[?25hb\x1b[2Jc\x1b[1;1Hd\x1b(Be\x1b7f\x1b]0;title\x07g\x1bP1;2|data\x1b\\h");
        assert_eq!(l.text, "abcdefgh");
    }

    #[test]
    fn controls_and_bell() {
        let l = parse("a\x07b\tc\x01d");
        assert_eq!(l.text, "ab\tcd");
        assert!(l.bell);
    }

    #[test]
    fn unterminated_escape_at_end_of_line_is_dropped() {
        let mut p = AnsiParser::new();
        let l = p.parse_line("text\x1b[31");
        assert_eq!(l.text, "text");
        let l = p.parse_line("next");
        assert_eq!(l.text, "next");
        assert_eq!(l.spans[0].style.fg, Color::Default);
    }

    #[test]
    fn osc8_links() {
        let l = parse("see \x1b]8;;https://example.com\x1b\\here\x1b]8;;\x1b\\ done");
        let s = spans(&l);
        assert_eq!(s.len(), 3);
        assert_eq!(s[1].0, "here");
        assert_eq!(s[1].1.link.as_deref(), Some("https://example.com"));
        assert_eq!(s[2].1.link, None);
    }

    #[test]
    fn osc8_with_bel_and_params() {
        let l = parse("\x1b]8;id=1;send:look\x07look\x1b]8;;\x07");
        assert_eq!(l.spans[0].style.link.as_deref(), Some("send:look"));
    }

    #[test]
    fn osc_interrupted_by_csi() {
        let l = parse("\x1b]8;;broken\x1b[31mred");
        assert_eq!(l.text, "red");
        assert_eq!(l.spans[0].style.fg, Color::Indexed(1));
    }

    #[test]
    fn utf8_text_offsets() {
        let l = parse("\x1b[31mé\x1b[32m→");
        assert_eq!(l.text, "é→");
        assert_eq!((l.spans[0].start, l.spans[0].end), (0, 2));
        assert_eq!((l.spans[1].start, l.spans[1].end), (2, 5));
    }

    #[test]
    fn intensity_and_attribute_resets() {
        let l = parse("\x1b[1;2;3;4;5;7;9m\x1b[22;23;24;25;27;29mx");
        assert_eq!(l.spans[0].style, Style::default());
    }

    #[test]
    fn underline_subparam_zero_turns_off() {
        let l = parse("\x1b[4m\x1b[4:0mx");
        assert!(!l.spans[0].style.underline);
    }

    #[test]
    fn sgr_overflow_and_out_of_range_colors_are_ignored() {
        let l = parse("\x1b[31m\x1b[999999999999mx");
        assert_eq!(l.spans[0].style.fg, Color::Indexed(1), "overflowing param must not reset style");
        let l = parse("\x1b[38;5;256ma\x1b[38;2;256;0;0mb");
        assert_eq!(l.spans[0].style.fg, Color::Default);
    }

    #[test]
    fn ignore_string_handles_double_esc_st() {
        let l = parse("a\x1bPignored\x1b\x1b\\b");
        assert_eq!(l.text, "ab");
    }

    #[test]
    fn osc8_overflow_and_control_chars_rejected() {
        let long = format!("\x1b]8;;https://example.com/{}\x07hi\x1b]8;;\x07", "a".repeat(MAX_OSC_LEN));
        let l = parse(&long);
        assert_eq!(l.spans[0].style.link, None);

        let ctrl = parse("\x1b]8;;https://example.com/\x01bad\x1b\\hi\x1b]8;;\x1b\\");
        assert_eq!(ctrl.spans[0].style.link, None);
    }

    #[test]
    fn csi_length_cap_and_large_sgr_are_fast() {
        let huge = format!("\x1b[{}mhi", "38;5;1;".repeat(5000));
        let l = parse(&huge);
        assert_eq!(l.text, "hi");
    }
}
