use std::collections::HashSet;

/// What Notia knows about a note's content: its outline, outgoing links, tags
/// and size. Produced by [`parse_note`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoteMeta {
    /// The text of the first level-one heading.
    pub title: Option<String>,
    pub headings: Vec<Heading>,
    pub links: Vec<Link>,
    /// Tags without the leading `#`, in order of first appearance and
    /// deduplicated case-insensitively.
    pub tags: Vec<String>,
    /// Alternative names from the `aliases` frontmatter field.
    pub aliases: Vec<String>,
    /// Whether AI tools may read and write this note.
    ///
    /// Locked unless YAML frontmatter contains `unlocked: true`. Any other
    /// value, including a missing field, stays locked.
    pub unlocked: bool,
    pub word_count: usize,
    pub char_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    pub level: u8,
    /// The heading text with inline Markdown removed.
    pub text: String,
    /// The 0-based line the heading is on.
    pub line: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    /// `[[Target#Heading|Alias]]`
    Wiki,
    /// `[Text](Target.md#Heading)`
    Markdown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// The note the link points at, as written: `Note`, `Folder/Note.md`.
    pub target: String,
    /// The heading or block reference after `#`.
    pub subpath: Option<String>,
    /// The wikilink alias or the Markdown link text.
    pub display: Option<String>,
    pub kind: LinkKind,
    /// Whether the link embeds its target (`![[…]]`, `![](…)`).
    pub embed: bool,
    /// The 0-based line the link is on.
    pub line: usize,
}

/// Parses the metadata of a Markdown note.
///
/// This is a line scanner, not a full CommonMark parser: it understands
/// frontmatter, fenced code blocks, ATX headings, inline code spans,
/// wikilinks, Markdown links and tags, which is what the index needs.
/// Frontmatter `unlocked: true` sets [`NoteMeta::unlocked`]. Anything else
/// leaves the note locked.
pub fn parse_note(text: &str) -> NoteMeta {
    let mut parser = Parser::default();
    let lines: Vec<&str> = text.lines().collect();
    let body_start = parser.frontmatter(&lines);

    let mut fence: Option<Fence> = None;
    for (line_ix, line) in lines.iter().enumerate().skip(body_start) {
        parser.meta.word_count += count_words(line);
        parser.meta.char_count += line.chars().count();

        let trimmed = line.trim_start();
        if let Some(open) = fence {
            if open.is_closed_by(trimmed) {
                fence = None;
            }
            continue;
        }
        let indent = line.len() - trimmed.len();
        if indent < 4 {
            if let Some(open) = Fence::open(trimmed) {
                fence = Some(open);
                continue;
            }
            if let Some((level, heading)) = atx_heading(trimmed) {
                if level == 1 && parser.meta.title.is_none() {
                    parser.meta.title = Some(heading.clone());
                }
                parser.meta.headings.push(Heading {
                    level,
                    text: heading,
                    line: line_ix,
                });
            }
        }
        parser.inline(line, line_ix);
    }
    parser.meta
}

#[derive(Default)]
struct Parser {
    meta: NoteMeta,
    seen_tags: HashSet<String>,
}

impl Parser {
    /// Reads `tags` and `aliases` from YAML frontmatter and returns the line
    /// the body starts on.
    fn frontmatter(&mut self, lines: &[&str]) -> usize {
        if lines.first().map(|line| line.trim_end()) != Some("---") {
            return 0;
        }
        let Some(close) = lines
            .iter()
            .skip(1)
            .position(|line| matches!(line.trim_end(), "---" | "..."))
            .map(|ix| ix + 1)
        else {
            return 0;
        };

        let mut key = String::new();
        for line in &lines[1..close] {
            let trimmed = line.trim_start();
            if let Some(item) = trimmed.strip_prefix("- ") {
                self.frontmatter_value(&key, item);
            } else if !line.starts_with([' ', '\t'])
                && let Some((name, value)) = line.split_once(':')
            {
                key = name.trim().to_lowercase();
                let value = value.trim();
                let value = value
                    .strip_prefix('[')
                    .and_then(|v| v.strip_suffix(']'))
                    .unwrap_or(value);
                for item in value.split(',') {
                    self.frontmatter_value(&key, item);
                }
            }
        }
        close + 1
    }

