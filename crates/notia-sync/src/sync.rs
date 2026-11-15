//! One pass over a vault: read it, compare it with the server, and transfer
//! the difference.

use std::{collections::BTreeMap, fs, io};

use notia_core::{CONFIG_DIR, Vault, VaultPath};

use crate::{
    SyncError,
    client::{SyncClient, WriteResult},
    is_synced, normalize_rev,
    plan::{ChangeKind, plan},
    snapshot::{self, FileRev, HashError, hash_file, mtime_of, read_limited, set_mtime_ms},
};

/// What one computer asks to sync. The token is not printed.
pub struct SyncOptions {
    pub root: std::path::PathBuf,
    pub server_url: String,
    pub token: String,
    pub vault_id: String,
    pub include_config: bool,
    pub max_bytes: u64,
}

/// What a sync changed. Counts are successful transfers. `errors` are files
/// that were left for the next sync.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub uploaded: usize,
    pub downloaded: usize,
    pub deleted_local: usize,
    pub deleted_remote: usize,
    pub conflicts: Vec<String>,
    pub skipped: Vec<String>,
    pub errors: Vec<String>,
}

impl SyncReport {
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.uploaded > 0 {
            parts.push(count(self.uploaded, "uploaded"));
        }
        if self.downloaded > 0 {
            parts.push(count(self.downloaded, "downloaded"));
        }
        let deleted = self.deleted_local + self.deleted_remote;
        if deleted > 0 {
            parts.push(count(deleted, "deleted"));
        }
        match self.conflicts.as_slice() {
            [] => {}
            [only] => parts.push(format!("1 conflict ({only})")),
            many => parts.push(format!("{} conflicts", many.len())),
        }
        if !self.skipped.is_empty() {
            parts.push(count(self.skipped.len(), "skipped"));
        }
        if parts.is_empty() && self.errors.is_empty() {
            return "Synced".to_string();
        }
        let mut text = if parts.is_empty() {
            "Sync incomplete".to_string()
        } else {
            format!("Synced · {}", parts.join(", "))
        };
        if let Some(error) = self.errors.first() {
            text = if parts.is_empty() {
                format!("Sync failed: {error}")
            } else {
                format!("{text}. {error}")
            };
        }
        text
    }

    /// Files on this computer were written or moved to the trash.
    pub fn changed_disk(&self) -> bool {
        self.downloaded > 0 || self.deleted_local > 0 || !self.conflicts.is_empty()
    }
}

fn count(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}")
    }
}

/// Ask the server how many files it has for this vault. Does not transfer any.
pub fn probe(options: &SyncOptions) -> Result<usize, SyncError> {
    let client = client(options)?;
    Ok(client.manifest()?.files.len())
}

/// Sync `options.root` with the server. A failure to reach the server or to
/// read the vault returns an error and changes nothing. A failure of one file
/// is recorded in the report and the other files still sync.
pub fn sync_vault(options: &SyncOptions) -> Result<SyncReport, SyncError> {
    let client = client(options)?;
    let snapshot = snapshot::snapshot(&options.root, options.include_config, options.max_bytes)?;
    let (mut base, warning) = snapshot::load_state(&options.root);
    let mut parked = BTreeMap::new();
    base.retain(|path, rev| {
        if is_synced(path, options.include_config) {
            true
        } else {
            parked.insert(path.clone(), rev.clone());
            false
        }
    });
    let remote_manifest = client.manifest()?;
    let mut remote = BTreeMap::new();
    for (path, rev) in remote_manifest.files {
        if !is_synced(&path, options.include_config) {
            continue;
        }
        let Some(rev) = normalize_rev(rev) else {
            return Err(SyncError::new(format!(
                "the server sent an invalid hash for {path}"
            )));
        };
        remote.insert(path, rev);
    }
    let planned = plan(
        &snapshot.files,
        &remote,
        &base,
        &snapshot.held,
        &conflict_stamp(),
    );
    let mut next = base;
    for (path, rev) in planned.aligned {
        next.insert(path, rev);
    }
    for path in planned.forget {
        next.remove(&path);
    }

    let vault = Vault::open(&options.root)
        .map_err(|error| SyncError::new(format!("could not open the vault: {error}")))?;
    let mut report = SyncReport {
        skipped: snapshot.notes,
        ..SyncReport::default()
    };
    if let Some(warning) = warning {
        report.errors.push(warning);
    }
    for change in planned.changes {
        if let Err(error) = apply(
            &vault,
            &client,
            options.max_bytes,
            &change,
            &mut next,
            &mut report,
        ) {
            report.errors.push(error);
        }
    }
    next.extend(parked);
    if let Err(error) = snapshot::save_state(&options.root, &next) {
        report.errors.push(error.to_string());
    }
    Ok(report)
}

