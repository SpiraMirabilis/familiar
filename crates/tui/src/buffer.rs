//! Scrollback: styled lines, word-wrapped to the pane's width (cached per line), drawn from the
//! bottom up with a scroll offset counted in screen rows.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ops::Range;

use familiar_ansi::StyledLine;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Server,
    /// A message from the client.
    Notice,
    /// A command the user sent.
    Echo,
}

impl Kind {
    /// What goes before the first row, and its width (continuation rows are indented by it).
    fn prefix(self) -> (&'static str, Style) {
        match self {
            Kind::Server => ("", Style::new()),
            Kind::Notice => ("∙ ", theme::faint()),
            Kind::Echo => ("› ", theme::accent().add_modifier(Modifier::BOLD)),
        }
    }
}

/// A character cell in a buffer: an entry and the bytes of the character there (empty when the
/// position is past the text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub seq: u64,
    pub start: usize,
    pub end: usize,
}

/// Text selected with the mouse. It's held in terms of the buffer's content rather than the
/// screen, so it stays put while the buffer scrolls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// The `Buffer::id` it's in.
    pub buffer: u64,
    pub anchor: Cell,
    pub head: Cell,
}

impl Selection {
    /// The first and last cells, in reading order.
    fn ends(&self) -> (Cell, Cell) {
        if (self.anchor.seq, self.anchor.start) <= (self.head.seq, self.head.start) { (self.anchor, self.head) } else { (self.head, self.anchor) }
    }

    /// The selected bytes of entry `seq`, whose text is `len` bytes long.
    fn within(&self, seq: u64, len: usize) -> Option<Range<usize>> {
        let (lo, hi) = self.ends();
        if seq < lo.seq || seq > hi.seq {
            return None;
        }
        let start = if seq == lo.seq { lo.start.min(len) } else { 0 };
        let end = if seq == hi.seq { hi.end.min(len) } else { len };
        Some(start..end.max(start))
    }
}

pub struct Entry {
    pub line: StyledLine,
    pub kind: Kind,
    /// Its place in everything the buffer has ever held.
    pub seq: u64,
    /// Row byte ranges for the last width wrapped to.
    rows: RefCell<Option<(u16, Vec<Range<usize>>)>>,
}

impl Entry {
    pub fn new(line: StyledLine, kind: Kind) -> Self {
        Self { line: expand_tabs(line), kind, seq: 0, rows: RefCell::new(None) }
    }

