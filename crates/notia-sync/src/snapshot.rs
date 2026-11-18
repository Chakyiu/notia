//! What is on disk, and the last revision this computer synced.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    io::Read as _,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use notia_core::CONFIG_DIR;
use serde::{Deserialize, Serialize};

use crate::{SyncError, sha256_hex};

/// One revision of a file. The hash is lowercase hex SHA-256.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRev {
    pub sha256: String,
    pub size: u64,
    pub mtime_ms: u64,
}

/// The server's list of files, and the JSON body of `GET /v1/manifest`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub files: BTreeMap<String, FileRev>,
    /// How many times files in this vault have been stored. A server from
    /// before this field leaves it out; a current server always sends a number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
}

#[derive(Debug)]
pub(crate) struct Snapshot {
    pub files: BTreeMap<String, FileRev>,
    /// Present, but not safe to treat as deleted or unchanged.
    pub held: BTreeSet<String>,
    pub notes: Vec<String>,
}

const STATE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct StoredState {
    version: u32,
    files: BTreeMap<String, FileRev>,
}

pub(crate) fn state_path(root: &Path) -> PathBuf {
    root.join(CONFIG_DIR).join("sync").join("state.json")
}

pub(crate) fn load_state(root: &Path) -> (BTreeMap<String, FileRev>, Option<String>) {
    let path = state_path(root);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return (BTreeMap::new(), None),
        Err(error) => {
            return (
                BTreeMap::new(),
                Some(format!(
                    "the sync history could not be read ({error}), so this sync compares the files directly"
                )),
            );
        }
    };
    match serde_json::from_str::<StoredState>(&text) {
        Ok(state) if state.version == STATE_VERSION => (state.files, None),
        _ => (
            BTreeMap::new(),
            Some(
                "the sync history could not be read, so this sync compares the files directly"
                    .into(),
            ),
        ),
    }
}

pub(crate) fn save_state(root: &Path, files: &BTreeMap<String, FileRev>) -> Result<(), SyncError> {
    let path = state_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| SyncError::new(format!("could not save the sync history: {error}")))?;
    }
    let body = serde_json::to_vec_pretty(&StoredState {
        version: STATE_VERSION,
        files: files.clone(),
    })
    .map_err(|error| SyncError::new(format!("could not save the sync history: {error}")))?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, body)
        .map_err(|error| SyncError::new(format!("could not save the sync history: {error}")))?;
    fs::rename(&temp, &path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        SyncError::new(format!("could not save the sync history: {error}"))
    })
}

pub(crate) enum HashError {
    TooBig,
    Io(io::Error),
}

pub(crate) fn hash_file(path: &Path, max_bytes: u64) -> Result<FileRev, HashError> {
    let bytes = read_limited(path, max_bytes)?;
    Ok(FileRev {
        sha256: sha256_hex(&bytes),
        size: bytes.len() as u64,
        mtime_ms: mtime_of(path),
    })
}

