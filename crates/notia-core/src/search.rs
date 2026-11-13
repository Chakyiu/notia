use std::{cmp::Reverse, ops::Range};

use crate::{IndexedNote, VaultPath, index::tag_matches};

/// A search over note names and content.
///
/// Plain words and `"quoted phrases"` must all occur, case-insensitively, in
/// the note's name or text. `tag:work` or `#work` keeps notes tagged `work`
/// (or a nested tag), and `path:` or `file:` filters by vault path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchQuery {
    terms: Vec<String>,
    tags: Vec<String>,
    paths: Vec<String>,
}

impl SearchQuery {
    pub fn parse(input: &str) -> Self {
        let mut query = Self::default();
        let mut rest = input.trim();
        while !rest.is_empty() {
            let token = if let Some(quoted) = rest.strip_prefix('"') {
                let end = quoted.find('"').unwrap_or(quoted.len());
                let phrase = &quoted[..end];
                rest = quoted.get(end + 1..).unwrap_or_default();
                if !phrase.trim().is_empty() {
                    query.terms.push(phrase.to_lowercase());
                }
                rest = rest.trim_start();
                continue;
            } else {
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                let token = &rest[..end];
                rest = rest[end..].trim_start();
                token
            };

            if let Some(tag) = token
                .strip_prefix("tag:")
                .or_else(|| token.strip_prefix('#'))
            {
                let tag = tag.trim_start_matches('#');
                if !tag.is_empty() {
                    query.tags.push(tag.to_lowercase());
                }
            } else if let Some(path) = token
                .strip_prefix("path:")
                .or_else(|| token.strip_prefix("file:"))
            {
                if !path.is_empty() {
                    query.paths.push(path.to_lowercase());
                }
            } else {
                query.terms.push(token.to_lowercase());
            }
        }
        query
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.tags.is_empty() && self.paths.is_empty()
    }

    fn matches_note(&self, note: &IndexedNote) -> bool {
        let path = note.path.as_str().to_lowercase();
        if !self.paths.iter().all(|filter| path.contains(filter)) {
            return false;
        }
        if !self
            .tags
            .iter()
            .all(|query| note.meta.tags.iter().any(|tag| tag_matches(tag, query)))
        {
            return false;
        }
        if self.terms.is_empty() {
            return true;
        }
        let name = note.title().to_lowercase();
        let text = note.text.to_lowercase();
        self.terms
            .iter()
            .all(|term| name.contains(term) || text.contains(term))
    }

