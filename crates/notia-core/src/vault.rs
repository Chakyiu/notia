use std::{
    cmp::Ordering,
    fs, io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::VaultPath;

/// Where Notia keeps vault-scoped configuration, like Obsidian's `.obsidian`.
pub const CONFIG_DIR: &str = ".notia";
/// Where deleted entries go, so a delete can be undone from the file manager.
pub const TRASH_DIR: &str = ".trash";
/// Where pasted images are stored, relative to the vault root.
pub const ATTACHMENTS_DIR: &str = "attachments";

/// A folder of Markdown notes on disk.
///
/// Every operation takes and returns [`VaultPath`]s, so callers never build
/// absolute paths themselves.
#[derive(Clone, Debug)]
pub struct Vault {
    root: PathBuf,
}

impl Vault {
    /// Opens the vault at `root`, creating the folder when it is missing.
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self {
            root: root.canonicalize()?,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The display name of the vault: its folder name.
    pub fn name(&self) -> String {
        self.root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Vault".to_string())
    }

    pub fn config_dir(&self) -> PathBuf {
        self.root.join(CONFIG_DIR)
    }

    pub fn abs_path(&self, path: &VaultPath) -> PathBuf {
        if path.is_root() {
            self.root.clone()
        } else {
            self.root.join(path.as_str())
        }
    }

    pub fn relative_path(&self, abs: &Path) -> Option<VaultPath> {
        let relative = abs.strip_prefix(&self.root).ok()?;
        Some(VaultPath::new(relative.to_string_lossy()))
    }

    /// Whether any segment of `path` is hidden, like `.notia` or `.git`.
    pub fn is_hidden(path: &VaultPath) -> bool {
        path.as_str()
            .split('/')
            .any(|segment| segment.starts_with('.'))
    }

    pub fn exists(&self, path: &VaultPath) -> bool {
        self.abs_path(path).exists()
    }

    /// Reads the folder structure, skipping hidden entries.
    pub fn scan(&self) -> io::Result<FileTree> {
        Ok(FileTree {
            entries: self.scan_dir(&VaultPath::root(), 0)?,
        })
    }

    fn scan_dir(&self, dir: &VaultPath, depth: usize) -> io::Result<Vec<FileEntry>> {
        // Deep enough for any real vault, and a guard against pathological trees.
        const MAX_DEPTH: usize = 64;

        let mut entries = Vec::new();
        for entry in fs::read_dir(self.abs_path(dir))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let file_type = entry.file_type()?;
            let path = dir.join(&name);
            if file_type.is_dir() {
                let children = if depth < MAX_DEPTH {
                    self.scan_dir(&path, depth + 1)?
                } else {
                    Vec::new()
                };
                entries.push(FileEntry {
                    path,
                    name,
                    kind: EntryKind::Folder,
                    children,
                });
            } else if file_type.is_file() {
                let kind = if path.is_note() {
                    EntryKind::Note
                } else {
                    EntryKind::Attachment
                };
                entries.push(FileEntry {
                    path,
                    name,
                    kind,
                    children: Vec::new(),
                });
            }
        }
        entries.sort_by(FileEntry::display_order);
        Ok(entries)
    }

    pub fn read_note(&self, path: &VaultPath) -> io::Result<String> {
        fs::read_to_string(self.abs_path(path))
    }

    pub fn modified(&self, path: &VaultPath) -> Option<SystemTime> {
        fs::metadata(self.abs_path(path))
            .and_then(|m| m.modified())
            .ok()
    }

    /// When the file was created. `None` when the file is missing or the
    /// platform does not record a creation time.
    pub fn created(&self, path: &VaultPath) -> Option<SystemTime> {
        fs::metadata(self.abs_path(path))
            .and_then(|meta| meta.created())
            .ok()
    }

    /// Writes through a temporary sibling and a rename, so a crash never
    /// leaves a half-written note behind.
    pub fn write_note(&self, path: &VaultPath, content: &str) -> io::Result<()> {
        self.write_bytes(path, content.as_bytes())
    }

    /// Writes bytes through a temporary sibling and a rename, creating parent
    /// folders. Used for notes and for pasted images.
    pub fn write_bytes(&self, path: &VaultPath, bytes: &[u8]) -> io::Result<()> {
        let target = self.abs_path(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp = target.with_file_name(format!(".{}.notia-tmp", path.file_name()));
        fs::write(&temp, bytes)?;
        fs::rename(&temp, &target).inspect_err(|_| {
            let _ = fs::remove_file(&temp);
        })
    }

    /// Creates a note named after `title` in `folder`, numbering the name when
    /// it is taken, and returns its path.
    pub fn create_note(
        &self,
        folder: &VaultPath,
        title: &str,
        content: &str,
    ) -> io::Result<VaultPath> {
        self.create_named(folder, title, "md", content)
    }

    /// Creates a Canvas note, whose body is the board as JSON.
    pub fn create_canvas(
        &self,
        folder: &VaultPath,
        title: &str,
        content: &str,
    ) -> io::Result<VaultPath> {
        self.create_named(folder, title, "canvas", content)
    }

    /// Creates a Kanban note, whose body is the board as JSON.
    pub fn create_kanban(
        &self,
        folder: &VaultPath,
        title: &str,
        content: &str,
    ) -> io::Result<VaultPath> {
        self.create_named(folder, title, "kanban", content)
    }

    fn create_named(
        &self,
        folder: &VaultPath,
        title: &str,
        extension: &str,
        content: &str,
    ) -> io::Result<VaultPath> {
        let path = self.unique_path(folder, &sanitize_file_name(title), Some(extension));
        self.write_note(&path, content)?;
        Ok(path)
    }

    pub fn create_folder(&self, parent: &VaultPath, name: &str) -> io::Result<VaultPath> {
        let path = self.unique_path(parent, &sanitize_file_name(name), None);
        fs::create_dir_all(self.abs_path(&path))?;
        Ok(path)
    }

    /// Renames the entry in place. A note keeps its extension when the new
    /// name has none.
    pub fn rename(&self, path: &VaultPath, new_name: &str) -> io::Result<VaultPath> {
        let mut name = sanitize_file_name(new_name);
        if VaultPath::new(&name).extension().is_none()
            && let Some(extension) = path.extension().filter(|_| path.is_note())
        {
            name.push('.');
            name.push_str(extension);
        }
        let target = path.with_file_name(&name);
        if target == *path {
            return Ok(target);
        }
        // A case-only rename is the same file on case-insensitive file systems.
        if self.exists(&target) && !target.as_str().eq_ignore_ascii_case(path.as_str()) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("“{name}” already exists"),
            ));
        }
        fs::rename(self.abs_path(path), self.abs_path(&target))?;
        Ok(target)
    }

    /// Moves the entry into `folder`, keeping its name.
    pub fn move_into(&self, path: &VaultPath, folder: &VaultPath) -> io::Result<VaultPath> {
        if folder.starts_with(path) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a folder cannot be moved into itself",
            ));
        }
        let target = folder.join(path.file_name());
        if target == *path {
            return Ok(target);
        }
        if self.exists(&target) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("“{}” already exists in the destination", path.file_name()),
            ));
        }
        fs::rename(self.abs_path(path), self.abs_path(&target))?;
        Ok(target)
    }

    /// Moves the entry into the vault's `.trash` folder.
    pub fn trash(&self, path: &VaultPath) -> io::Result<()> {
        let trash = VaultPath::new(TRASH_DIR);
        fs::create_dir_all(self.abs_path(&trash))?;
        let target = self.unique_path(&trash, path.file_stem(), path.extension());
        fs::rename(self.abs_path(path), self.abs_path(&target))
    }

    /// The first free `stem.ext`, `stem 1.ext`, `stem 2.ext`, … in `folder`.
    pub fn unique_path(
        &self,
        folder: &VaultPath,
        stem: &str,
        extension: Option<&str>,
    ) -> VaultPath {
        let file_name = |n: usize| {
            let stem = if n == 0 {
                stem.to_string()
            } else {
                format!("{stem} {n}")
            };
            match extension {
                Some(ext) => format!("{stem}.{ext}"),
                None => stem,
            }
        };
        (0..)
            .map(|n| folder.join(&file_name(n)))
            .find(|candidate| !self.exists(candidate))
            .expect("an unbounded range always yields a free name")
    }
}