pub(crate) fn mtime_of(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

pub(crate) fn read_limited(path: &Path, max_bytes: u64) -> Result<Vec<u8>, HashError> {
    let file = fs::File::open(path).map_err(HashError::Io)?;
    let mut reader = file.take(max_bytes.saturating_add(1));
    let mut bytes = Vec::new();
    io::Read::read_to_end(&mut reader, &mut bytes).map_err(HashError::Io)?;
    if bytes.len() as u64 > max_bytes {
        return Err(HashError::TooBig);
    }
    Ok(bytes)
}

pub(crate) fn snapshot(
    root: &Path,
    include_config: bool,
    max_bytes: u64,
) -> Result<Snapshot, SyncError> {
    let mut out = Snapshot {
        files: BTreeMap::new(),
        held: BTreeSet::new(),
        notes: Vec::new(),
    };
    walk(root, "", include_config, max_bytes, &mut out)?;
    Ok(out)
}

fn walk(
    dir: &Path,
    relative: &str,
    include_config: bool,
    max_bytes: u64,
    out: &mut Snapshot,
) -> Result<(), SyncError> {
    let entries = fs::read_dir(dir).map_err(|error| {
        SyncError::new(format!(
            "could not read {}: {error}",
            if relative.is_empty() {
                "the vault".to_string()
            } else {
                relative.to_string()
            }
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            SyncError::new(format!("could not read an entry in the vault: {error}"))
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(SyncError::new(
                "a file name is not UTF-8, so sync stopped before it could delete anything",
            ));
        };
        if name.contains('\0') {
            return Err(SyncError::new(
                "a file name contains a null, so sync stopped before it could delete anything",
            ));
        }
        let rel = if relative.is_empty() {
            name.to_string()
        } else {
            format!("{relative}/{name}")
        };
        let file_type = entry
            .file_type()
            .map_err(|error| SyncError::new(format!("could not inspect {rel}: {error}")))?;
        if file_type.is_symlink() {
            // A symlink is not the file. Holding it stops a sync from
            // treating the path as deleted on the server.
            if crate::is_synced(&rel, include_config) {
                out.held.insert(rel);
            }
            continue;
        }
        if file_type.is_dir() {
            if name.starts_with('.') {
                if include_config && relative.is_empty() && name == CONFIG_DIR {
                    walk(&entry.path(), &rel, include_config, max_bytes, out)?;
                }
                continue;
            }
            if rel == format!("{CONFIG_DIR}/sync") || rel == format!("{CONFIG_DIR}/plugins") {
                continue;
            }
            walk(&entry.path(), &rel, include_config, max_bytes, out)?;
            continue;
        }
        if !file_type.is_file() || !crate::is_synced(&rel, include_config) {
            continue;
        }
        match hash_file(&entry.path(), max_bytes) {
            Ok(rev) => {
                out.files.insert(rel, rev);
            }
            Err(HashError::TooBig) => {
                let limit = max_bytes / 1024 / 1024;
                out.notes.push(format!(
                    "{rel} is larger than {limit} MB and was left alone"
                ));
                out.held.insert(rel);
            }
            Err(HashError::Io(error)) => {
                out.notes.push(format!("could not read {rel}: {error}"));
                out.held.insert(rel);
            }
        }
    }
    Ok(())
}

pub(crate) fn mtime_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

pub(crate) fn set_mtime_ms(path: &Path, mtime_ms: u64) {
    if mtime_ms == 0 {
        return;
    }
    let Ok(file) = fs::OpenOptions::new().write(true).open(path) else {
        return;
    };
    let modified = UNIX_EPOCH + std::time::Duration::from_millis(mtime_ms);
    let _ = file.set_modified(modified);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_skips_hidden_files_unless_config_is_included() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("attachments")).unwrap();
        fs::write(root.join("Welcome.md"), "hello").unwrap();
        fs::write(root.join("attachments/pic.txt"), "image").unwrap();
        fs::create_dir_all(root.join(".notia/plugin-data")).unwrap();
        fs::create_dir_all(root.join(".notia/plugins/demo")).unwrap();
        fs::create_dir_all(root.join(".notia/themes")).unwrap();
        fs::create_dir_all(root.join(".trash")).unwrap();
        fs::write(root.join(".notia/app.json"), "{}").unwrap();
        fs::write(root.join(".notia/plugin-data/sync.json"), "secret").unwrap();
        fs::write(root.join(".notia/plugin-data/other.json"), "kept").unwrap();
        fs::write(root.join(".notia/plugins/demo/main.js"), "nope").unwrap();
        fs::write(root.join(".notia/themes/midnight.json"), "{}").unwrap();
        fs::write(root.join(".trash/old.md"), "gone").unwrap();

        let plain = snapshot(root, false, 1024).unwrap();
        assert!(plain.files.contains_key("Welcome.md"));
        assert!(plain.files.contains_key("attachments/pic.txt"));
        assert!(!plain.files.keys().any(|path| path.starts_with('.')));

        let config = snapshot(root, true, 1024).unwrap();
        assert!(config.files.contains_key(".notia/app.json"));
        assert!(config.files.contains_key(".notia/plugin-data/other.json"));
        assert!(config.files.contains_key(".notia/themes/midnight.json"));
        assert!(!config.files.contains_key(".notia/plugin-data/sync.json"));
        assert!(!config.files.contains_key(".notia/plugins/demo/main.js"));
        assert!(!config.files.contains_key(".trash/old.md"));
    }

    #[test]
    fn oversized_files_are_held_so_they_are_not_deleted_remotely() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("big.bin"), vec![1, 2, 3, 4]).unwrap();
        let snap = snapshot(dir.path(), false, 2).unwrap();
        assert!(snap.files.is_empty());
        assert!(snap.held.contains("big.bin"));
        assert_eq!(snap.notes.len(), 1);
    }

    #[test]
    fn state_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut files = BTreeMap::new();
        files.insert(
            "Welcome.md".into(),
            FileRev {
                sha256: "ab".into(),
                size: 1,
                mtime_ms: 2,
            },
        );
        save_state(dir.path(), &files).unwrap();
        let (loaded, warning) = load_state(dir.path());
        assert!(warning.is_none());
        assert_eq!(loaded, files);
    }
}