    fn frontmatter_value(&mut self, key: &str, raw: &str) {
        let value = raw.trim().trim_matches(['"', '\'']).trim();
        if value.is_empty() {
            return;
        }
        match key {
            "tags" | "tag" => {
                for tag in value.split_whitespace() {
                    self.tag(tag.trim_start_matches('#'));
                }
            }
            "aliases" | "alias" => self.meta.aliases.push(value.to_string()),
            "unlocked" => self.meta.unlocked = value.eq_ignore_ascii_case("true"),
            _ => {}
        }
    }

    fn tag(&mut self, tag: &str) {
        if !tag.is_empty() && self.seen_tags.insert(tag.to_lowercase()) {
            self.meta.tags.push(tag.to_string());
        }
    }

    fn inline(&mut self, line: &str, line_ix: usize) {
        let bytes = line.as_bytes();
        let mut ix = 0;
        while ix < bytes.len() {
            let rest = &line[ix..];
            let next = match bytes[ix] {
                b'`' => Some(code_span_end(line, ix)),
                b'\\' => Some(ix + 1 + rest[1..].chars().next().map_or(0, char::len_utf8)),
                b'!' if rest.starts_with("![[") => self.wikilink(line, ix + 1, line_ix, true),
                b'!' if rest.starts_with("![") => self.markdown_link(line, ix + 1, line_ix, true),
                b'[' if rest.starts_with("[[") => self.wikilink(line, ix, line_ix, false),
                b'[' => self.markdown_link(line, ix, line_ix, false),
                b'#' if starts_tag(line, ix) => tag_len(&rest[1..]).map(|len| {
                    self.tag(&rest[1..1 + len]);
                    ix + 1 + len
                }),
                _ => None,
            };
            ix = next.unwrap_or_else(|| ix + rest.chars().next().map_or(1, char::len_utf8));
        }
    }

    /// Parses `[[target#subpath|display]]` starting at the first `[`.
    fn wikilink(&mut self, line: &str, start: usize, line_ix: usize, embed: bool) -> Option<usize> {
        let inner_start = start + 2;
        let close = inner_start + line[inner_start..].find("]]")?;
        let inner = &line[inner_start..close];
        if inner.trim().is_empty() || inner.contains('[') {
            return None;
        }
        let (target, display) = match inner.split_once('|') {
            Some((target, display)) => (target, Some(display.trim().to_string())),
            None => (inner, None),
        };
        let (target, subpath) = split_subpath(target);
        self.meta.links.push(Link {
            target: target.trim().to_string(),
            subpath,
            display,
            kind: LinkKind::Wiki,
            embed,
            line: line_ix,
        });
        Some(close + 2)
    }

    /// Parses `[text](destination)` starting at `[`. External URLs are
    /// consumed without being recorded so that their `#` is not a tag.
    fn markdown_link(
        &mut self,
        line: &str,
        start: usize,
        line_ix: usize,
        embed: bool,
    ) -> Option<usize> {
        let bytes = line.as_bytes();
        let mut depth = 0usize;
        let mut ix = start;
        let text_end = loop {
            match *bytes.get(ix)? {
                b'[' => depth += 1,
                b']' => {
                    depth -= 1;
                    if depth == 0 {
                        break ix;
                    }
                }
                b'\\' => ix += 1,
                _ => {}
            }
            ix += 1;
        };
        if bytes.get(text_end + 1) != Some(&b'(') {
            return None;
        }
        let dest_start = text_end + 2;
        let dest_end = dest_start + line[dest_start..].find(')')?;
        let end = dest_end + 1;

        let raw = line[dest_start..dest_end].trim();
        let destination = match raw.strip_prefix('<').and_then(|d| d.strip_suffix('>')) {
            Some(bracketed) => bracketed,
            None => raw.split_whitespace().next().unwrap_or_default(),
        };
        if destination.is_empty() || is_external(destination) {
            return Some(end);
        }
        let decoded = percent_decode(destination);
        let (target, subpath) = split_subpath(&decoded);
        let target = target.trim();
        let has_other_extension = target
            .rsplit('/')
            .next()
            .and_then(|name| name.rsplit_once('.'))
            .is_some_and(|(_, ext)| !ext.eq_ignore_ascii_case("md"));
        if target.is_empty() || has_other_extension {
            return Some(end);
        }
        self.meta.links.push(Link {
            target: target.to_string(),
            subpath,
            display: Some(plain_inline_text(&line[start + 1..text_end])),
            kind: LinkKind::Markdown,
            embed,
            line: line_ix,
        });
        Some(end)
    }
}

#[derive(Clone, Copy)]
struct Fence {
    marker: u8,
    len: usize,
}