fn client(options: &SyncOptions) -> Result<SyncClient, SyncError> {
    SyncClient::connect(
        &options.server_url,
        &options.token,
        &options.vault_id,
        options.max_bytes,
    )
}

fn conflict_stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

fn apply(
    vault: &Vault,
    client: &SyncClient,
    max_bytes: u64,
    change: &crate::plan::Change,
    base: &mut BTreeMap<String, FileRev>,
    report: &mut SyncReport,
) -> Result<(), String> {
    let path = change.path.as_str();
    match change.kind {
        ChangeKind::Download => {
            if !same_as_planned(vault, path, change.local.as_ref(), max_bytes)? {
                report.skipped.push(format!("{path} changed while syncing"));
                return Ok(());
            }
            let bytes = client.download(path).map_err(|error| error.to_string())?;
            let remote = change
                .remote
                .as_ref()
                .ok_or_else(|| format!("{path} has no server copy"))?;
            if sha256_of(&bytes) != remote.sha256 {
                return Err(format!("{path} from the server did not match its hash"));
            }
            write_file(vault, path, &bytes, remote.mtime_ms)?;
            base.insert(path.to_string(), remote.clone());
            report.downloaded += 1;
        }
        ChangeKind::Upload => {
            let Some(rev) = upload_current(
                vault,
                client,
                path,
                change.remote.as_ref(),
                max_bytes,
                report,
            )?
            else {
                return Ok(());
            };
            base.insert(path.to_string(), rev);
            report.uploaded += 1;
        }
        ChangeKind::DeleteLocal => {
            let vault_path = VaultPath::new(path);
            if vault.exists(&vault_path) {
                if !same_as_planned(vault, path, change.local.as_ref(), max_bytes)? {
                    report.skipped.push(format!("{path} changed while syncing"));
                    return Ok(());
                }
                vault
                    .trash(&vault_path)
                    .map_err(|error| format!("could not delete {path}: {error}"))?;
                remove_empty_dirs(vault, &vault_path);
            }
            base.remove(path);
            report.deleted_local += 1;
        }
        ChangeKind::DeleteRemote => {
            let remote = change
                .remote
                .as_ref()
                .ok_or_else(|| format!("{path} has no server copy"))?;
            match client
                .delete(path, &remote.sha256)
                .map_err(|error| error.to_string())?
            {
                WriteResult::Applied(_) => {
                    base.remove(path);
                    report.deleted_remote += 1;
                }
                WriteResult::Rejected => {
                    report.errors.push(format!(
                        "{path} changed on the server and will be merged on the next sync"
                    ));
                }
            }
        }
        ChangeKind::Conflict => {
            let remote = change
                .remote
                .as_ref()
                .ok_or_else(|| format!("{path} has no server copy"))?;
            let conflict_path = change
                .conflict_path
                .as_deref()
                .ok_or_else(|| format!("{path} has no place to keep the other copy"))?;
            let bytes = client.download(path).map_err(|error| error.to_string())?;
            if sha256_of(&bytes) != remote.sha256 {
                return Err(format!("{path} from the server did not match its hash"));
            }
            write_file(vault, conflict_path, &bytes, remote.mtime_ms)?;
            report.conflicts.push(path.to_string());
            if let Some(rev) = upload_current(vault, client, path, Some(remote), max_bytes, report)?
            {
                base.insert(path.to_string(), rev);
                report.uploaded += 1;
            }
            match client
                .upload(conflict_path, &bytes, remote.mtime_ms, None)
                .map_err(|error| error.to_string())?
            {
                WriteResult::Applied(rev) => {
                    base.insert(conflict_path.to_string(), rev);
                    report.uploaded += 1;
                }
                WriteResult::Rejected => report.errors.push(format!(
                    "{conflict_path} was already on the server and will be merged on the next sync"
                )),
            }
        }
    }
    Ok(())
}

