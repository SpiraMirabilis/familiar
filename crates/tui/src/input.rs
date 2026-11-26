//! The command line: a small multi-line editor with readline keys and history.

use familiar_engine::history::History;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub struct Input {
    pub text: String,
    /// Byte offset, always on a grapheme boundary.
    pub cursor: usize,
    history: History,
    /// The last killed text, for Ctrl+Y.
    kill: String,
    /// Tab completion in progress: (start of the word, candidates, which one is shown).
    completion: Option<(usize, Vec<String>, usize)>,
    /// The other end of the selection, which runs from here to the cursor.
    anchor: Option<usize>,
    /// The text is a history entry, brought back with ↑ or ↓ and not changed since.
    pub recalled: bool,
}

impl Input {
    pub fn new(history_size: usize) -> Self {
        Self { text: String::new(), cursor: 0, history: History::new(history_size), kill: String::new(), completion: None, anchor: None, recalled: false }
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn set(&mut self, text: &str) {
        self.text = text.to_string();
        self.cursor = self.text.len();
        self.edited();
    }

    /// Forgets what belonged to the text before it changed.
    fn edited(&mut self) {
        self.completion = None;
        self.anchor = None;
        self.recalled = false;
    }

    /// The selected text's byte range, if any is selected.
    pub fn selection(&self) -> Option<std::ops::Range<usize>> {
        let anchor = self.anchor.filter(|a| *a != self.cursor)?;
        Some(anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|r| &self.text[r])
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.text.len();
    }

    /// Moves the cursor with `motion` (`Input::left` and the like), selecting what it passes.
    pub fn select(&mut self, motion: impl FnOnce(&mut Self)) {
        let anchor = self.anchor.take().unwrap_or(self.cursor);
        motion(self);
        self.anchor = Some(anchor);
    }

    /// Deletes the selected text. Returns false if nothing was selected.
    pub fn delete_selection(&mut self) -> bool {
        let Some(range) = self.selection() else { return false };
        self.cursor = range.start;
        self.text.replace_range(range, "");
        self.edited();
        true
    }

    /// Takes the text to send, remembering it in history.
    pub fn take(&mut self, remember: bool) -> String {
        let text = std::mem::take(&mut self.text);
        self.cursor = 0;
        self.edited();
        if remember {
            self.history.push(&text);
        }
        self.history.reset();
        text
    }

    pub fn history(&self) -> &[String] {
        self.history.entries()
    }

    /// Types `s`, in place of the selection if there is one.
    pub fn insert(&mut self, s: &str) {
        self.delete_selection();
        self.edited();
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    fn prev_boundary(&self, at: usize) -> usize {
        self.text[..at].grapheme_indices(true).next_back().map_or(0, |(i, _)| i)
    }

    fn next_boundary(&self, at: usize) -> usize {
        self.text[at..].graphemes(true).next().map_or(at, |g| at + g.len())
    }

    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor > 0 {
            let start = self.prev_boundary(self.cursor);
            self.text.replace_range(start..self.cursor, "");
            self.cursor = start;
        }
        self.edited();
    }

    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        let end = self.next_boundary(self.cursor);
        self.text.replace_range(self.cursor..end, "");
        self.edited();
    }

    /// ← and →: with text selected, to that end of the selection.
    pub fn left(&mut self) {
        match self.selection() {
            Some(range) => self.cursor = range.start,
            None => self.cursor = self.prev_boundary(self.cursor),
        }
        self.anchor = None;
    }

    pub fn right(&mut self) {
        match self.selection() {
            Some(range) => self.cursor = range.end,
            None => self.cursor = self.next_boundary(self.cursor),
        }
        self.anchor = None;
    }

    fn word_start(&self, from: usize) -> usize {
        let before = &self.text[..from];
        let trimmed = before.trim_end();
        trimmed.rfind(char::is_whitespace).map_or(0, |i| i + trimmed[i..].chars().next().map_or(1, char::len_utf8))
    }

    fn word_end(&self, from: usize) -> usize {
        let after = &self.text[from..];
        let skip = after.len() - after.trim_start().len();
        let rest = &after[skip..];
        from + skip + rest.find(char::is_whitespace).unwrap_or(rest.len())
    }

    pub fn word_left(&mut self) {
        self.cursor = self.word_start(self.cursor);
        self.anchor = None;
    }

