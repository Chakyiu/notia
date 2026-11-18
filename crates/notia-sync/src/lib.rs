//! Sync a Notia vault with a server the user runs.
//!
//! The app plugin and `notia-sync-server` share this crate. Files are compared
//! by their SHA-256. Each computer remembers the last synced hash, so a file
//! edited on both sides since that sync keeps both copies.

mod paths;
mod plan;
mod snapshot;
mod store;

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "server")]
mod serve;
#[cfg(feature = "client")]
mod sync;

use std::fmt;

pub use paths::{is_synced, parse_rel_path, require_vault_id};
pub use snapshot::{FileRev, Manifest};
pub use store::Store;

#[cfg(feature = "client")]
pub use sync::{RemoteCursor, SyncOptions, SyncReport, probe, remote_cursor, sync_vault};

#[cfg(feature = "server")]
pub use serve::{Listener, bind};

/// Largest file the client will hash or send. The server's default matches.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Plugin settings. Kept out of the sync even when config files are included,
/// so the token and this computer's server address are not uploaded. The
/// plugin id that writes this file is `sync`.
pub const SETTINGS_FILE: &str = ".notia/plugin-data/sync.json";

/// Why a sync could not start, or a single request failed.
#[derive(Debug)]
pub struct SyncError {
    message: String,
}

impl SyncError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SyncError {}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push(HEX[(byte >> 4) as usize] as char);
        hex.push(HEX[(byte & 0x0f) as usize] as char);
    }
    hex
}

const HEX: &[u8; 16] = b"0123456789abcdef";

pub(crate) fn normalize_rev(rev: FileRev) -> Option<FileRev> {
    let sha256 = rev.sha256.to_ascii_lowercase();
    if sha256.len() == 64 && sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Some(FileRev {
            sha256,
            size: rev.size,
            mtime_ms: rev.mtime_ms,
        })
    } else {
        None
    }
}