/// Removes characters that file systems or wikilinks cannot hold, falling
/// back to `Untitled` when nothing is left.
pub fn sanitize_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '#' | '^' | '[' | ']' => ' ',
            ch if ch.is_control() => ' ',
            ch => ch,
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches('.').trim();
    if trimmed.is_empty() {
        "Untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The folder structure of a vault, folders first and then by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileTree {
    entries: Vec<FileEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Folder,
    Note,
    Attachment,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileEntry {
    pub path: VaultPath,
    pub name: String,
    pub kind: EntryKind,
    pub children: Vec<FileEntry>,
}

impl FileEntry {
    pub fn is_folder(&self) -> bool {
        self.kind == EntryKind::Folder
    }

    /// The label the file explorer shows: notes hide their `.md` extension.
    pub fn display_name(&self) -> &str {
        match self.kind {
            EntryKind::Note => self.path.file_stem(),
            _ => &self.name,
        }
    }

    fn display_order(a: &FileEntry, b: &FileEntry) -> Ordering {
        b.is_folder()
            .cmp(&a.is_folder())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    }
}

impl FileTree {
    pub fn entries(&self) -> &[FileEntry] {
        &self.entries
    }

    /// Every note in display order.
    pub fn notes(&self) -> Vec<VaultPath> {
        let mut notes = Vec::new();
        Self::walk(&self.entries, &mut |entry| {
            if entry.kind == EntryKind::Note {
                notes.push(entry.path.clone());
            }
        });
        notes
    }

    pub fn folders(&self) -> Vec<VaultPath> {
        let mut folders = Vec::new();
        Self::walk(&self.entries, &mut |entry| {
            if entry.is_folder() {
                folders.push(entry.path.clone());
            }
        });
        folders
    }

    pub fn find(&self, path: &VaultPath) -> Option<&FileEntry> {
        let mut entries = &self.entries;
        let mut found = None;
        for segment_end in path
            .as_str()
            .match_indices('/')
            .map(|(ix, _)| ix)
            .chain([path.as_str().len()])
        {
            let prefix = &path.as_str()[..segment_end];
            let entry = entries.iter().find(|entry| entry.path.as_str() == prefix)?;
            entries = &entry.children;
            found = Some(entry);
        }
        found
    }

    fn walk(entries: &[FileEntry], visit: &mut impl FnMut(&FileEntry)) {
        for entry in entries {
            visit(entry);
            Self::walk(&entry.children, visit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        (dir, vault)
    }

    #[test]
    fn scans_folders_first_and_skips_hidden_entries() {
        let (_dir, vault) = vault();
        vault.write_note(&"b.md".into(), "b").unwrap();
        vault.write_note(&"A/c.md".into(), "c").unwrap();
        vault
            .write_note(&".notia/settings.json".into(), "{}")
            .unwrap();
        fs::write(vault.abs_path(&"image.png".into()), [0u8]).unwrap();

        let tree = vault.scan().unwrap();
        let names: Vec<_> = tree.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["A", "b.md", "image.png"]);
        assert_eq!(tree.entries()[2].kind, EntryKind::Attachment);
        assert_eq!(
            tree.notes(),
            vec![VaultPath::new("A/c.md"), VaultPath::new("b.md")]
        );
        assert_eq!(
            tree.find(&"A/c.md".into()).map(|e| e.kind),
            Some(EntryKind::Note)
        );
        assert!(tree.find(&"A/missing.md".into()).is_none());
    }

    #[test]
    fn creates_unique_note_names() {
        let (_dir, vault) = vault();
        let first = vault.create_note(&VaultPath::root(), "Idea", "").unwrap();
        let second = vault.create_note(&VaultPath::root(), "Idea", "").unwrap();
        let odd = vault
            .create_note(&VaultPath::root(), "  a/b: c?  ", "")
            .unwrap();
        assert_eq!(first.as_str(), "Idea.md");
        assert_eq!(second.as_str(), "Idea 1.md");
        assert_eq!(odd.as_str(), "a b c.md");
    }

    #[test]
    fn writes_bytes_and_creates_parent_folders() {
        let (_dir, vault) = vault();
        let path = VaultPath::new("attachments/Pasted image.png");
        vault.write_bytes(&path, b"png").unwrap();
        assert_eq!(fs::read(vault.abs_path(&path)).unwrap(), b"png");
    }

    #[test]
    fn renames_moves_and_trashes() {
        let (_dir, vault) = vault();
        let note = vault
            .create_note(&VaultPath::root(), "Draft", "text")
            .unwrap();
        let renamed = vault.rename(&note, "Final").unwrap();
        assert_eq!(renamed.as_str(), "Final.md");
        assert_eq!(vault.read_note(&renamed).unwrap(), "text");

        let folder = vault.create_folder(&VaultPath::root(), "Archive").unwrap();
        let moved = vault.move_into(&renamed, &folder).unwrap();
        assert_eq!(moved.as_str(), "Archive/Final.md");
        assert!(vault.move_into(&folder, &folder).is_err());

        vault.trash(&moved).unwrap();
        assert!(!vault.exists(&moved));
        assert!(vault.exists(&VaultPath::new(".trash/Final.md")));
    }

    #[test]
    fn rename_refuses_to_overwrite() {
        let (_dir, vault) = vault();
        let a = vault.create_note(&VaultPath::root(), "A", "a").unwrap();
        vault.create_note(&VaultPath::root(), "B", "b").unwrap();
        assert!(vault.rename(&a, "B").is_err());
        assert_eq!(vault.read_note(&"B.md".into()).unwrap(), "b");
    }

    #[test]
    fn reads_created_and_modified_times() {
        let (_dir, vault) = vault();
        let path = vault.create_note(&VaultPath::root(), "Times", "x").unwrap();
        assert!(vault.modified(&path).is_some());
        if cfg!(any(target_os = "macos", target_os = "windows")) {
            assert!(vault.created(&path).is_some());
        }
        assert!(vault.created(&VaultPath::new("missing.md")).is_none());
        assert!(vault.modified(&VaultPath::new("missing.md")).is_none());
    }

    #[test]
    fn kanban_notes_keep_their_extension() {
        let (_dir, vault) = vault();
        let path = vault
            .create_kanban(&VaultPath::root(), "Board", "{\"columns\":[]}")
            .unwrap();
        assert_eq!(path.as_str(), "Board.kanban");
        assert_eq!(vault.scan().unwrap().entries()[0].kind, EntryKind::Note);
        assert_eq!(vault.scan().unwrap().entries()[0].display_name(), "Board");
        let renamed = vault.rename(&path, "Plan").unwrap();
        assert_eq!(renamed.as_str(), "Plan.kanban");
        assert_eq!(vault.read_note(&renamed).unwrap(), "{\"columns\":[]}");
    }
}
