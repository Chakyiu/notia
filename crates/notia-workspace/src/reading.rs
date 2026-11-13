//! Turns a note into Markdown the reading view can render.
//!
//! The renderer knows CommonMark and GFM, not Obsidian's extensions, so
//! wikilinks and tags become links with `notia://` URLs, `==highlights==`
//! become `<mark>`, and a callout title moves below its `> [!type]` marker.
//! Code spans and fenced code stay untouched.

/// The scheme of in-app link URLs.
const SCHEME: &str = "notia://";

/// Callout types the reading view draws as surfaces.
pub const CALLOUTS: &[&str] = &[
    "note",
    "abstract",
    "summary",
    "tldr",
    "info",
    "todo",
    "tip",
    "hint",
    "important",
    "success",
    "check",
    "done",
    "question",
    "help",
    "faq",
    "warning",
    "caution",
    "attention",
    "failure",
    "fail",
    "missing",
    "danger",
    "error",
    "bug",
    "example",
    "quote",
    "cite",
];

/// Markdown for the reading view, with where each source line starts in it.
pub struct ReadingMarkdown {
    pub text: String,
    line_starts: Vec<usize>,
}

impl ReadingMarkdown {
    /// The byte range in [`Self::text`] that renders the 0-based source line.
    pub fn line_range(&self, line: usize) -> Option<std::ops::Range<usize>> {
        let start = *self.line_starts.get(line)?;
        let end = self
            .line_starts
            .get(line + 1)
            .map_or(self.text.len(), |next| next.saturating_sub(1).max(start));
        Some(start..end)
    }
}

/// Where an in-app link points.
#[derive(Debug, PartialEq, Eq)]
pub enum NoteUrl {
    /// A link target, possibly with a `#heading`, as written in the note.
    Note(String),
    Tag(String),
}

pub fn parse_note_url(url: &str) -> Option<NoteUrl> {
    let rest = url.strip_prefix(SCHEME)?;
    if let Some(target) = rest.strip_prefix("note/") {
        Some(NoteUrl::Note(percent_decode(target)))
    } else {
        rest.strip_prefix("tag/")
            .map(|tag| NoteUrl::Tag(percent_decode(tag)))
    }
}

pub fn to_reading_markdown(source: &str) -> ReadingMarkdown {
    let mut text = String::with_capacity(source.len() + source.len() / 8);
    let mut line_starts = Vec::new();
    let mut fence: Option<(char, usize)> = None;

    for line in source.split('\n') {
        line_starts.push(text.len());
        let trimmed = line.trim_start();
        if let Some((marker, len)) = fence {
            let run = trimmed.chars().take_while(|&ch| ch == marker).count();
            if run >= len && trimmed[run..].trim().is_empty() {
                fence = None;
            }
            text.push_str(line);
        } else if let Some(open) = fence_open(trimmed) {
            fence = Some(open);
            text.push_str(line);
        } else if let Some((marker, title)) = callout_title(line) {
            text.push_str(&marker);
            if !title.is_empty() {
                // Emphasis inside the title would close the bold early.
                let bold = !title.contains(['*', '_']);
                text.push_str(if bold { "\n> **" } else { "\n> " });
                transform_inline(title, &mut text);
                if bold {
                    text.push_str("**");
                }
                // Ends the title's paragraph so the body starts its own.
                text.push_str("\n>");
            }
        } else {
            transform_inline(line, &mut text);
        }
        text.push('\n');
    }
    text.pop();
    ReadingMarkdown { text, line_starts }
}

fn fence_open(line: &str) -> Option<(char, usize)> {
    let marker = line.chars().next().filter(|ch| *ch == '`' || *ch == '~')?;
    let len = line.chars().take_while(|&ch| ch == marker).count();
    (len >= 3 && !(marker == '`' && line[len..].contains('`'))).then_some((marker, len))
}

/// Splits `> [!type]± Title` into the bare marker and the title, for a
/// registered callout type.
fn callout_title(line: &str) -> Option<(String, &str)> {
    let rest = line.strip_prefix('>')?.trim_start().strip_prefix("[!")?;
    let close = rest.find(']')?;
    let kind = &rest[..close];
    if !CALLOUTS.contains(&kind.to_ascii_lowercase().as_str()) {
        return None;
    }
    let title = rest[close + 1..].trim_start_matches(['+', '-']).trim();
    Some((format!("> [!{kind}]"), title))
}