    fn rows(&self, width: u16) -> std::cell::Ref<'_, Vec<Range<usize>>> {
        let cached = matches!(&*self.rows.borrow(), Some((w, _)) if *w == width);
        if !cached {
            let prefix = UnicodeWidthStr::width(self.kind.prefix().0);
            let rows = wrap(&self.line.text, (width as usize).saturating_sub(prefix).max(1));
            *self.rows.borrow_mut() = Some((width, rows));
        }
        std::cell::Ref::map(self.rows.borrow(), |r| &r.as_ref().unwrap().1)
    }

    pub fn row_count(&self, width: u16) -> usize {
        self.rows(width).len()
    }

    /// One wrapped row as a styled line (with the selected part, if any, highlighted), plus the
    /// links in it as (start column, end column, uri).
    pub fn row(&self, width: u16, index: usize, selection: Option<&Selection>) -> (Line<'static>, Vec<(u16, u16, String)>) {
        let selected = selection.and_then(|s| s.within(self.seq, self.line.text.len())).unwrap_or(0..0);
        let range = self.rows(width).get(index).cloned().unwrap_or(0..0);
        let (prefix, prefix_style) = self.kind.prefix();
        let mut spans = Vec::new();
        let mut links = Vec::new();
        let mut col = UnicodeWidthStr::width(prefix) as u16;
        if !prefix.is_empty() {
            spans.push(if index == 0 { Span::styled(prefix, prefix_style) } else { Span::raw(" ".repeat(col as usize)) });
        }
        let base = match self.kind {
            Kind::Notice => theme::muted().add_modifier(Modifier::ITALIC),
            Kind::Echo => theme::accent(),
            Kind::Server => Style::new(),
        };
        for span in &self.line.spans {
            let (s, e) = (span.start.max(range.start), span.end.min(range.end));
            if s >= e {
                continue;
            }
            let text = &self.line.text[s..e];
            let w = UnicodeWidthStr::width(text) as u16;
            if let Some(link) = &span.style.link {
                links.push((col, col + w, link.to_string()));
            }
            let style = base.patch(theme::style(&span.style));
            // Up to three pieces: before, inside and after the selection.
            let (a, b) = (selected.start.clamp(s, e), selected.end.clamp(s, e));
            for (from, to, on) in [(s, a, false), (a, b, true), (b, e, false)] {
                if from < to {
                    let style = if on { style.add_modifier(Modifier::REVERSED) } else { style };
                    spans.push(Span::styled(self.line.text[from..to].to_string(), style));
                }
            }
            col += w;
        }
        (Line::from(spans), links)
    }

    /// The cell at screen column `col` (counted from the pane's left edge) of a wrapped row.
    pub fn cell_at(&self, width: u16, index: usize, col: usize) -> Cell {
        let range = self.rows(width).get(index).cloned().unwrap_or(0..0);
        let mut x = UnicodeWidthStr::width(self.kind.prefix().0);
        if col >= x {
            for (i, g) in self.line.text[range.clone()].grapheme_indices(true) {
                x += UnicodeWidthStr::width(g);
                if col < x {
                    return Cell { seq: self.seq, start: range.start + i, end: range.start + i + g.len() };
                }
            }
            return Cell { seq: self.seq, start: range.end, end: range.end };
        }
        Cell { seq: self.seq, start: range.start, end: range.start }
    }

    pub fn plain(&self) -> &str {
        &self.line.text
    }
}

/// Replaces tabs with spaces (to the next multiple of 8), keeping span styles.
fn expand_tabs(line: StyledLine) -> StyledLine {
    if !line.text.contains('\t') && !line.text.contains('\r') {
        return line;
    }
    let mut out = StyledLine { text: String::new(), spans: Vec::new(), bell: line.bell };
    let mut col = 0;
    for span in &line.spans {
        let start = out.text.len();
        for c in line.text[span.start..span.end].chars() {
            match c {
                '\t' => {
                    let n = 8 - col % 8;
                    out.text.push_str(&" ".repeat(n));
                    col += n;
                }
                '\r' => {}
                c => {
                    out.text.push(c);
                    col += 1;
                }
            }
        }
        if out.text.len() > start {
            out.spans.push(familiar_ansi::Span { start, end: out.text.len(), style: span.style.clone() });
        }
    }
    out
}

/// Splits text into rows no wider than `width` columns, breaking after spaces where it can.
pub fn wrap(text: &str, width: usize) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut start = 0;
    let mut used = 0;
    // Where the row could end at a space, and the width up to there.
    let mut last_break: Option<usize> = None;
    for (i, g) in text.grapheme_indices(true) {
        let w = UnicodeWidthStr::width(g);
        if used + w > width && i > start {
            let end = match last_break {
                Some(b) if b > start => b,
                _ => i,
            };
            rows.push(start..end);
            start = end;
            // Recount what has moved onto the new row; if it still doesn't fit, break here.
            used = UnicodeWidthStr::width(&text[start..i]);
            if used + w > width && i > start {
                rows.push(start..i);
                start = i;
                used = 0;
            }
            last_break = None;
        }
        used += w;
        if g == " " {
            last_break = Some(i + 1);
        }
    }
    rows.push(start..text.len());
    rows
}

pub struct Buffer {
    /// Tells buffers apart (for the selection).
    pub id: u64,
    next_seq: u64,
    entries: VecDeque<Entry>,
    max: usize,
    /// Rows scrolled up from the bottom. 0 follows new output.
    pub scroll: usize,
    /// Lines that arrived while scrolled up.
    pub unseen: usize,
    /// The width last drawn at, for keeping the view still when lines arrive while scrolled.
    width: u16,
}