/// Upload the bytes on disk now. `None` means the file was gone or the server
/// rejected the write; the caller leaves the base entry as it is.
fn upload_current(
    vault: &Vault,
    client: &SyncClient,
    path: &str,
    remote: Option<&FileRev>,
    max_bytes: u64,
    report: &mut SyncReport,
) -> Result<Option<FileRev>, String> {
    let abs = vault.abs_path(&VaultPath::new(path));
    let bytes = match read_limited(&abs, max_bytes) {
        Ok(bytes) => bytes,
        Err(HashError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            report
                .skipped
                .push(format!("{path} disappeared while syncing"));
            return Ok(None);
        }
        Err(HashError::TooBig) => {
            report
                .skipped
                .push(format!("{path} is too large and was left alone"));
            return Ok(None);
        }
        Err(HashError::Io(error)) => return Err(format!("could not read {path}: {error}")),
    };
    let rev = FileRev {
        sha256: sha256_of(&bytes),
        size: bytes.len() as u64,
        mtime_ms: mtime_of(&abs),
    };
    let base = remote.map(|rev| rev.sha256.as_str());
    match client
        .upload(path, &bytes, rev.mtime_ms, base)
        .map_err(|error| error.to_string())?
    {
        WriteResult::Applied(_) => Ok(Some(rev)),
        WriteResult::Rejected => {
            report.errors.push(format!(
                "{path} changed on the server and will be merged on the next sync"
            ));
            Ok(None)
        }
    }
}

fn same_as_planned(
    vault: &Vault,
    path: &str,
    expected: Option<&FileRev>,
    max_bytes: u64,
) -> Result<bool, String> {
    let abs = vault.abs_path(&VaultPath::new(path));
    match hash_file(&abs, max_bytes) {
        Ok(rev) => Ok(expected.is_some_and(|expected| expected.sha256 == rev.sha256)),
        Err(HashError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            Ok(expected.is_none())
        }
        Err(HashError::TooBig) => Ok(false),
        Err(HashError::Io(error)) => Err(format!("could not read {path}: {error}")),
    }
}

fn write_file(vault: &Vault, path: &str, bytes: &[u8], mtime_ms: u64) -> Result<(), String> {
    let vault_path = VaultPath::new(path);
    vault
        .write_bytes(&vault_path, bytes)
        .map_err(|error| format!("could not write {path}: {error}"))?;
    set_mtime_ms(&vault.abs_path(&vault_path), mtime_ms);
    Ok(())
}

fn remove_empty_dirs(vault: &Vault, path: &VaultPath) {
    let mut parent = path.parent();
    while let Some(dir) = parent {
        if dir.is_root() || dir.as_str() == CONFIG_DIR {
            break;
        }
        let abs = vault.abs_path(&dir);
        let Ok(mut entries) = fs::read_dir(&abs) else {
            break;
        };
        if entries.next().is_some() {
            break;
        }
        if fs::remove_dir(&abs).is_err() {
            break;
        }
        parent = dir.parent();
    }
}

