//! The files a sync server keeps for each vault.

use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

use crate::{
    FileRev, Manifest, SyncError, parse_rel_path, require_vault_id, sha256_hex,
    snapshot::mtime_ms_now,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreWrite {
    Applied(FileRev),
    /// The file on disk is not the revision the client last read.
    Conflict,
}

pub struct Store {
    root: PathBuf,
    max_bytes: u64,
}

impl Store {
    pub fn open(root: impl Into<PathBuf>, max_bytes: u64) -> Result<Self, SyncError> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|error| {
            SyncError::new(format!("could not create {}: {error}", root.display()))
        })?;
        Ok(Self {
            root,
            max_bytes: max_bytes.max(1),
        })
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    pub fn manifest(&self, vault: &str) -> Result<Manifest, SyncError> {
        let id = require_vault_id(vault)?;
        let path = self.manifest_path(&id);
        if !path.exists() {
            return Ok(Manifest::default());
        }
        let text = fs::read_to_string(&path)
            .map_err(|error| SyncError::new(format!("could not read the manifest: {error}")))?;
        serde_json::from_str(&text)
            .map_err(|error| SyncError::new(format!("the manifest is invalid: {error}")))
    }

    pub fn get(&self, vault: &str, path: &str) -> Result<Option<(FileRev, Vec<u8>)>, SyncError> {
        let file = self.contained(vault, path)?;
        if !file.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&file)
            .map_err(|error| SyncError::new(format!("could not read {path}: {error}")))?;
        let rev = self
            .manifest(vault)?
            .files
            .get(path)
            .cloned()
            .unwrap_or(FileRev {
                sha256: sha256_hex(&bytes),
                size: bytes.len() as u64,
                mtime_ms: 0,
            });
        Ok(Some((rev, bytes)))
    }

    /// Write `bytes` when `base` is the hash currently stored, or when the
    /// file is new and `base` is `None`. An identical hash is accepted again
    /// so a retried upload does not become a conflict.
    pub fn put(
        &self,
        vault: &str,
        path: &str,
        bytes: &[u8],
        mtime_ms: u64,
        base: Option<&str>,
    ) -> Result<StoreWrite, SyncError> {
        if bytes.len() as u64 > self.max_bytes {
            return Err(SyncError::new("the file is larger than the server allows"));
        }
        let file = self.contained(vault, path)?;
        let mut manifest = self.manifest(vault)?;
        let next = FileRev {
            sha256: sha256_hex(bytes),
            size: bytes.len() as u64,
            mtime_ms: if mtime_ms == 0 {
                mtime_ms_now()
            } else {
                mtime_ms
            },
        };
        if let Some(current) = manifest.files.get(path) {
            if current.sha256 == next.sha256 {
                return Ok(StoreWrite::Applied(current.clone()));
            }
            if base != Some(current.sha256.as_str()) {
                return Ok(StoreWrite::Conflict);
            }
        } else if base.is_some() {
            return Ok(StoreWrite::Conflict);
        }
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                SyncError::new(format!("could not create a folder for {path}: {error}"))
            })?;
        }
        let temp = file.with_extension("notia-tmp");
        fs::write(&temp, bytes)
            .map_err(|error| SyncError::new(format!("could not write {path}: {error}")))?;
        fs::rename(&temp, &file).map_err(|error| {
            let _ = fs::remove_file(&temp);
            SyncError::new(format!("could not write {path}: {error}"))
        })?;
        manifest.files.insert(path.to_string(), next.clone());
        bump_generation(&mut manifest);
        self.save_manifest(vault, &manifest)?;
        Ok(StoreWrite::Applied(next))
    }

    /// Delete when `base` is the stored hash. A file that is already gone is
    /// [`StoreWrite::Applied`] with an empty revision so the client can drop
    /// its base entry.
    pub fn delete(&self, vault: &str, path: &str, base: &str) -> Result<StoreWrite, SyncError> {
        let file = self.contained(vault, path)?;
        let mut manifest = self.manifest(vault)?;
        let Some(current) = manifest.files.get(path).cloned() else {
            if file.exists() {
                // The manifest is the list clients see. An orphan file is
                // removed so a later upload is not surprised by it.
                let _ = fs::remove_file(&file);
            }
            return Ok(StoreWrite::Applied(FileRev {
                sha256: String::new(),
                size: 0,
                mtime_ms: 0,
            }));
        };
        if current.sha256 != base {
            return Ok(StoreWrite::Conflict);
        }
        manifest.files.remove(path);
        bump_generation(&mut manifest);
        self.save_manifest(vault, &manifest)?;
        match fs::remove_file(&file) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                manifest.files.insert(path.to_string(), current);
                let _ = self.save_manifest(vault, &manifest);
                return Err(SyncError::new(format!("could not delete {path}: {error}")));
            }
        }
        Ok(StoreWrite::Applied(FileRev {
            sha256: String::new(),
            size: 0,
            mtime_ms: 0,
        }))
    }

    fn contained(&self, vault: &str, path: &str) -> Result<PathBuf, SyncError> {
        let id = require_vault_id(vault)?;
        let Some(relative) = parse_rel_path(path) else {
            return Err(SyncError::new("that path is not allowed"));
        };
        let files = self.files_dir(&id);
        let mut full = files.clone();
        for component in Path::new(relative.as_str()).components() {
            match component {
                Component::Normal(segment) => full.push(segment),
                _ => return Err(SyncError::new("that path is not allowed")),
            }
        }
        if !full.starts_with(&files) {
            return Err(SyncError::new("that path is not allowed"));
        }
        Ok(full)
    }

    fn files_dir(&self, vault: &str) -> PathBuf {
        self.root.join("vaults").join(vault).join("files")
    }

    fn manifest_path(&self, vault: &str) -> PathBuf {
        self.root.join("vaults").join(vault).join("manifest.json")
    }

    fn save_manifest(&self, vault: &str, manifest: &Manifest) -> Result<(), SyncError> {
        let id = require_vault_id(vault)?;
        let path = self.manifest_path(&id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| SyncError::new(format!("could not save the manifest: {error}")))?;
        }
        let body = serde_json::to_vec_pretty(manifest)
            .map_err(|error| SyncError::new(format!("could not save the manifest: {error}")))?;
        let temp = path.with_extension("json.tmp");
        fs::write(&temp, body)
            .map_err(|error| SyncError::new(format!("could not save the manifest: {error}")))?;
        fs::rename(&temp, &path).map_err(|error| {
            let _ = fs::remove_file(&temp);
            SyncError::new(format!("could not save the manifest: {error}"))
        })
    }
}