impl Buffer {
    pub fn new(max: usize) -> Self {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self { id, next_seq: 0, entries: VecDeque::new(), max: max.max(100), scroll: 0, unseen: 0, width: 80 }
    }

    pub fn push(&mut self, line: StyledLine, kind: Kind) {
        let mut entry = Entry::new(line, kind);
        entry.seq = self.next_seq;
        self.next_seq += 1;
        if self.scroll > 0 {
            self.scroll += entry.row_count(self.width);
            self.unseen += 1;
        }
        self.entries.push_back(entry);
        while self.entries.len() > self.max {
            self.entries.pop_front();
        }
    }

    pub fn notice(&mut self, text: &str) {
        for line in text.lines() {
            self.push(StyledLine::plain(line), Kind::Notice);
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.scroll = 0;
        self.unseen = 0;
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn entry(&self, seq: u64) -> Option<&Entry> {
        let first = self.entries.front()?.seq;
        self.entries.get(usize::try_from(seq.checked_sub(first)?).ok()?)
    }

    /// The selected text, one line per entry (so a wrapped line comes back whole).
    pub fn selected_text(&self, selection: &Selection) -> String {
        let (lo, hi) = selection.ends();
        let lines: Vec<&str> =
            self.entries.iter().filter(|e| (lo.seq..=hi.seq).contains(&e.seq)).filter_map(|e| Some(&e.line.text[selection.within(e.seq, e.line.text.len())?])).collect();
        lines.join("\n")
    }

    pub fn total_rows(&self, width: u16) -> usize {
        self.entries.iter().map(|e| e.row_count(width)).sum()
    }

    pub fn scroll_by(&mut self, delta: isize, width: u16, height: u16) {
        let max = self.total_rows(width).saturating_sub(height as usize);
        self.scroll = (self.scroll as isize + delta).clamp(0, max as isize) as usize;
        if self.scroll == 0 {
            self.unseen = 0;
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll = 0;
        self.unseen = 0;
    }

    /// The rows to draw for a pane of this size: (entry, row within it), top to bottom.
    pub fn visible(&mut self, width: u16, height: u16, scroll: usize) -> Vec<(&Entry, usize)> {
        self.width = width;
        let want = scroll + height as usize;
        let mut rows: Vec<(&Entry, usize)> = Vec::with_capacity(want);
        'outer: for entry in self.entries.iter().rev() {
            let n = entry.row_count(width);
            for r in (0..n).rev() {
                rows.push((entry, r));
                if rows.len() >= want {
                    break 'outer;
                }
            }
        }
        let skip = scroll.min(rows.len());
        let mut shown: Vec<_> = rows.into_iter().skip(skip).collect();
        shown.reverse();
        shown
    }

    /// Words from recent lines that start with `prefix` (ignoring case), most recent first.
    pub fn words_starting(&self, prefix: &str, limit_lines: usize) -> Vec<String> {
        let lower = prefix.to_lowercase();
        let mut out: Vec<String> = Vec::new();
        for entry in self.entries.iter().rev().take(limit_lines) {
            for word in entry.plain().split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-' || c == '\'')) {
                let word = word.trim_matches(|c| c == '\'' || c == '-');
                if word.len() > prefix.len() && word.to_lowercase().starts_with(&lower) && !out.iter().any(|w| w == word) {
                    out.push(word.to_string());
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str, width: usize) -> Vec<&str> {
        wrap(text, width).into_iter().map(|r| &text[r]).collect()
    }

    #[test]
    fn wraps_at_spaces_and_hard_breaks_long_words() {
        assert_eq!(rows("the quick brown fox", 10), ["the quick ", "brown fox"]);
        assert_eq!(rows("abcdefghijkl", 5), ["abcde", "fghij", "kl"]);
        assert_eq!(rows("", 5), [""]);
        assert_eq!(rows("日本語のテキスト", 6), ["日本語", "のテキ", "スト"], "wide characters count double");
    }

    #[test]
    fn scrolling_keeps_the_view_still_and_counts_new_lines() {
        let mut b = Buffer::new(1000);
        for i in 0..50 {
            b.push(StyledLine::plain(format!("line {i}")), Kind::Server);
        }
        assert_eq!(b.visible(20, 3, 0).iter().map(|(e, _)| e.plain()).collect::<Vec<_>>(), ["line 47", "line 48", "line 49"]);
        b.scroll_by(10, 20, 3);
        b.push(StyledLine::plain("new"), Kind::Server);
        let shown: Vec<&str> = b.visible(20, 3, b.scroll).iter().map(|(e, _)| e.plain()).collect();
        assert_eq!(shown, ["line 37", "line 38", "line 39"]);
        assert_eq!(b.unseen, 1);
        b.scroll_to_bottom();
        assert_eq!(b.visible(20, 1, 0)[0].0.plain(), "new");
    }

    #[test]
    fn tabs_become_spaces_and_links_are_located() {
        let mut line = familiar_ansi::AnsiParser::new().parse_line("a\tb see https://example.org/x");
        line.linkify();
        let e = Entry::new(line, Kind::Echo);
        assert_eq!(e.plain(), "a       b see https://example.org/x");
        let (_, links) = e.row(80, 0, None);
        assert_eq!(links, [(16, 37, "https://example.org/x".to_string())], "after the 2-column echo prefix");
    }

    #[test]
    fn selection_follows_content_and_copies_whole_lines() {
        let mut b = Buffer::new(100);
        b.push(StyledLine::plain("the quick brown fox"), Kind::Server);
        b.push(StyledLine::plain("hello"), Kind::Echo);
        // Wrapped at 10: "the quick " / "brown fox". From the q of "quick" to the o of "brown".
        let anchor = b.entry(0).unwrap().cell_at(10, 0, 4);
        assert_eq!((anchor.start, anchor.end), (4, 5));
        let head = b.entry(0).unwrap().cell_at(10, 1, 2);
        let sel = Selection { buffer: b.id, anchor, head };
        assert_eq!(b.selected_text(&sel), "quick bro", "no newline where the line only wrapped");
        // Backwards, into the next entry, past its end and inside its prefix.
        let end = b.entry(1).unwrap().cell_at(10, 0, 50);
        assert_eq!(b.selected_text(&Selection { buffer: b.id, anchor: end, head: anchor }), "quick brown fox\nhello");
        let prefix = b.entry(1).unwrap().cell_at(10, 0, 1);
        assert_eq!(b.selected_text(&Selection { buffer: b.id, anchor, head: prefix }), "quick brown fox\n");
        // The highlight splits the row's spans.
        let (line, _) = b.entry(0).unwrap().row(10, 0, Some(&sel));
        let parts: Vec<(&str, bool)> = line.spans.iter().map(|s| (s.content.as_ref(), s.style.add_modifier.contains(Modifier::REVERSED))).collect();
        assert_eq!(parts, [("the ", false), ("quick ", true)]);
        // Entries are still found after older ones are dropped.
        for i in 0..200 {
            b.push(StyledLine::plain(format!("line {i}")), Kind::Server);
        }
        assert!(b.entry(0).is_none());
        assert_eq!(b.entry(201).unwrap().plain(), "line 199");
    }

    #[test]
    fn recent_words_for_completion() {
        let mut b = Buffer::new(100);
        b.push(StyledLine::plain("Gandalf says hi to Galadriel."), Kind::Server);
        b.push(StyledLine::plain("Gimli waves."), Kind::Server);
        assert_eq!(b.words_starting("g", 50), ["Gimli", "Gandalf", "Galadriel"]);
        assert_eq!(b.words_starting("gal", 50), ["Galadriel"]);
    }
}
