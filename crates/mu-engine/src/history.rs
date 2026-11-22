//! Input history with shell-style navigation. The unsent text is kept as a draft while browsing.

#[derive(Debug, Clone)]
pub struct History {
    entries: Vec<String>,
    max: usize,
    /// Index of the entry being shown, or None while editing new text.
    cursor: Option<usize>,
    draft: String,
}

impl History {
    pub fn new(max: usize) -> Self {
        Self { entries: Vec::new(), max: max.max(1), cursor: None, draft: String::new() }
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Records a sent line. Blank lines and repeats of the last entry are skipped.
    pub fn push(&mut self, line: &str) {
        self.cursor = None;
        self.draft.clear();
        if line.trim().is_empty() || self.entries.last().is_some_and(|l| l == line) {
            return;
        }
        self.entries.push(line.to_string());
        if self.entries.len() > self.max {
            let excess = self.entries.len() - self.max;
            self.entries.drain(..excess);
        }
    }

    /// Moves to an older entry. `current` is the text in the input box, saved as the draft when
    /// browsing starts. Returns None if there is nothing older.
    pub fn older(&mut self, current: &str) -> Option<String> {
        let index = match self.cursor {
            None if self.entries.is_empty() => return None,
            None => {
                self.draft = current.to_string();
                self.entries.len() - 1
            }
            Some(0) => return None,
            Some(i) => i - 1,
        };
        self.cursor = Some(index);
        Some(self.entries[index].clone())
    }

    /// Moves to a newer entry, ending with the saved draft. Returns None if not browsing.
    pub fn newer(&mut self) -> Option<String> {
        let i = self.cursor?;
        if i + 1 < self.entries.len() {
            self.cursor = Some(i + 1);
            Some(self.entries[i + 1].clone())
        } else {
            self.cursor = None;
            Some(std::mem::take(&mut self.draft))
        }
    }

    /// Stops browsing, for example when the user edits the recalled text.
    pub fn reset(&mut self) {
        self.cursor = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_keeps_draft() {
        let mut h = History::new(10);
        h.push("one");
        h.push("two");
        assert_eq!(h.older("dra").as_deref(), Some("two"));
        assert_eq!(h.older("two").as_deref(), Some("one"));
        assert_eq!(h.older("one"), None);
        assert_eq!(h.newer().as_deref(), Some("two"));
        assert_eq!(h.newer().as_deref(), Some("dra"));
        assert_eq!(h.newer(), None);
    }

    #[test]
    fn skips_blank_and_repeats_and_caps_size() {
        let mut h = History::new(2);
        for line in ["a", "a", " ", "b", "c"] {
            h.push(line);
        }
        assert_eq!(h.entries(), ["b", "c"]);
    }

    #[test]
    fn empty_history() {
        let mut h = History::new(5);
        assert_eq!(h.older("x"), None);
        assert_eq!(h.newer(), None);
    }
}