    /// What to highlight in matching lines: the terms, or the tags when the
    /// query has no terms.
    fn highlights(&self) -> Vec<String> {
        if self.terms.is_empty() {
            self.tags.iter().map(|tag| format!("#{tag}")).collect()
        } else {
            self.terms.clone()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineMatch {
    /// The 0-based line in the note.
    pub line: usize,
    /// The line, shortened around the first match when it is long.
    pub text: String,
    /// Byte ranges of the matches within `text`.
    pub ranges: Vec<Range<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMatches {
    pub path: VaultPath,
    /// Whether every term occurs in the note's name.
    pub name_matches: bool,
    /// The first matching lines, up to the requested limit.
    pub lines: Vec<LineMatch>,
    /// How many lines match in total.
    pub line_count: usize,
}

/// Runs `query` over `notes`. Notes whose name matches come first, then the
/// notes with the most matching lines.
pub fn search<'a>(
    notes: impl IntoIterator<Item = &'a IndexedNote>,
    query: &SearchQuery,
    max_lines_per_note: usize,
) -> Vec<FileMatches> {
    if query.is_empty() {
        return Vec::new();
    }
    let highlights = query.highlights();
    let mut results: Vec<FileMatches> = notes
        .into_iter()
        .filter(|note| query.matches_note(note))
        .map(|note| {
            let name = note.title().to_lowercase();
            let name_matches =
                !query.terms.is_empty() && query.terms.iter().all(|term| name.contains(term));
            let mut lines = Vec::new();
            let mut line_count = 0;
            for (line_ix, line) in note.text.lines().enumerate() {
                let Some(ranges) = match_ranges(line, &highlights) else {
                    continue;
                };
                line_count += 1;
                if lines.len() < max_lines_per_note {
                    lines.push(snippet(line, line_ix, ranges));
                }
            }
            FileMatches {
                path: note.path.clone(),
                name_matches,
                lines,
                line_count,
            }
        })
        .collect();
    results.sort_by(|a, b| {
        (Reverse(a.name_matches), Reverse(a.line_count), &a.path).cmp(&(
            Reverse(b.name_matches),
            Reverse(b.line_count),
            &b.path,
        ))
    });
    results
}

/// The merged byte ranges of `terms` in `line`, or `None` when nothing
/// matches. Lowercasing can change byte lengths for a few scripts; such lines
/// still match but carry no ranges rather than wrong ones.
fn match_ranges(line: &str, terms: &[String]) -> Option<Vec<Range<usize>>> {
    let lower = line.to_lowercase();
    let mut ranges: Vec<Range<usize>> = terms
        .iter()
        .flat_map(|term| {
            lower
                .match_indices(term.as_str())
                .map(|(start, found)| start..start + found.len())
        })
        .collect();
    if ranges.is_empty() {
        return None;
    }
    if lower.len() != line.len() {
        return Some(Vec::new());
    }
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    Some(merged)
}

fn snippet(line: &str, line_ix: usize, ranges: Vec<Range<usize>>) -> LineMatch {
    const MAX_CHARS: usize = 160;
    const CONTEXT_BEFORE: usize = 50;

    if line.chars().count() <= MAX_CHARS {
        return LineMatch {
            line: line_ix,
            text: line.to_string(),
            ranges,
        };
    }

    let first = ranges.first().map_or(0, |range| range.start);
    let mut start = first.saturating_sub(CONTEXT_BEFORE);
    while !line.is_char_boundary(start) {
        start -= 1;
    }
    let end = line[start..]
        .char_indices()
        .nth(MAX_CHARS)
        .map_or(line.len(), |(ix, _)| start + ix);

    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < line.len() { "…" } else { "" };
    let shift = prefix.len();
    let ranges = ranges
        .into_iter()
        .filter(|range| range.start >= start && range.end <= end)
        .map(|range| range.start - start + shift..range.end - start + shift)
        .collect();
    LineMatch {
        line: line_ix,
        text: format!("{prefix}{}{suffix}", &line[start..end]),
        ranges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notes() -> Vec<IndexedNote> {
        [
            (
                "Rust notes.md",
                "Ownership and borrowing.\nThe borrow checker.\n#lang/rust",
            ),
            ("Journal/2026-11-13.md", "Learned about rust today. #daily"),
            ("Recipes.md", "Bread\nBorrow a cup of sugar"),
        ]
        .into_iter()
        .map(|(path, text)| IndexedNote::new(VaultPath::new(path), text, None))
        .collect()
    }

    #[test]
    fn parses_queries() {
        let query = SearchQuery::parse(r#"  borrow "cup of"  tag:#daily path:journal #x "#);
        assert_eq!(query.terms, vec!["borrow", "cup of"]);
        assert_eq!(query.tags, vec!["daily", "x"]);
        assert_eq!(query.paths, vec!["journal"]);
        assert!(SearchQuery::parse("   ").is_empty());
    }

    #[test]
    fn ranks_name_matches_first_and_highlights_lines() {
        let notes = notes();
        let results = search(&notes, &SearchQuery::parse("rust"), 10);
        let paths: Vec<_> = results.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, ["Rust notes.md", "Journal/2026-11-13.md"]);
        assert!(results[0].name_matches);
        assert_eq!(results[1].lines[0].ranges, vec![14..18]);

        let borrow = search(&notes, &SearchQuery::parse("BORROW"), 10);
        assert_eq!(borrow[0].path.as_str(), "Rust notes.md");
        assert_eq!(borrow[0].line_count, 2);
    }

    #[test]
    fn filters_by_tag_and_path() {
        let notes = notes();
        let tagged = search(&notes, &SearchQuery::parse("#lang"), 10);
        assert_eq!(tagged.len(), 1);
        assert_eq!(tagged[0].lines[0].text, "#lang/rust");

        let in_journal = search(&notes, &SearchQuery::parse("path:journal rust"), 10);
        assert_eq!(in_journal.len(), 1);
        assert_eq!(in_journal[0].path.as_str(), "Journal/2026-11-13.md");
    }

    #[test]
    fn shortens_long_lines_around_the_match() {
        let line = format!("{}needle{}", "a".repeat(300), "b".repeat(300));
        let ranges = match_ranges(&line, &["needle".to_string()]).unwrap();
        let snippet = snippet(&line, 0, ranges);
        assert!(snippet.text.starts_with('…') && snippet.text.ends_with('…'));
        let range = snippet.ranges[0].clone();
        assert_eq!(&snippet.text[range], "needle");
    }
}
