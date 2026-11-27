//! Splits telnet application data into lines and prompts.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// Line bytes in the server's encoding, without CR/LF.
    pub bytes: Vec<u8>,
    /// True if the line ended with GA/EOR (or a prompt timeout) instead of a newline.
    pub prompt: bool,
}

#[derive(Debug, Default)]
pub struct LineSplitter {
    partial: Vec<u8>,
}

/// Maximum bytes accumulated for a single unterminated line before flushing a forced split.
const MAX_LINE_BYTES: usize = 256 * 1024;

impl LineSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Splits `data` on LF and returns the complete lines. CR and NUL are dropped.
    pub fn push(&mut self, data: &[u8]) -> Vec<Line> {
        let mut lines = Vec::new();
        for &b in data {
            match b {
                b'\n' => lines.push(Line { bytes: std::mem::take(&mut self.partial), prompt: false }),
                b'\r' | 0 => {}
                _ => {
                    self.partial.push(b);
                    if self.partial.len() >= MAX_LINE_BYTES {
                        lines.push(Line { bytes: std::mem::take(&mut self.partial), prompt: false });
                    }
                }
            }
        }
        lines
    }

    /// Ends the partial line as a prompt (after GA/EOR). Returns None if there is no partial line.
    pub fn take_prompt(&mut self) -> Option<Line> {
        if self.partial.is_empty() {
            None
        } else {
            Some(Line { bytes: std::mem::take(&mut self.partial), prompt: true })
        }
    }

    /// The text received since the last newline. Used to show an unterminated prompt without
    /// consuming it.
    pub fn partial(&self) -> &[u8] {
        &self.partial
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_strips_cr() {
        let mut s = LineSplitter::new();
        let lines = s.push(b"one\r\ntwo\n\rthr");
        assert_eq!(lines.iter().map(|l| l.bytes.as_slice()).collect::<Vec<_>>(), vec![b"one" as &[u8], b"two"]);
        assert_eq!(s.partial(), b"thr");
        let lines = s.push(b"ee\r\n");
        assert_eq!(lines[0].bytes, b"three");
    }

    #[test]
    fn prompt_takes_partial() {
        let mut s = LineSplitter::new();
        s.push(b"HP: 10> ");
        assert_eq!(s.take_prompt(), Some(Line { bytes: b"HP: 10> ".to_vec(), prompt: true }));
        assert_eq!(s.take_prompt(), None);
    }

    #[test]
    fn empty_lines_are_kept() {
        let mut s = LineSplitter::new();
        assert_eq!(s.push(b"\r\n\r\n").len(), 2);
    }

    #[test]
    fn overly_long_unterminated_line_is_flushed() {
        let mut s = LineSplitter::new();
        let data = vec![b'a'; MAX_LINE_BYTES + 10];
        let lines = s.push(&data);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].bytes.len(), MAX_LINE_BYTES);
        assert_eq!(s.partial().len(), 10);
    }
}
