//! Which notes an MCP client is allowed to see.
//!
//! The lock lives on the note: YAML frontmatter `unlocked: true`. Missing
//! frontmatter, any other value, and every non-Markdown path stay locked.
//! A write cannot create a note or turn a locked note into an unlocked one.

use std::{fs, io, path::Path};

use notia_core::{Vault, VaultPath, parse_note};

/// Largest note read or written when the caller does not pass a limit.
pub const DEFAULT_MAX_NOTE_BYTES: u64 = 1024 * 1024;

pub struct Host {
    vault: Vault,
    max_bytes: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ListedNote {
    pub path: String,
    pub title: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NoteError {
    NotAvailable,
    Locked,
    TooLarge,
    MustStayUnlocked,
    Io(String),
}

impl NoteError {
    fn io(what: &str, error: io::Error) -> Self {
        Self::Io(format!("could not {what}: {error}"))
    }
}

impl std::fmt::Display for NoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAvailable => f.write_str("note is not available"),
            Self::Locked => f.write_str("note is locked"),
            Self::TooLarge => f.write_str("note is larger than the server allows"),
            Self::MustStayUnlocked => {
                f.write_str("the note must stay unlocked; keep `unlocked: true` in its frontmatter")
            }
            Self::Io(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for NoteError {}

impl Host {
    /// Opens the vault at `root`. The folder must already exist.
    pub fn open(root: impl AsRef<Path>, max_bytes: u64) -> Result<Self, String> {
        if max_bytes == 0 {
            return Err("max bytes must be at least 1".into());
        }
        let root = root.as_ref();
        if !root.is_dir() {
            return Err(format!("{} is not a folder", root.display()));
        }
        let vault = Vault::open(root)
            .map_err(|error| format!("could not open {}: {error}", root.display()))?;
        Ok(Self { vault, max_bytes })
    }

    pub fn root(&self) -> &Path {
        self.vault.root()
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    pub(crate) fn list_unlocked(&self) -> Result<Vec<ListedNote>, NoteError> {
        let tree = self
            .vault
            .scan()
            .map_err(|error| NoteError::io("read the vault", error))?;
        let mut listed = Vec::new();
        for path in tree.notes() {
            if !path.is_markdown() || Vault::is_hidden(&path) {
                continue;
            }
            let text = match self.load(&path) {
                Ok(text) => text,
                Err(NoteError::NotAvailable | NoteError::TooLarge) => continue,
                Err(error) => return Err(error),
            };
            let meta = parse_note(&text);
            if !meta.unlocked {
                continue;
            }
            let title = meta
                .title
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| path.file_stem().to_string());
            listed.push(ListedNote {
                path: path.as_str().to_string(),
                title,
            });
        }
        Ok(listed)
    }

    pub(crate) fn read_note(&self, raw: &str) -> Result<String, NoteError> {
        let path = note_path(raw)?;
        let text = self.load(&path)?;
        if parse_note(&text).unlocked {
            Ok(text)
        } else {
            Err(NoteError::Locked)
        }
    }

    pub(crate) fn write_note(&self, raw: &str, content: &str) -> Result<(), NoteError> {
        if content.len() as u64 > self.max_bytes {
            return Err(NoteError::TooLarge);
        }
        let path = note_path(raw)?;
        let current = self.load(&path)?;
        if !parse_note(&current).unlocked {
            return Err(NoteError::Locked);
        }
        if !parse_note(content).unlocked {
            return Err(NoteError::MustStayUnlocked);
        }
        self.vault
            .write_note(&path, content)
            .map_err(|error| NoteError::io("write the note", error))
    }

    fn load(&self, path: &VaultPath) -> Result<String, NoteError> {
        if path.is_root() || !path.is_markdown() || Vault::is_hidden(path) {
            return Err(NoteError::NotAvailable);
        }
        let abs = self.vault.abs_path(path);
        let meta = match fs::symlink_metadata(&abs) {
            Ok(meta) => meta,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(NoteError::NotAvailable);
            }
            Err(error) => return Err(NoteError::io("read the note", error)),
        };
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err(NoteError::NotAvailable);
        }
        // shortcut: oversize files fail before the unlock check, so a locked file
        // over the cap is reported as too large. Upgrade if that distinction matters.
        if meta.len() > self.max_bytes {
            return Err(NoteError::TooLarge);
        }
        let canon = abs
            .canonicalize()
            .map_err(|error| NoteError::io("read the note", error))?;
        if !canon.starts_with(self.vault.root()) {
            return Err(NoteError::NotAvailable);
        }
        let bytes = fs::read(&canon).map_err(|error| NoteError::io("read the note", error))?;
        if bytes.len() as u64 > self.max_bytes {
            return Err(NoteError::TooLarge);
        }
        String::from_utf8(bytes).map_err(|_| NoteError::NotAvailable)
    }
}

fn note_path(raw: &str) -> Result<VaultPath, NoteError> {
    if raw.chars().any(char::is_control) {
        return Err(NoteError::NotAvailable);
    }
    let slash = raw.replace('\\', "/");
    if slash.starts_with('/') || slash.contains(':') {
        return Err(NoteError::NotAvailable);
    }
    if slash
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(NoteError::NotAvailable);
    }
    let path = VaultPath::new(&slash);
    if path.as_str() != slash || Vault::is_hidden(&path) || !path.is_markdown() {
        return Err(NoteError::NotAvailable);
    }
    Ok(path)
}