fn transform_inline(line: &str, out: &mut String) {
    let mut rest = line;
    let mut previous: Option<char> = None;
    while let Some(ch) = rest.chars().next() {
        let consumed = if ch == '`' {
            let run = rest.chars().take_while(|&c| c == '`').count();
            let closing = "`".repeat(run);
            match rest[run..].find(&closing) {
                Some(end) => {
                    let len = run + end + run;
                    out.push_str(&rest[..len]);
                    len
                }
                None => {
                    out.push_str(&rest[..run]);
                    run
                }
            }
        } else if ch == '\\' {
            let len = 1 + rest[1..].chars().next().map_or(0, char::len_utf8);
            out.push_str(&rest[..len]);
            len
        } else if let Some(len) = wikilink(rest, out) {
            len
        } else if ch == '#'
            && previous.is_none_or(|p| p.is_whitespace() || matches!(p, '(' | ',' | ';'))
        {
            match tag_len(&rest[1..]) {
                Some(len) => {
                    let tag = &rest[1..1 + len];
                    out.push_str(&format!("[#{tag}]({SCHEME}tag/{})", percent_encode(tag)));
                    1 + len
                }
                None => {
                    out.push('#');
                    1
                }
            }
        } else if let Some(inner) = rest.strip_prefix("==")
            && let Some(end) = inner.find("==")
            && end > 0
        {
            out.push_str("<mark>");
            transform_inline(&inner[..end], out);
            out.push_str("</mark>");
            end + 4
        } else {
            out.push(ch);
            ch.len_utf8()
        };
        previous = rest[..consumed].chars().next_back();
        rest = &rest[consumed..];
    }
}

/// Rewrites a wikilink or embed at the start of `rest`, returning how many
/// bytes it spans.
fn wikilink(rest: &str, out: &mut String) -> Option<usize> {
    let (embed, inner_start) = if rest.starts_with("![[") {
        (true, 3)
    } else if rest.starts_with("[[") {
        (false, 2)
    } else {
        return None;
    };
    let close = inner_start + rest[inner_start..].find("]]")?;
    let inner = &rest[inner_start..close];
    if inner.trim().is_empty() || inner.contains('[') {
        return None;
    }
    let (target, alias) = match inner.split_once('|') {
        Some((target, alias)) => (target.trim(), Some(alias.trim())),
        None => (inner.trim(), None),
    };
    let label = alias
        .map(str::to_string)
        .unwrap_or_else(|| match target.split_once('#') {
            Some(("", heading)) => heading.to_string(),
            Some((note, heading)) => format!("{note} › {heading}"),
            None => target.to_string(),
        });
    let label = label.replace(['[', ']'], "");
    let prefix = if embed { "↪ " } else { "" };
    out.push_str(&format!(
        "[{prefix}{label}]({SCHEME}note/{})",
        percent_encode(target)
    ));
    Some(close + 2)
}

fn tag_len(rest: &str) -> Option<usize> {
    let len: usize = rest
        .chars()
        .take_while(|&ch| ch.is_alphanumeric() || matches!(ch, '_' | '-' | '/'))
        .map(char::len_utf8)
        .sum();
    let body = rest[..len].trim_end_matches('/');
    (!body.is_empty() && !body.chars().all(|ch| ch.is_ascii_digit())).then_some(body.len())
}

fn percent_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut ix = 0;
    while ix < bytes.len() {
        if bytes[ix] == b'%'
            && let Some(byte) = text
                .get(ix + 1..ix + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            decoded.push(byte);
            ix += 3;
        } else {
            decoded.push(bytes[ix]);
            ix += 1;
        }
    }
    String::from_utf8(decoded).unwrap_or_else(|_| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_wikilinks_tags_and_highlights() {
        let reading = to_reading_markdown(
            "See [[My Note#Part|this]], [[Plain]] and ![[pic.png]] #todo ==hot==",
        );
        assert_eq!(
            reading.text,
            "See [this](notia://note/My%20Note%23Part), [Plain](notia://note/Plain) and \
             [↪ pic.png](notia://note/pic.png) [#todo](notia://tag/todo) <mark>hot</mark>"
        );
    }

    #[test]
    fn leaves_code_untouched() {
        let source = "`[[x]] #y`\n```\n[[z]] #w\n```\n#after";
        let reading = to_reading_markdown(source);
        assert_eq!(
            reading.text,
            "`[[x]] #y`\n```\n[[z]] #w\n```\n[#after](notia://tag/after)"
        );
    }

    #[test]
    fn moves_callout_titles_and_maps_lines() {
        let reading = to_reading_markdown(
            "intro\n> [!TIP]- Read this\n> body\n> [!custom] stays\n> [!note] *Mine*",
        );
        assert_eq!(
            reading.text,
            "intro\n> [!TIP]\n> **Read this**\n>\n> body\n> [!custom] stays\n> [!note]\n> *Mine*\n>"
        );
        let range = reading.line_range(2).unwrap();
        assert_eq!(&reading.text[range], "> body");
        assert_eq!(&reading.text[reading.line_range(0).unwrap()], "intro");
    }

    #[test]
    fn round_trips_note_urls() {
        let mut out = String::new();
        wikilink("[[Folder/Ünï Note#Head]]", &mut out);
        let url = out.split("](").nth(1).unwrap().trim_end_matches(')');
        assert_eq!(
            parse_note_url(url),
            Some(NoteUrl::Note("Folder/Ünï Note#Head".into()))
        );
        assert_eq!(
            parse_note_url("notia://tag/a%2Fb"),
            Some(NoteUrl::Tag("a/b".into()))
        );
        assert_eq!(parse_note_url("https://example.com"), None);
    }
}