impl Fence {
    fn open(line: &str) -> Option<Fence> {
        let marker = *line.as_bytes().first()?;
        if marker != b'`' && marker != b'~' {
            return None;
        }
        let len = line.bytes().take_while(|&b| b == marker).count();
        if len < 3 || (marker == b'`' && line[len..].contains('`')) {
            return None;
        }
        Some(Fence { marker, len })
    }

    fn is_closed_by(self, line: &str) -> bool {
        let len = line.bytes().take_while(|&b| b == self.marker).count();
        len >= self.len && line[len..].trim().is_empty()
    }
}

fn atx_heading(line: &str) -> Option<(u8, String)> {
    let level = line.bytes().take_while(|&b| b == b'#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &line[level..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let mut content = rest.trim();
    let without_closing = content.trim_end_matches('#');
    if without_closing.len() < content.len()
        && (without_closing.is_empty() || without_closing.ends_with([' ', '\t']))
    {
        content = without_closing.trim_end();
    }
    Some((level as u8, plain_inline_text(content)))
}

/// The byte index after the code span opening at `start`, or after the
/// backtick run when it is never closed.
fn code_span_end(line: &str, start: usize) -> usize {
    let run = line[start..].bytes().take_while(|&b| b == b'`').count();
    let mut search = start + run;
    while let Some(found) = line[search..].find('`') {
        let at = search + found;
        let len = line[at..].bytes().take_while(|&b| b == b'`').count();
        if len == run {
            return at + len;
        }
        search = at + len;
    }
    start + run
}

/// A `#` starts a tag at the start of a line or after whitespace or an
/// opening bracket, never inside a word or URL.
fn starts_tag(line: &str, ix: usize) -> bool {
    line[..ix]
        .chars()
        .next_back()
        .is_none_or(|prev| prev.is_whitespace() || matches!(prev, '(' | ',' | ';'))
}

/// The byte length of the tag body at the start of `rest`. Tags need at least
/// one non-digit, so `#123` is not a tag.
fn tag_len(rest: &str) -> Option<usize> {
    let len: usize = rest
        .chars()
        .take_while(|&ch| ch.is_alphanumeric() || matches!(ch, '_' | '-' | '/'))
        .map(char::len_utf8)
        .sum();
    let body = rest[..len].trim_end_matches('/');
    if body.is_empty() || body.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some(body.len())
}

fn split_subpath(target: &str) -> (&str, Option<String>) {
    match target.split_once('#') {
        Some((target, subpath)) => (
            target,
            Some(subpath.trim().to_string()).filter(|s| !s.is_empty()),
        ),
        None => (target, None),
    }
}

fn is_external(destination: &str) -> bool {
    destination.contains("://")
        || ["mailto:", "tel:", "data:", "obsidian:"]
            .iter()
            .any(|scheme| destination.starts_with(scheme))
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut ix = 0;
    while ix < bytes.len() {
        if bytes[ix] == b'%'
            && let Some(byte) = input
                .get(ix + 1..ix + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            ix += 3;
        } else {
            out.push(bytes[ix]);
            ix += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_string())
}

/// Removes emphasis, code and link syntax so a heading reads as plain text.
fn plain_inline_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(ch) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("[[")
            && let Some(end) = after.find("]]")
        {
            let inner = &after[..end];
            let shown = match inner.split_once('|') {
                Some((_, alias)) => alias,
                None => inner.split('#').next().unwrap_or(inner),
            };
            out.push_str(shown);
            rest = &after[end + 2..];
            continue;
        }
        if ch == '['
            && let Some(text_end) = rest.find("](")
            && let Some(close) = rest[text_end..].find(')')
        {
            out.push_str(&rest[1..text_end]);
            rest = &rest[text_end + close + 1..];
            continue;
        }
        if let Some(after) = ["**", "__", "==", "~~"]
            .iter()
            .find_map(|marker| rest.strip_prefix(marker))
        {
            rest = after;
            continue;
        }
        if !matches!(ch, '*' | '`') {
            out.push(ch);
        }
        rest = &rest[ch.len_utf8()..];
    }
    out.trim().to_string()
}