    pub fn word_right(&mut self) {
        self.cursor = self.word_end(self.cursor);
        self.anchor = None;
    }

    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.cursor..].find('\n').map_or(self.text.len(), |i| self.cursor + i)
    }

    pub fn home(&mut self) {
        self.cursor = self.line_start();
        self.anchor = None;
    }

    pub fn end(&mut self) {
        self.cursor = self.line_end();
        self.anchor = None;
    }

    fn kill_range(&mut self, start: usize, end: usize) {
        if start < end {
            self.kill = self.text[start..end].to_string();
            self.text.replace_range(start..end, "");
            self.cursor = start;
        }
        self.edited();
    }

    /// Ctrl+U: delete to the start of the line.
    pub fn kill_to_start(&mut self) {
        self.kill_range(self.line_start(), self.cursor);
    }

    /// Ctrl+K: delete to the end of the line.
    pub fn kill_to_end(&mut self) {
        self.kill_range(self.cursor, self.line_end());
    }

    /// Ctrl+W: delete the word before the cursor.
    pub fn kill_word(&mut self) {
        self.kill_range(self.word_start(self.cursor), self.cursor);
    }

    /// Alt+D: delete the word after the cursor.
    pub fn kill_word_forward(&mut self) {
        self.kill_range(self.cursor, self.word_end(self.cursor));
    }

    pub fn yank(&mut self) {
        let text = self.kill.clone();
        self.insert(&text);
    }

    /// Moves up a line inside multi-line text. Returns false on the first line.
    pub fn line_up(&mut self) -> bool {
        self.anchor = None;
        let start = self.line_start();
        if start == 0 {
            return false;
        }
        let col = UnicodeWidthStr::width(&self.text[start..self.cursor]);
        let prev_start = self.text[..start - 1].rfind('\n').map_or(0, |i| i + 1);
        self.cursor = column_to_byte(&self.text[prev_start..start - 1], col) + prev_start;
        true
    }

    /// Moves down a line. Returns false on the last line.
    pub fn line_down(&mut self) -> bool {
        self.anchor = None;
        let end = self.line_end();
        if end == self.text.len() {
            return false;
        }
        let col = UnicodeWidthStr::width(&self.text[self.line_start()..self.cursor]);
        let next_start = end + 1;
        let next_end = self.text[next_start..].find('\n').map_or(self.text.len(), |i| next_start + i);
        self.cursor = column_to_byte(&self.text[next_start..next_end], col) + next_start;
        true
    }

    pub fn history_older(&mut self) {
        if let Some(text) = self.history.older(&self.text) {
            self.set(&text);
            self.recalled = true;
        }
    }

    pub fn history_newer(&mut self) {
        if let Some(text) = self.history.newer() {
            self.set(&text);
            self.recalled = true;
        }
    }

    /// The word before the cursor and where it starts, for completion.
    pub fn current_word(&self) -> (usize, &str) {
        let start = self.text[..self.cursor].rfind(|c: char| c.is_whitespace()).map_or(0, |i| i + 1);
        (start, &self.text[start..self.cursor])
    }

    /// Tab: completes the word before the cursor from `candidates` (called with the word), and
    /// cycles through them on repeated presses. Returns false if there was nothing to complete.
    pub fn complete(&mut self, candidates: impl FnOnce(&str) -> Vec<String>) -> bool {
        self.anchor = None;
        self.recalled = false;
        if let Some((start, list, i)) = self.completion.take() {
            let next = (i + 1) % list.len();
            self.text.replace_range(start..self.cursor, &list[next]);
            self.cursor = start + list[next].len();
            self.completion = Some((start, list, next));
            return true;
        }
        let (start, word) = self.current_word();
        if word.is_empty() {
            return false;
        }
        let list = candidates(word);
        let Some(first) = list.first().cloned() else { return false };
        self.text.replace_range(start..self.cursor, &first);
        self.cursor = start + first.len();
        self.completion = Some((start, list, 0));
        true
    }

    /// The text broken into display rows of `width` columns, and the cursor's (column, row).
    pub fn layout(&self, width: u16) -> (Vec<String>, (u16, u16)) {
        let (rows, cursor) = self.layout_rows(width);
        (rows.into_iter().map(|(_, row)| row).collect(), cursor)
    }

    /// As `layout`, with where each row starts in the text.
    pub fn layout_rows(&self, width: u16) -> (Vec<(usize, String)>, (u16, u16)) {
        let width = width.max(2) as usize;
        let mut rows = Vec::new();
        let mut cursor = (0, 0);
        let mut offset = 0;
        for line in self.text.split('\n') {
            let mut row = String::new();
            let mut start = offset;
            let mut used = 0;
            for (i, g) in line.grapheme_indices(true) {
                let w = UnicodeWidthStr::width(g);
                if used + w > width {
                    rows.push((start, std::mem::take(&mut row)));
                    start = offset + i;
                    used = 0;
                }
                if offset + i == self.cursor {
                    cursor = (used as u16, rows.len() as u16);
                }
                row.push_str(g);
                used += w;
            }
            if offset + line.len() == self.cursor {
                if used >= width {
                    rows.push((start, std::mem::take(&mut row)));
                    start = offset + line.len();
                    used = 0;
                }
                cursor = (used as u16, rows.len() as u16);
            }
            rows.push((start, row));
            offset += line.len() + 1;
        }
        (rows, cursor)
    }
}