#[cfg(test)]
pub(crate) struct TestVault {
    pub dir: tempfile::TempDir,
    pub host: Host,
    pub secret: &'static str,
}

#[cfg(test)]
pub(crate) fn test_vault(max_bytes: u64) -> TestVault {
    let dir = tempfile::tempdir().expect("temp dir");
    let secret = "secret-phrase";
    fs::write(dir.path().join("locked.md"), format!("# Secret\n{secret}")).unwrap();
    fs::write(
        dir.path().join("open.md"),
        "---\nunlocked: true\n---\n# Open\nhello\n",
    )
    .unwrap();
    fs::create_dir(dir.path().join("nested")).unwrap();
    fs::write(
        dir.path().join("nested").join("open.md"),
        "---\nunlocked: true\n---\n# Nested\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("note.txt"),
        "---\nunlocked: true\n---\nnot a note",
    )
    .unwrap();
    fs::create_dir(dir.path().join(".notia")).unwrap();
    fs::write(
        dir.path().join(".notia").join("hidden.md"),
        format!("---\nunlocked: true\n---\n{secret}"),
    )
    .unwrap();
    fs::write(
        dir.path().join("body.md"),
        format!("# Title\nunlocked: true\n{secret}"),
    )
    .unwrap();
    let host = Host::open(dir.path(), max_bytes).unwrap();
    TestVault { dir, host, secret }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_only_unlocked_markdown_notes() {
        let vault = test_vault(1024);
        let listed = vault.host.list_unlocked().unwrap();
        let paths: Vec<_> = listed.iter().map(|note| note.path.as_str()).collect();
        assert_eq!(paths, vec!["nested/open.md", "open.md"]);
        assert_eq!(listed[0].title, "Nested");
        assert_eq!(listed[1].title, "Open");
        let rendered = format!("{listed:?}");
        assert!(!rendered.contains(vault.secret));
    }

    #[test]
    fn reads_an_unlocked_note_and_refuses_everything_else() {
        let vault = test_vault(1024);
        let open = vault.host.read_note("open.md").unwrap();
        assert!(open.contains("hello"));
        assert!(open.contains("unlocked: true"));

        let locked = vault.host.read_note("locked.md").unwrap_err();
        assert_eq!(locked, NoteError::Locked);
        assert!(!locked.to_string().contains(vault.secret));

        for path in [
            "body.md",
            "missing.md",
            "note.txt",
            ".notia/hidden.md",
            "../secret.md",
            "nested/../locked.md",
            "/etc/passwd.md",
            "C:/Windows/secret.md",
            "nested//open.md",
        ] {
            let error = vault.host.read_note(path).unwrap_err();
            assert!(
                matches!(error, NoteError::Locked | NoteError::NotAvailable),
                "{path} -> {error:?}"
            );
            assert!(!error.to_string().contains(vault.secret), "{path}");
        }

        let nested = vault.host.read_note(r"nested\open.md").unwrap();
        assert!(nested.contains("# Nested"));
    }

    #[test]
    fn writes_only_existing_unlocked_notes_and_keeps_the_flag() {
        let vault = test_vault(1024);
        let locked_before = fs::read_to_string(vault.dir.path().join("locked.md")).unwrap();
        let error = vault
            .host
            .write_note("locked.md", "---\nunlocked: true\n---\n# Stolen\n")
            .unwrap_err();
        assert_eq!(error, NoteError::Locked);
        assert_eq!(
            fs::read_to_string(vault.dir.path().join("locked.md")).unwrap(),
            locked_before
        );

        let open_before = fs::read_to_string(vault.dir.path().join("open.md")).unwrap();
        let error = vault
            .host
            .write_note("open.md", "# Open\nreplaced\n")
            .unwrap_err();
        assert_eq!(error, NoteError::MustStayUnlocked);
        assert_eq!(
            fs::read_to_string(vault.dir.path().join("open.md")).unwrap(),
            open_before
        );

        let error = vault
            .host
            .write_note("new.md", "---\nunlocked: true\n---\n# New\n")
            .unwrap_err();
        assert_eq!(error, NoteError::NotAvailable);
        assert!(!vault.dir.path().join("new.md").exists());
        assert!(!vault.dir.path().join("missing").exists());

        vault
            .host
            .write_note("open.md", "---\nunlocked: true\n---\n# Open\nupdated\n")
            .unwrap();
        assert!(vault.host.read_note("open.md").unwrap().contains("updated"));
    }

    #[test]
    fn oversized_files_are_refused_without_their_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut unlocked = "---\nunlocked: true\n---\n".to_string();
        unlocked.push_str(&"x".repeat(64));
        fs::write(dir.path().join("big.md"), &unlocked).unwrap();
        fs::write(
            dir.path().join("locked-big.md"),
            format!("secret-phrase{}", "y".repeat(64)),
        )
        .unwrap();
        let host = Host::open(dir.path(), 32).unwrap();
        assert_eq!(host.read_note("big.md").unwrap_err(), NoteError::TooLarge);
        assert_eq!(
            host.read_note("locked-big.md").unwrap_err(),
            NoteError::TooLarge
        );
        assert!(
            !host
                .read_note("big.md")
                .unwrap_err()
                .to_string()
                .contains('x')
        );
        let error = host.write_note("big.md", &"z".repeat(64)).unwrap_err();
        assert_eq!(error, NoteError::TooLarge);
        assert!(
            fs::read_to_string(dir.path().join("big.md"))
                .unwrap()
                .contains("xxx")
        );
    }

    #[test]
    fn crlf_frontmatter_unlocks() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("crlf.md"),
            "---\r\nunlocked: true\r\n---\r\n# Crlf\r\n",
        )
        .unwrap();
        let host = Host::open(dir.path(), 1024).unwrap();
        assert!(host.read_note("crlf.md").unwrap().contains("# Crlf"));
    }

    #[test]
    fn unicode_paths_work() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("笔记.md"),
            "---\nunlocked: true\n---\n# 笔记\n",
        )
        .unwrap();
        let host = Host::open(dir.path(), 1024).unwrap();
        assert!(host.read_note("笔记.md").unwrap().contains("笔记"));
    }

    #[test]
    fn symlinks_are_not_available() {
        let vault = test_vault(1024);
        let link = vault.dir.path().join("link.md");
        if symlink_file(&vault.dir.path().join("open.md"), &link).is_err() {
            // shortcut: skips when the OS refuses symlink creation, upgrade when CI can create them.
            return;
        }
        assert_eq!(
            vault.host.read_note("link.md").unwrap_err(),
            NoteError::NotAvailable
        );
        let error = vault
            .host
            .write_note("link.md", "---\nunlocked: true\n---\n# Link\n")
            .unwrap_err();
        assert_eq!(error, NoteError::NotAvailable);
        assert!(
            fs::read_to_string(vault.dir.path().join("open.md"))
                .unwrap()
                .contains("hello")
        );
    }

    #[test]
    fn refuses_a_missing_vault_and_a_zero_limit() {
        let missing = tempfile::tempdir().unwrap();
        let gone = missing.path().join("nope");
        assert!(Host::open(&gone, 32).is_err());
        assert!(Host::open(missing.path(), 0).is_err());
    }

    fn symlink_file(target: &Path, link: &Path) -> io::Result<()> {
        #[cfg(unix)]
        return std::os::unix::fs::symlink(target, link);
        #[cfg(windows)]
        return std::os::windows::fs::symlink_file(target, link);
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (target, link);
            Err(io::Error::new(io::ErrorKind::Unsupported, "no symlink"))
        }
    }
}