fn bump_generation(manifest: &mut Manifest) {
    manifest.generation = Some(manifest.generation.unwrap_or(0).saturating_add(1));
}

/// Hashes the store is willing to treat as a base the client last read.
#[cfg(feature = "server")]
pub(crate) fn base_hash(value: Option<&str>) -> Result<Option<String>, SyncError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    crate::normalize_rev(FileRev {
        sha256: value.to_string(),
        size: 0,
        mtime_ms: 0,
    })
    .map(|rev| Some(rev.sha256))
    .ok_or_else(|| SyncError::new("the base hash is invalid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path(), 1024).unwrap();
        (dir, store)
    }

    #[test]
    fn put_get_and_delete_round_trip() {
        let (_dir, store) = store();
        let applied = store
            .put("Notes", "Folder/My Note.md", b"hello", 5, None)
            .unwrap();
        let StoreWrite::Applied(rev) = applied else {
            panic!("expected the write to apply");
        };
        let (loaded, bytes) = store.get("Notes", "Folder/My Note.md").unwrap().unwrap();
        assert_eq!(bytes, b"hello");
        assert_eq!(loaded.sha256, rev.sha256);
        assert!(
            store
                .manifest("Notes")
                .unwrap()
                .files
                .contains_key("Folder/My Note.md")
        );

        let again = store
            .put("Notes", "Folder/My Note.md", b"hello", 5, None)
            .unwrap();
        assert!(matches!(again, StoreWrite::Applied(_)));

        store
            .delete("Notes", "Folder/My Note.md", &rev.sha256)
            .unwrap();
        assert!(store.get("Notes", "Folder/My Note.md").unwrap().is_none());
        assert!(store.manifest("Notes").unwrap().files.is_empty());
    }

    #[test]
    fn a_stale_base_does_not_overwrite() {
        let (_dir, store) = store();
        store.put("Notes", "A.md", b"one", 1, None).unwrap();
        let conflict = store
            .put("Notes", "A.md", b"two", 1, Some("not-the-hash"))
            .unwrap();
        // The presented base is not 64 hex digits, so the server rejects it
        // before the store. The store itself compares the raw hash.
        let _ = conflict;
        let stale = store.put("Notes", "A.md", b"two", 1, Some(&"a".repeat(64)));
        // base_hash is the HTTP layer. The store compares the string directly.
        let result = stale.unwrap();
        assert_eq!(result, StoreWrite::Conflict);
        let (_, bytes) = store.get("Notes", "A.md").unwrap().unwrap();
        assert_eq!(bytes, b"one");
    }

    #[test]
    fn generation_advances_only_when_the_stored_file_changes() {
        let (_dir, store) = store();
        assert_eq!(store.manifest("Notes").unwrap().generation, None);

        let StoreWrite::Applied(first) = store.put("Notes", "A.md", b"one", 1, None).unwrap()
        else {
            panic!("expected the first write to apply");
        };
        assert_eq!(store.manifest("Notes").unwrap().generation, Some(1));

        store.put("Notes", "A.md", b"one", 1, None).unwrap();
        assert_eq!(store.manifest("Notes").unwrap().generation, Some(1));

        let conflict = store
            .put("Notes", "A.md", b"two", 1, Some(&"ab".repeat(32)))
            .unwrap();
        assert_eq!(conflict, StoreWrite::Conflict);
        assert_eq!(store.manifest("Notes").unwrap().generation, Some(1));

        let StoreWrite::Applied(rev) = store
            .put("Notes", "A.md", b"two", 2, Some(&first.sha256))
            .unwrap()
        else {
            panic!("expected the write to apply");
        };
        assert_eq!(store.manifest("Notes").unwrap().generation, Some(2));
        store.delete("Notes", "A.md", &rev.sha256).unwrap();
        assert_eq!(store.manifest("Notes").unwrap().generation, Some(3));
        store.delete("Notes", "A.md", &rev.sha256).unwrap();
        assert_eq!(store.manifest("Notes").unwrap().generation, Some(3));
    }

    #[test]
    fn paths_cannot_escape_the_vault_directory() {
        let (dir, store) = store();
        for path in ["../secret.txt", "foo/../../secret.txt", "/etc/passwd"] {
            let error = store.put("Notes", path, b"nope", 1, None).unwrap_err();
            assert!(error.message().contains("not allowed"), "{path}: {error}");
        }
        assert!(!dir.path().join("secret.txt").exists());
        assert!(!dir.path().parent().unwrap().join("secret.txt").exists());
    }
}
