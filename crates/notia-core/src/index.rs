use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    sync::Arc,
    time::SystemTime,
};

use crate::{FileTree, Link, NoteMeta, Vault, VaultPath, parse_note};

/// A note's content and parsed metadata, as last read from disk.
#[derive(Clone, Debug)]
pub struct IndexedNote {
    pub path: VaultPath,
    pub text: Arc<str>,
    pub meta: NoteMeta,
    pub modified: Option<SystemTime>,
}

impl IndexedNote {
    pub fn new(path: VaultPath, text: impl Into<Arc<str>>, modified: Option<SystemTime>) -> Self {
        let text = text.into();
        let meta = parse_note(&text);
        Self {
            path,
            text,
            meta,
            modified,
        }
    }

    /// The name a note is shown and linked by: its file name without `.md`.
    pub fn title(&self) -> &str {
        self.path.file_stem()
    }

    /// The trimmed text of a 0-based line, shortened for display.
    pub fn line_text(&self, line: usize) -> String {
        let text = self.text.lines().nth(line).unwrap_or_default().trim();
        truncate(text, 200)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backlink {
    pub source: VaultPath,
    pub line: usize,
    pub context: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagCount {
    pub tag: String,
    pub count: usize,
}

/// Notes that changed on disk since an index snapshot, read off the main
/// thread by [`VaultIndex::read_changes`] and applied with
/// [`VaultIndex::apply`].
#[derive(Debug, Default)]
pub struct IndexChanges {
    updated: Vec<IndexedNote>,
    removed: Vec<VaultPath>,
}

impl IndexChanges {
    pub fn is_empty(&self) -> bool {
        self.updated.is_empty() && self.removed.is_empty()
    }

    pub fn updated(&self) -> impl Iterator<Item = &VaultPath> {
        self.updated.iter().map(|note| &note.path)
    }

    pub fn removed(&self) -> &[VaultPath] {
        &self.removed
    }
}

/// Every note of a vault with its links and tags, kept in memory so that
/// backlinks, tags and search never touch the disk.
#[derive(Clone, Debug, Default)]
pub struct VaultIndex {
    notes: HashMap<VaultPath, IndexedNote>,
    /// Lowercased names and aliases to the notes carrying them.
    by_name: HashMap<String, Vec<VaultPath>>,
}

impl VaultIndex {
    pub fn len(&self) -> usize {
        self.notes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    pub fn get(&self, path: &VaultPath) -> Option<&IndexedNote> {
        self.notes.get(path)
    }

    pub fn contains(&self, path: &VaultPath) -> bool {
        self.notes.contains_key(path)
    }

    pub fn notes(&self) -> impl Iterator<Item = &IndexedNote> {
        self.notes.values()
    }

    pub fn insert(&mut self, note: IndexedNote) {
        self.remove(&note.path);
        for name in Self::names(&note) {
            self.by_name
                .entry(name)
                .or_default()
                .push(note.path.clone());
        }
        self.notes.insert(note.path.clone(), note);
    }

    pub fn remove(&mut self, path: &VaultPath) -> Option<IndexedNote> {
        let note = self.notes.remove(path)?;
        for name in Self::names(&note) {
            if let Some(paths) = self.by_name.get_mut(&name) {
                paths.retain(|candidate| candidate != path);
                if paths.is_empty() {
                    self.by_name.remove(&name);
                }
            }
        }
        Some(note)
    }

    fn names(note: &IndexedNote) -> Vec<String> {
        let mut names = vec![note.title().to_lowercase()];
        for alias in &note.meta.aliases {
            let alias = alias.to_lowercase();
            if !names.contains(&alias) {
                names.push(alias);
            }
        }
        names
    }

    /// The modification time the index holds for every note.
    pub fn snapshot(&self) -> HashMap<VaultPath, Option<SystemTime>> {
        self.notes
            .iter()
            .map(|(path, note)| (path.clone(), note.modified))
            .collect()
    }

    /// Reads the notes of `tree` that are new or modified compared with
    /// `known`, and lists the known notes that no longer exist. Does blocking
    /// I/O; run it on a background thread.
    pub fn read_changes(
        vault: &Vault,
        tree: &FileTree,
        known: &HashMap<VaultPath, Option<SystemTime>>,
    ) -> IndexChanges {
        let notes = tree.notes();
        let mut changes = IndexChanges::default();
        for path in &notes {
            let modified = vault.modified(path);
            if known
                .get(path)
                .is_some_and(|&known| known.is_some() && known == modified)
            {
                continue;
            }
            if let Ok(text) = vault.read_note(path) {
                changes
                    .updated
                    .push(IndexedNote::new(path.clone(), text, modified));
            }
        }
        let present: HashSet<&VaultPath> = notes.iter().collect();
        changes.removed = known
            .keys()
            .filter(|path| !present.contains(path))
            .cloned()
            .collect();
        changes
    }

    pub fn apply(&mut self, changes: IndexChanges) {
        for path in &changes.removed {
            self.remove(path);
        }
        for note in changes.updated {
            self.insert(note);
        }
    }

    /// Finds the note a link target refers to, the way Obsidian does: a path
    /// relative to the linking note, then a path from the vault root, then
    /// any note with that name or alias, preferring the linking note's folder
    /// and then the shortest path.
    pub fn resolve(&self, target: &str, from: Option<&VaultPath>) -> Option<VaultPath> {
        let target = target.trim();
        if target.is_empty() {
            return from.cloned();
        }

        let folder = from
            .and_then(VaultPath::parent)
            .unwrap_or_else(VaultPath::root);
        if let Some(relative) = join_relative(&folder, target)
            && let Some(found) = self.lookup(&relative)
        {
            return Some(found);
        }
        if let Some(found) = self.lookup(&VaultPath::new(target)) {
            return Some(found);
        }

        let without_extension = strip_markdown_extension(target).to_lowercase();
        let name = without_extension.rsplit('/').next().unwrap_or_default();
        let candidates = self.by_name.get(name)?;
        let mut matching: Vec<&VaultPath> = if without_extension.contains('/') {
            let suffix = format!("/{without_extension}");
            candidates
                .iter()
                .filter(|path| {
                    let path = path.without_markdown_extension().to_lowercase();
                    path == without_extension || path.ends_with(&suffix)
                })
                .collect()
        } else {
            candidates.iter().collect()
        };
        matching.sort_by_key(|path| {
            (
                path.parent().as_ref() != Some(&folder),
                path.as_str().len(),
                path.as_str(),
            )
        });
        matching.first().map(|path| (*path).clone())
    }

    fn lookup(&self, path: &VaultPath) -> Option<VaultPath> {
        if self.notes.contains_key(path) {
            return Some(path.clone());
        }
        let with_extension = VaultPath::new(format!("{path}.md"));
        self.notes
            .contains_key(&with_extension)
            .then_some(with_extension)
    }

    /// The notes linking to `path`, ordered by note and line.
    pub fn backlinks(&self, path: &VaultPath) -> Vec<Backlink> {
        let mut backlinks: Vec<Backlink> = self
            .notes
            .values()
            .filter(|note| &note.path != path)
            .flat_map(|note| {
                note.meta
                    .links
                    .iter()
                    .filter(|link| {
                        self.resolve(&link.target, Some(&note.path)).as_ref() == Some(path)
                    })
                    .map(|link| Backlink {
                        source: note.path.clone(),
                        line: link.line,
                        context: note.line_text(link.line),
                    })
            })
            .collect();
        backlinks.sort_by(|a, b| a.source.cmp(&b.source).then(a.line.cmp(&b.line)));
        backlinks.dedup_by(|a, b| a.source == b.source && a.line == b.line);
        backlinks
    }

    /// The links of `path` with the note each one resolves to.
    pub fn outgoing_links(&self, path: &VaultPath) -> Vec<(Link, Option<VaultPath>)> {
        let Some(note) = self.notes.get(path) else {
            return Vec::new();
        };
        note.meta
            .links
            .iter()
            .map(|link| (link.clone(), self.resolve(&link.target, Some(path))))
            .collect()
    }

    /// Every tag with the number of notes using it, most used first.
    pub fn tags(&self) -> Vec<TagCount> {
        let mut counts: HashMap<String, TagCount> = HashMap::new();
        for note in self.notes.values() {
            for tag in &note.meta.tags {
                let entry = counts
                    .entry(tag.to_lowercase())
                    .or_insert_with(|| TagCount {
                        tag: tag.clone(),
                        count: 0,
                    });
                entry.count += 1;
                // Notes are visited in no particular order; settle on one spelling.
                if *tag < entry.tag {
                    entry.tag = tag.clone();
                }
            }
        }
        let mut tags: Vec<TagCount> = counts.into_values().collect();
        tags.sort_by_key(|tag| (Reverse(tag.count), tag.tag.to_lowercase()));
        tags
    }

    /// Notes tagged `tag` or one of its nested tags (`tag/child`).
    pub fn notes_with_tag(&self, tag: &str) -> Vec<VaultPath> {
        let tag = tag.trim_start_matches('#');
        let mut paths: Vec<VaultPath> = self
            .notes
            .values()
            .filter(|note| {
                note.meta
                    .tags
                    .iter()
                    .any(|candidate| tag_matches(candidate, tag))
            })
            .map(|note| note.path.clone())
            .collect();
        paths.sort();
        paths
    }
}

/// Whether `tag` is `query` or nested under it, case-insensitively.
pub(crate) fn tag_matches(tag: &str, query: &str) -> bool {
    let tag = tag.to_lowercase();
    let query = query.to_lowercase();
    tag == query
        || tag
            .strip_prefix(&query)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn strip_markdown_extension(target: &str) -> &str {
    match target.len().checked_sub(3) {
        Some(ix) if target.is_char_boundary(ix) && target[ix..].eq_ignore_ascii_case(".md") => {
            &target[..ix]
        }
        _ => target,
    }
}

/// Resolves `target` against `folder`, honoring `.` and `..`. Returns `None`
/// when the target climbs above the vault root.
fn join_relative(folder: &VaultPath, target: &str) -> Option<VaultPath> {
    let mut segments: Vec<&str> = folder
        .as_str()
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            segment => segments.push(segment),
        }
    }
    Some(VaultPath::new(segments.join("/")))
}

pub(crate) fn truncate(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((ix, _)) => format!("{}…", &text[..ix]),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(notes: &[(&str, &str)]) -> VaultIndex {
        let mut index = VaultIndex::default();
        for (path, text) in notes {
            index.insert(IndexedNote::new(VaultPath::new(path), *text, None));
        }
        index
    }

    #[test]
    fn resolves_by_name_path_relative_and_alias() {
        let index = index(&[
            (
                "Home.md",
                "[[Plan]] [[Projects/Plan]] [x](./Projects/Plan.md) [[Road map]]",
            ),
            ("Plan.md", ""),
            (
                "Projects/Plan.md",
                "---\naliases: [Road map]\n---\n[[Plan]] [up](../Home.md)",
            ),
        ]);
        let home = VaultPath::new("Home.md");
        let nested = VaultPath::new("Projects/Plan.md");

        assert_eq!(
            index.resolve("Plan", Some(&home)),
            Some(VaultPath::new("Plan.md"))
        );
        assert_eq!(
            index.resolve("Projects/Plan", Some(&home)),
            Some(nested.clone())
        );
        assert_eq!(
            index.resolve("./Projects/Plan.md", Some(&home)),
            Some(nested.clone())
        );
        assert_eq!(index.resolve("road map", Some(&home)), Some(nested.clone()));
        // A same-named note in the linking note's folder wins.
        assert_eq!(index.resolve("Plan", Some(&nested)), Some(nested.clone()));
        assert_eq!(
            index.resolve("../Home.md", Some(&nested)),
            Some(home.clone())
        );
        assert_eq!(index.resolve("Missing", Some(&home)), None);
        assert_eq!(index.resolve("", Some(&home)), Some(home));
    }

    #[test]
    fn collects_backlinks_with_context() {
        let index = index(&[
            ("A.md", "intro\nsee [[C]] here\n[[C]] again"),
            ("B.md", "- [[C|the c note]]"),
            ("C.md", "[[C]] self"),
        ]);
        let backlinks = index.backlinks(&VaultPath::new("C.md"));
        let summary: Vec<_> = backlinks
            .iter()
            .map(|b| (b.source.as_str(), b.line, b.context.as_str()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("A.md", 1, "see [[C]] here"),
                ("A.md", 2, "[[C]] again"),
                ("B.md", 0, "- [[C|the c note]]")
            ]
        );
    }

    #[test]
    fn counts_tags_and_matches_nested_tags() {
        let index = index(&[("a.md", "#Work #work/meeting"), ("b.md", "#work #home")]);
        let tags: Vec<_> = index.tags().into_iter().map(|t| (t.tag, t.count)).collect();
        assert_eq!(tags[0], ("Work".to_string(), 2));
        assert_eq!(tags.len(), 3);
        assert_eq!(
            index.notes_with_tag("#work"),
            vec![VaultPath::new("a.md"), VaultPath::new("b.md")]
        );
        assert_eq!(
            index.notes_with_tag("work/meeting"),
            vec![VaultPath::new("a.md")]
        );
        assert!(index.notes_with_tag("wor").is_empty());
    }

    #[test]
    fn reindexes_only_changed_notes() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        vault.write_note(&"a.md".into(), "[[b]]").unwrap();
        vault.write_note(&"b.md".into(), "").unwrap();

        let mut index = VaultIndex::default();
        let changes = VaultIndex::read_changes(&vault, &vault.scan().unwrap(), &index.snapshot());
        assert_eq!(changes.updated().count(), 2);
        index.apply(changes);

        let unchanged = VaultIndex::read_changes(&vault, &vault.scan().unwrap(), &index.snapshot());
        assert!(unchanged.is_empty());

        std::fs::remove_file(vault.abs_path(&"b.md".into())).unwrap();
        let removed = VaultIndex::read_changes(&vault, &vault.scan().unwrap(), &index.snapshot());
        assert_eq!(removed.removed(), [VaultPath::new("b.md")]);
        index.apply(removed);
        assert_eq!(index.len(), 1);
        assert_eq!(index.resolve("b", Some(&"a.md".into())), None);
    }
}
