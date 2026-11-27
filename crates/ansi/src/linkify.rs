//! Finds plain-text URLs and turns them into links.

use std::sync::Arc;

use crate::{Span, StyledLine};

/// Returns the byte ranges of http(s) URLs in `text`.
pub fn find_urls(text: &str) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut search = 0;
    while let Some(offset) = text[search..].find("http") {
        let start = search + offset;
        let rest = &text[start..];
        let scheme_len = if rest.starts_with("https://") {
            8
        } else if rest.starts_with("http://") {
            7
        } else {
            search = start + 4;
            continue;
        };
        // A URL has to start at a word boundary, so "xhttp://" doesn't count.
        let boundary = text[..start].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let len = rest.find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`')).unwrap_or(rest.len());
        let mut end = start + len;
        // Drop trailing punctuation, and a closing bracket with no opening one.
        let (mut parens_open, mut parens_close, mut brackets_open, mut brackets_close) = (0usize, 0usize, 0usize, 0usize);
        for b in text[start..end].bytes() {
            match b {
                b'(' => parens_open += 1,
                b')' => parens_close += 1,
                b'[' => brackets_open += 1,
                b']' => brackets_close += 1,
                _ => {}
            }
        }
        loop {
            match text[start..end].chars().next_back() {
                Some('.' | ',' | ';' | ':' | '!' | '?' | '\'') => end -= 1,
                Some(')') if parens_open < parens_close => {
                    end -= 1;
                    parens_close -= 1;
                }
                Some(']') if brackets_open < brackets_close => {
                    end -= 1;
                    brackets_close -= 1;
                }
                _ => break,
            }
        }
        if boundary && end > start + scheme_len {
            found.push((start, end));
        }
        search = end.max(start + scheme_len);
    }
    found
}

impl StyledLine {
    /// Turns plain-text URLs into links. Text that is already an OSC 8 link is left alone.
    pub fn linkify(&mut self) {
        let urls: Vec<(usize, usize, Arc<str>)> = find_urls(&self.text)
            .into_iter()
            .map(|(a, b)| (a, b, Arc::from(&self.text[a..b])))
            .collect();
        if urls.is_empty() {
            return;
        }
        let mut spans: Vec<Span> = Vec::with_capacity(self.spans.len() + urls.len() * 2);
        let mut push = |span: Span| match spans.last_mut() {
            Some(last) if last.style == span.style && last.end == span.start => last.end = span.end,
            _ => spans.push(span),
        };
        let mut u = 0;
        for span in std::mem::take(&mut self.spans) {
            if span.style.link.is_some() {
                push(span);
                continue;
            }
            while u < urls.len() && urls[u].1 <= span.start {
                u += 1;
            }
            let mut pos = span.start;
            let mut j = u;
            while j < urls.len() && urls[j].0 < span.end {
                let (a, b, ref url) = urls[j];
                let (s, e) = (a.max(span.start), b.min(span.end));
                if s < e {
                    if pos < s {
                        push(Span { start: pos, end: s, style: span.style.clone() });
                    }
                    let mut style = span.style.clone();
                    style.link = Some(Arc::clone(url));
                    push(Span { start: s, end: e, style });
                    pos = e;
                }
                if b <= span.end {
                    u = j + 1;
                }
                j += 1;
            }
            if pos < span.end {
                push(Span { start: pos, end: span.end, style: span.style.clone() });
            }
        }
        self.spans = spans;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AnsiParser;

    fn urls(text: &str) -> Vec<&str> {
        find_urls(text).into_iter().map(|(a, b)| &text[a..b]).collect()
    }

    #[test]
    fn finds_urls() {
        assert_eq!(urls("see https://a.org/x?y=1."), vec!["https://a.org/x?y=1"]);
        assert_eq!(urls("(http://a.org/wiki/Foo_(bar))"), vec!["http://a.org/wiki/Foo_(bar)"]);
        assert_eq!(urls("(http://a.org)"), vec!["http://a.org"]);
        assert_eq!(urls("two http://a.org and https://b.org!"), vec!["http://a.org", "https://b.org"]);
        assert!(urls("xhttp://a.org http:// httpd").is_empty());
        assert_eq!(urls("<https://a.org>"), vec!["https://a.org"]);
    }

    #[test]
    fn linkify_splits_styled_spans() {
        let mut line = AnsiParser::new().parse_line("go \x1b[31mhttps://a.org/\x1b[32mpath\x1b[0m now");
        line.linkify();
        let parts: Vec<_> = line.spans.iter().map(|s| (line.span_text(s), s.style.link.as_deref())).collect();
        assert_eq!(
            parts,
            vec![
                ("go ", None),
                ("https://a.org/", Some("https://a.org/path")),
                ("path", Some("https://a.org/path")),
                (" now", None),
            ]
        );
    }

    #[test]
    fn linkify_keeps_osc8_links() {
        let mut line = AnsiParser::new().parse_line("\x1b]8;;send:look\x07https://x.org\x1b]8;;\x07");
        line.linkify();
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].style.link.as_deref(), Some("send:look"));
    }

    #[test]
    fn trailing_parens_are_linear() {
        let input = format!("http://a.org{}", ")".repeat(20_000));
        assert_eq!(urls(&input), vec!["http://a.org"]);
    }
}
