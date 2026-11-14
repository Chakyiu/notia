//! The text of open notes, as plugins read and change it.
//!
//! [`workspace::editor`](crate::workspace::editor) reads an open note's text
//! and selection, and [`workspace::edit`](crate::workspace::edit) changes
//! them. The editor applies an edit once the current update is over, but
//! reads include it at once, so a plugin always reads what it wrote.

use std::ops::Range;

use anyhow::ensure;
use gpui_kit::SharedString;
use notia_core::VaultPath;

use crate::ViewMode;

/// The editor of an open note. It keeps its id while the note stays open,
/// even when the note is renamed or moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EditorId(u64);

impl EditorId {
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// A selection as byte offsets into the text. `head` is the cursor, and
/// `anchor` is the end that stays put while the selection is extended. They
/// are equal when nothing is selected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditorSelection {
    pub anchor: usize,
    pub head: usize,
}

impl EditorSelection {
    pub fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head }
    }

    /// A cursor at `offset`, with nothing selected.
    pub fn cursor(offset: usize) -> Self {
        Self::new(offset, offset)
    }

    /// The selected bytes.
    pub fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Whether the cursor is at the start of the selection.
    pub fn is_reversed(&self) -> bool {
        self.head < self.anchor
    }
}

/// An open note's text and selection, including edits the editor has yet
/// to apply.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorSnapshot {
    pub id: EditorId,
    pub path: VaultPath,
    pub mode: ViewMode,
    pub text: SharedString,
    pub selection: EditorSelection,
}

/// Replaces the bytes in `range` with `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorChange {
    pub range: Range<usize>,
    pub text: String,
}

/// Changes to a note's text that one undo reverts.
///
/// The ranges of `changes` are byte offsets into the text before the edit,
/// and must not overlap. `selection` refers to the text after the edit.
/// Without one, each end of the selection stays with the text it was on: it
/// moves with the changes before it, stays in front of text inserted where
/// it is, and goes to the start of a change that replaces the text under it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditorTransaction {
    pub changes: Vec<EditorChange>,
    pub selection: Option<EditorSelection>,
}

impl EditorTransaction {
    /// Inserts `text` at `offset` and puts the cursor after it.
    pub fn insert(offset: usize, text: impl Into<String>) -> Self {
        let text = text.into();
        let end = offset + text.len();
        Self {
            changes: vec![EditorChange {
                range: offset..offset,
                text,
            }],
            selection: Some(EditorSelection::cursor(end)),
        }
    }

    /// The text and selection after the edit, or why the edit does not fit
    /// `text`.
    pub fn apply(
        &self,
        text: &str,
        selection: EditorSelection,
    ) -> anyhow::Result<(String, EditorSelection)> {
        let changes = self.sorted(text)?;
        let after = match merge(text, &changes) {
            Some(change) => {
                let mut after =
                    String::with_capacity(text.len() - change.range.len() + change.text.len());
                after.push_str(&text[..change.range.start]);
                after.push_str(&change.text);
                after.push_str(&text[change.range.end..]);
                after
            }
            None => text.to_string(),
        };
        let selection = match self.selection {
            Some(selection) => {
                check_offset(&after, selection.anchor)?;
                check_offset(&after, selection.head)?;
                selection
            }
            None => EditorSelection::new(
                map_offset(selection.anchor, &changes),
                map_offset(selection.head, &changes),
            ),
        };
        Ok((after, selection))
    }

    /// One change with the same effect as all of them, spanning the first
    /// changed byte to the last. `None` when nothing changes.
    pub fn merged(&self, text: &str) -> anyhow::Result<Option<EditorChange>> {
        Ok(merge(text, &self.sorted(text)?))
    }

    /// The changes from first to last, checked against `text`.
    fn sorted(&self, text: &str) -> anyhow::Result<Vec<&EditorChange>> {
        let mut changes: Vec<&EditorChange> = self.changes.iter().collect();
        // Stable, so insertions at one offset keep their order.
        changes.sort_by_key(|change| (change.range.start, change.range.end));
        let mut previous_end = 0;
        for change in &changes {
            let Range { start, end } = change.range;
            ensure!(
                start <= end,
                "the change at {start}..{end} ends before it starts"
            );
            check_offset(text, start)?;
            check_offset(text, end)?;
            ensure!(start >= previous_end, "two changes overlap at {start}");
            previous_end = end;
        }
        Ok(changes)
    }
}

/// Undoes or redoes edits, or focuses the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorCommand {
    Undo,
    Redo,
    /// Puts the keyboard focus in the note's text, leaving preview.
    Focus,
}