fn sha256_of(bytes: &[u8]) -> String {
    crate::sha256_hex(bytes)
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use std::{fs, path::Path, thread, time::Duration};

    use super::{SyncOptions, probe, sync_vault};
    use crate::serve::start_test_server;

    fn server() -> (String, crate::serve::TestServer) {
        let running = start_test_server("test-token", 1024 * 1024);
        let url = running.url.clone();
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_millis(300)))
            .http_status_as_error(false)
            .build()
            .into();
        for _ in 0..50 {
            if agent
                .get(format!("{url}/v1/health"))
                .call()
                .is_ok_and(|response| response.status().is_success())
            {
                return (url, running);
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("the sync server did not start");
    }

    fn options(root: &Path, url: &str, vault: &str, include_config: bool) -> SyncOptions {
        SyncOptions {
            root: root.to_path_buf(),
            server_url: url.to_string(),
            token: "test-token".into(),
            vault_id: vault.into(),
            include_config,
            max_bytes: 1024 * 1024,
        }
    }

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn two_computers_converge_and_keep_both_edits() {
        let (url, _server) = server();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        write(&a.path().join("Welcome.md"), "hello");
        write(&a.path().join("attachments/pic.txt"), "image");
        write(&a.path().join("My Note.md"), "spaced");

        let first = sync_vault(&options(a.path(), &url, "notes", false)).unwrap();
        assert!(first.errors.is_empty(), "{first:?}");
        assert_eq!(first.uploaded, 3);

        let pulled = sync_vault(&options(b.path(), &url, "notes", false)).unwrap();
        assert!(pulled.errors.is_empty(), "{pulled:?}");
        assert_eq!(read(&b.path().join("Welcome.md")), "hello");
        assert_eq!(read(&b.path().join("attachments/pic.txt")), "image");
        assert_eq!(read(&b.path().join("My Note.md")), "spaced");

        write(&b.path().join("Welcome.md"), "from b");
        fs::rename(b.path().join("My Note.md"), b.path().join("Renamed.md")).unwrap();
        sync_vault(&options(b.path(), &url, "notes", false)).unwrap();
        sync_vault(&options(a.path(), &url, "notes", false)).unwrap();
        assert_eq!(read(&a.path().join("Welcome.md")), "from b");
        assert_eq!(read(&a.path().join("Renamed.md")), "spaced");
        assert!(!a.path().join("My Note.md").exists());

        fs::remove_file(a.path().join("attachments/pic.txt")).unwrap();
        sync_vault(&options(a.path(), &url, "notes", false)).unwrap();
        sync_vault(&options(b.path(), &url, "notes", false)).unwrap();
        assert!(!b.path().join("attachments/pic.txt").exists());

        write(&a.path().join("Welcome.md"), "from a");
        write(&b.path().join("Welcome.md"), "from b again");
        sync_vault(&options(a.path(), &url, "notes", false)).unwrap();
        let conflicted = sync_vault(&options(b.path(), &url, "notes", false)).unwrap();
        assert!(conflicted.errors.is_empty(), "{conflicted:?}");
        assert_eq!(read(&b.path().join("Welcome.md")), "from b again");
        let kept = fs::read_dir(b.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains("sync-conflict")
            })
            .expect("conflict copy");
        assert_eq!(read(&kept.path()), "from a");

        sync_vault(&options(a.path(), &url, "notes", false)).unwrap();
        assert_eq!(read(&a.path().join("Welcome.md")), "from b again");
        assert!(fs::read_dir(a.path()).unwrap().any(|entry| {
            entry.ok().is_some_and(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains("sync-conflict")
            })
        }));
    }

    #[test]
    fn config_files_sync_only_when_asked() {
        let (url, _server) = server();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        write(&a.path().join("Welcome.md"), "note");
        write(&a.path().join(".notia/app.json"), "{\"theme\":1}");
        write(&a.path().join(".notia/plugin-data/sync.json"), "secret");
        write(&a.path().join(".notia/plugin-data/other.json"), "kept");
        write(&a.path().join(".notia/plugins/demo/main.js"), "nope");
        write(&a.path().join(".trash/old.md"), "trash");

        sync_vault(&options(a.path(), &url, "config", false)).unwrap();
        sync_vault(&options(b.path(), &url, "config", false)).unwrap();
        assert_eq!(read(&b.path().join("Welcome.md")), "note");
        assert!(!b.path().join(".notia/app.json").exists());

        sync_vault(&options(a.path(), &url, "config", true)).unwrap();
        sync_vault(&options(b.path(), &url, "config", true)).unwrap();
        assert_eq!(read(&b.path().join(".notia/app.json")), "{\"theme\":1}");
        assert_eq!(
            read(&b.path().join(".notia/plugin-data/other.json")),
            "kept"
        );
        assert!(!b.path().join(".notia/plugin-data/sync.json").exists());
        assert!(!b.path().join(".notia/plugins/demo/main.js").exists());
        assert!(!b.path().join(".trash/old.md").exists());
    }

    #[test]
    fn a_wrong_token_is_rejected() {
        let (url, _server) = server();
        let vault = tempfile::tempdir().unwrap();
        let mut options = options(vault.path(), &url, "notes", false);
        options.token = "nope".into();
        let error = probe(&options).unwrap_err();
        assert!(error.message().contains("token"), "{error}");
    }

    #[test]
    fn identical_files_are_not_copied_twice() {
        let (url, _server) = server();
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        write(&a.path().join("Welcome.md"), "same");
        write(&b.path().join("Welcome.md"), "same");
        sync_vault(&options(a.path(), &url, "same", false)).unwrap();
        let second = sync_vault(&options(b.path(), &url, "same", false)).unwrap();
        assert!(second.errors.is_empty(), "{second:?}");
        assert_eq!(second.uploaded, 0);
        assert_eq!(second.downloaded, 0);
        assert!(second.conflicts.is_empty());
        assert_eq!(read(&b.path().join("Welcome.md")), "same");
    }
}
