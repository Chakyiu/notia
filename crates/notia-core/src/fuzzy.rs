/// Scores how well `query` matches `candidate` as a case-insensitive
/// subsequence. Higher is better; `None` means some character of the query
/// does not occur in order. Matches at word starts, consecutive runs and
/// exact substrings score higher, and shorter candidates win ties.
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i64> {
    let query: Vec<char> = query
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if query.is_empty() {
        return Some(0);
    }
    let original: Vec<char> = candidate.chars().collect();
    let lower: Vec<char> = original
        .iter()
        .map(|ch| ch.to_lowercase().next().unwrap_or(*ch))
        .collect();

    let mut score = 0i64;
    let mut next = 0;
    let mut previous: Option<usize> = None;
    for &wanted in &query {
        let ix = next + lower[next..].iter().position(|&ch| ch == wanted)?;
        score += 1;
        if previous.is_some_and(|previous| previous + 1 == ix) {
            score += 5;
        }
        if is_word_start(&original, ix) {
            score += 10;
        } else if original[ix].is_uppercase() && original[ix - 1].is_lowercase() {
            score += 6;
        }
        previous = Some(ix);
        next = ix + 1;
    }

    let haystack: String = lower.iter().collect();
    let needle: String = query.iter().collect();
    if let Some(byte_ix) = haystack.find(&needle) {
        let at = haystack[..byte_ix].chars().count();
        score += match at {
            0 => 30,
            at if is_word_start(&original, at) => 20,
            _ => 10,
        };
    }
    Some(score - original.len() as i64 / 8)
}

fn is_word_start(chars: &[char], ix: usize) -> bool {
    ix == 0 || !chars[ix - 1].is_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_characters_in_order() {
        assert!(fuzzy_score("dly", "Daily note").is_some());
        assert!(fuzzy_score("yld", "Daily note").is_none());
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }

    #[test]
    fn prefers_prefixes_word_starts_and_short_names() {
        let prefix = fuzzy_score("plan", "Plan").unwrap();
        let inner = fuzzy_score("plan", "Airplane").unwrap();
        let scattered = fuzzy_score("plan", "Project lean analysis notes").unwrap();
        assert!(prefix > inner, "{prefix} > {inner}");
        assert!(inner > scattered, "{inner} > {scattered}");
        assert!(fuzzy_score("rn", "Rust notes").unwrap() > fuzzy_score("rn", "Burn").unwrap());
    }
}