fn column_to_byte(line: &str, col: usize) -> usize {
    let mut used = 0;
    for (i, g) in line.grapheme_indices(true) {
        if used >= col {
            return i;
        }
        used += UnicodeWidthStr::width(g);
    }
    line.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> Input {
        let mut i = Input::new(50);
        i.insert(text);
        i
    }

    #[test]
    fn editing_keys() {
        let mut i = typed("say héllo world");
        i.word_left();
        i.kill_word();
        assert_eq!(i.text, "say world");
        i.yank();
        assert_eq!(i.text, "say héllo world");
        i.home();
        i.kill_to_end();
        assert_eq!(i.text, "");
        i.insert("ab");
        i.left();
        i.backspace();
        assert_eq!((i.text.as_str(), i.cursor), ("b", 0));
    }

    #[test]
    fn multiline_movement_and_layout() {
        let mut i = typed("first line\nsecond");
        assert!(i.line_up());
        assert_eq!(&i.text[i.cursor..], "line\nsecond", "same column on the line above");
        assert!(!i.line_up());
        assert!(i.line_down());
        let (rows, cursor) = typed("abcdefgh").layout(4);
        assert_eq!(rows, ["abcd", "efgh", ""]);
        assert_eq!(cursor, (0, 2));
        let (rows, cursor) = typed("ab\ncd").layout(10);
        assert_eq!((rows, cursor), (vec!["ab".to_string(), "cd".to_string()], (2, 1)));
    }

    #[test]
    fn history_and_completion() {
        let mut i = typed("look");
        i.take(true);
        i.insert("kill g");
        i.history_older();
        assert_eq!(i.text, "look");
        i.history_newer();
        assert_eq!(i.text, "kill g");
        let names = |w: &str| ["Gandalf", "Gimli"].iter().filter(|n| n.to_lowercase().starts_with(&w.to_lowercase())).map(|n| n.to_string()).collect();
        assert!(i.complete(names));
        assert_eq!(i.text, "kill Gandalf");
        assert!(i.complete(|_| unreachable!()));
        assert_eq!(i.text, "kill Gimli", "repeated Tab cycles");
    }

    #[test]
    fn recalled_until_changed() {
        let mut i = typed("/world Vale");
        i.take(true);
        i.history_older();
        assert!(i.recalled);
        i.right();
        assert!(i.recalled, "moving about isn't a change");
        i.insert("x");
        assert!(!i.recalled);
    }

    #[test]
    fn selection() {
        let mut i = typed("say hello");
        i.select_all();
        assert_eq!(i.selected_text(), Some("say hello"));
        i.insert("w");
        assert_eq!((i.text.as_str(), i.selection()), ("w", None), "typing replaces the selection");
        let mut i = typed("say héllo");
        i.select(Input::word_left);
        assert_eq!(i.selected_text(), Some("héllo"));
        i.select(Input::left);
        assert_eq!(i.selected_text(), Some(" héllo"), "selecting goes on from the same end");
        i.backspace();
        assert_eq!(i.text, "say");
        let mut i = typed("abc");
        i.select(Input::home);
        i.right();
        assert_eq!((i.cursor, i.selection()), (3, None), "→ goes to the end of the selection");
        i.select(Input::left);
        i.delete();
        assert_eq!(i.text, "ab");
    }
}
