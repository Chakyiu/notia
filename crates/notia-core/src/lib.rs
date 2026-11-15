//! The domain model of Notia: a vault of Markdown notes on disk, the metadata
//! parsed from each note, the link and tag index built over the vault, and
//! full-text search. Nothing here depends on the UI.

mod fuzzy;
mod index;
mod note;
mod path;
mod search;
mod vault;

pub use fuzzy::fuzzy_score;
pub use index::{Backlink, IndexChanges, IndexedNote, TagCount, VaultIndex};
pub use note::{Heading, Link, LinkKind, NoteMeta, parse_note};
pub use path::VaultPath;
pub use search::{FileMatches, LineMatch, SearchQuery, search};
pub use vault::{
    ATTACHMENTS_DIR, CONFIG_DIR, EntryKind, FileEntry, FileTree, TRASH_DIR, Vault,
    sanitize_file_name,
};