/// `changes`, sorted and checked, as one change.
fn merge(text: &str, changes: &[&EditorChange]) -> Option<EditorChange> {
    let start = changes.first()?.range.start;
    let end = changes.last()?.range.end;
    let mut replacement = String::new();
    let mut copied = start;
    for change in changes {
        replacement.push_str(&text[copied..change.range.start]);
        replacement.push_str(&change.text);
        copied = change.range.end;
    }
    Some(EditorChange {
        range: start..end,
        text: replacement,
    })
}

/// Where `offset` ends up after `changes`, sorted and checked.
fn map_offset(offset: usize, changes: &[&EditorChange]) -> usize {
    let mut shift = 0isize;
    for change in changes {
        let Range { start, end } = change.range;
        if offset <= start {
            break;
        }
        if offset < end {
            return start.saturating_add_signed(shift);
        }
        shift += change.text.len() as isize - (end - start) as isize;
    }
    offset.saturating_add_signed(shift)
}

fn check_offset(text: &str, offset: usize) -> anyhow::Result<()> {
    ensure!(
        offset <= text.len(),
        "offset {offset} is past the end of the text, which has {} bytes",
        text.len()
    );
    ensure!(
        text.is_char_boundary(offset),
        "offset {offset} is inside a character"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(range: Range<usize>, text: &str) -> EditorChange {
        EditorChange {
            range,
            text: text.to_string(),
        }
    }

    fn transaction(changes: Vec<EditorChange>) -> EditorTransaction {
        EditorTransaction {
            changes,
            selection: None,
        }
    }

    #[test]
    fn applies_changes_in_offset_order() {
        let edit = transaction(vec![change(6..11, "there"), change(0..0, "> ")]);
        let (text, _) = edit
            .apply("hello world", EditorSelection::default())
            .unwrap();
        assert_eq!(text, "> hello there");
        assert_eq!(
            edit.merged("hello world").unwrap(),
            Some(change(0..11, "> hello there"))
        );
    }

    #[test]
    fn keeps_insertions_at_one_offset_in_order() {
        let edit = transaction(vec![change(0..0, "a"), change(0..0, "b"), change(0..1, "")]);
        let (text, _) = edit.apply("x", EditorSelection::default()).unwrap();
        assert_eq!(text, "ab");
    }

    #[test]
    fn moves_the_selection_with_the_text() {
        let text = "one two three";
        let selection = EditorSelection::new(4, 7);
        // Before the selection: it moves along.
        let (_, after) = transaction(vec![change(0..3, "1")])
            .apply(text, selection)
            .unwrap();
        assert_eq!(after, EditorSelection::new(2, 5));
        // Inserted at the anchor: the anchor stays in front.
        let (_, after) = transaction(vec![change(4..4, "**")])
            .apply(text, selection)
            .unwrap();
        assert_eq!(after, EditorSelection::new(4, 9));
        // Replacing the text under the head: it goes to the start.
        let (_, after) = transaction(vec![change(5..13, "")])
            .apply(text, selection)
            .unwrap();
        assert_eq!(after, EditorSelection::new(4, 5));
    }

    #[test]
    fn an_explicit_selection_refers_to_the_new_text() {
        let edit = EditorTransaction::insert(1, "é");
        let (text, selection) = edit.apply("ab", EditorSelection::cursor(2)).unwrap();
        assert_eq!(text, "aéb");
        assert_eq!(selection, EditorSelection::cursor(3));
    }

    #[test]
    fn rejects_changes_that_do_not_fit() {
        let selection = EditorSelection::default();
        assert!(
            transaction(vec![change(0..9, "")])
                .apply("short", selection)
                .is_err()
        );
        assert!(
            transaction(vec![change(1..1, "")])
                .apply("é", selection)
                .is_err()
        );
        assert!(
            transaction(vec![change(0..3, ""), change(2..4, "")])
                .apply("abcd", selection)
                .is_err()
        );
        let past_the_end = EditorTransaction {
            changes: Vec::new(),
            selection: Some(EditorSelection::cursor(9)),
        };
        assert!(past_the_end.apply("abc", selection).is_err());
    }

    #[test]
    fn nothing_to_merge_without_changes() {
        let select = EditorTransaction {
            changes: Vec::new(),
            selection: Some(EditorSelection::new(0, 2)),
        };
        assert_eq!(select.merged("abc").unwrap(), None);
        assert_eq!(
            select.apply("abc", EditorSelection::default()).unwrap(),
            ("abc".to_string(), EditorSelection::new(0, 2))
        );
    }
}
