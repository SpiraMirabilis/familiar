//! A small fuzzy matcher for the pickers, the same as the terminal client's: the query's
//! characters must appear in order. Matches at word starts and runs of consecutive characters
//! score higher.

/// The score and the matched character positions, or None if `query` doesn't match.
pub fn score(query: &str, text: &str) -> Option<(i64, Vec<usize>)> {
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars.iter().flat_map(|c| c.to_lowercase()).collect();
    if lower.len() != chars.len() {
        // Rare case-mapping length changes: fall back to a plain substring check.
        return text.to_lowercase().contains(&query.to_lowercase()).then(|| (1, Vec::new()));
    }
    let mut positions = Vec::new();
    let mut score = 0i64;
    let mut at = 0;
    let mut prev: Option<usize> = None;
    for q in query.chars().flat_map(char::to_lowercase) {
        if q == ' ' {
            continue;
        }
        let found = (at..lower.len()).find(|&i| lower[i] == q)?;
        let word_start = found == 0 || !chars[found - 1].is_alphanumeric();
        score += 10;
        if word_start {
            score += 20;
        }
        if prev.is_some_and(|p| p + 1 == found) {
            score += 15;
        }
        score -= (found - at) as i64;
        positions.push(found);
        prev = Some(found);
        at = found + 1;
    }
    if positions.first() == Some(&0) {
        score += 30;
    }
    // Shorter texts win ties.
    score -= chars.len() as i64 / 8;
    Some((score, positions))
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn ranks_word_starts_and_runs() {
        assert!(score("lm", "LambdaMOO").is_some());
        assert!(score("xyz", "LambdaMOO").is_none());
        let (a, _) = score("aard", "Aardwolf").unwrap();
        let (b, _) = score("aard", "Star Wars: a rad dawn").unwrap_or((0, vec![]));
        assert!(a > b);
        assert_eq!(score("fp", "Fluff Park").unwrap().1, [0, 6]);
    }
}