/// Counts words the way a writer would: runs of letters and digits, with
/// apostrophes and hyphens inside a word, and each CJK character as a word.
fn count_words(line: &str) -> usize {
    let mut count = 0;
    for token in line.split_whitespace() {
        let mut in_word = false;
        for ch in token.chars() {
            if is_cjk(ch) {
                count += 1;
                in_word = false;
            } else if ch.is_alphanumeric() {
                if !in_word {
                    count += 1;
                    in_word = true;
                }
            } else if !(in_word && matches!(ch, '\'' | '’' | '-')) {
                in_word = false;
            }
        }
    }
    count
}

fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x3040..=0x30FF // Hiragana, Katakana
        | 0x3400..=0x4DBF // CJK Extension A
        | 0x4E00..=0x9FFF // CJK Unified Ideographs
        | 0xAC00..=0xD7AF // Hangul
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
        | 0x20000..=0x2FA1F)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_headings_and_title() {
        let meta =
            parse_note("# Hello **World**\n\nText\n## Sub [[Note|Alias]] ##\n####### not\n#nope");
        assert_eq!(meta.title.as_deref(), Some("Hello World"));
        assert_eq!(
            meta.headings,
            vec![
                Heading {
                    level: 1,
                    text: "Hello World".into(),
                    line: 0
                },
                Heading {
                    level: 2,
                    text: "Sub Alias".into(),
                    line: 3
                },
            ]
        );
    }

    #[test]
    fn ignores_code_blocks_and_spans() {
        let meta = parse_note(
            "```rust\n# not heading [[NotLink]] #nottag\n```\nUse `[[x]]` and `#y` here #real",
        );
        assert!(meta.headings.is_empty());
        assert!(meta.links.is_empty());
        assert_eq!(meta.tags, vec!["real"]);
    }

    #[test]
    fn parses_wikilinks() {
        let meta = parse_note("See [[Folder/Note#Part|shown]] and ![[Image.png]] and [[ Plain ]].");
        let links: Vec<_> = meta
            .links
            .iter()
            .map(|l| {
                (
                    l.target.as_str(),
                    l.subpath.as_deref(),
                    l.display.as_deref(),
                    l.embed,
                )
            })
            .collect();
        assert_eq!(
            links,
            vec![
                ("Folder/Note", Some("Part"), Some("shown"), false),
                ("Image.png", None, None, true),
                ("Plain", None, None, false),
            ]
        );
    }

    #[test]
    fn parses_markdown_links_but_skips_urls_and_files() {
        let meta = parse_note(
            "[a](Other%20Note.md#Top) [b](https://x.com/#frag) [c](<With Space.md>) [d](pic.png) [e](#local)",
        );
        let targets: Vec<_> = meta
            .links
            .iter()
            .map(|l| (l.target.as_str(), l.subpath.as_deref()))
            .collect();
        assert_eq!(
            targets,
            vec![("Other Note.md", Some("Top")), ("With Space.md", None)]
        );
        assert!(meta.tags.is_empty());
    }

    #[test]
    fn finds_tags() {
        let meta = parse_note("#start middle#not (#paren) #2024 #year2024 #a/b/ #Start #ünï");
        assert_eq!(meta.tags, vec!["start", "paren", "year2024", "a/b", "ünï"]);
    }

    #[test]
    fn reads_frontmatter() {
        let meta = parse_note(
            "---\ntags: [project, \"#idea\"]\naliases:\n  - Plan\n  - 'Road map'\n---\n# Body #inline",
        );
        assert_eq!(meta.tags, vec!["project", "idea", "inline"]);
        assert_eq!(meta.aliases, vec!["Plan", "Road map"]);
        assert_eq!(meta.headings[0].line, 6);
        assert_eq!(meta.word_count, 2);
    }

    #[test]
    fn notes_stay_locked_unless_frontmatter_opts_in() {
        assert!(!parse_note("# Hello\nunlocked: true").unlocked);
        assert!(!parse_note("---\nunlocked: false\n---\n").unlocked);
        assert!(!parse_note("---\nunlocked: \"yes\"\n---\n").unlocked);
        assert!(parse_note("---\nunlocked: true\n---\n# Hi").unlocked);
        assert!(parse_note("---\nUnlocked: \"True\"\n---\n").unlocked);
        let meta = parse_note("---\nunlocked: true\n---\n# Hi");
        assert!(meta.tags.is_empty());
        assert!(meta.unlocked);
    }

    #[test]
    fn counts_words() {
        assert_eq!(count_words("Hello, world! It's a well-known fact."), 6);
        assert_eq!(count_words("- [ ] **todo** item"), 2);
        assert_eq!(count_words("中文字"), 3);
        assert_eq!(count_words("--- ### ***"), 0);
    }
}
