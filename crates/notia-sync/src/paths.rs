//! Vault-relative paths that are safe to send to another computer.

use std::collections::BTreeSet;

use notia_core::{CONFIG_DIR, VaultPath};

use crate::SETTINGS_FILE;

/// Accept `Folder/Note.md` and reject anything that would leave the vault
/// after normalization, including `..`, absolute paths, and backslashes.
pub fn parse_rel_path(raw: &str) -> Option<VaultPath> {
    if raw.is_empty()
        || raw.len() > 1024
        || raw.contains('\0')
        || raw.contains('\\')
        || raw.starts_with('/')
        || raw.ends_with('/')
        || raw.contains("//")
    {
        return None;
    }
    let path = VaultPath::new(raw);
    if path.as_str() != raw {
        return None;
    }
    if path
        .as_str()
        .split('/')
        .any(|segment| segment.is_empty() || segment.len() > 255)
    {
        return None;
    }
    Some(path)
}

/// A vault id is one folder name on the server. Empty, dotted, and path
/// separators are rejected so two computers cannot aim a vault outside its
/// directory.
pub fn require_vault_id(raw: &str) -> Result<String, crate::SyncError> {
    let id = raw.trim();
    if id.is_empty() || id.chars().count() > 80 {
        return Err(crate::SyncError::new(
            "the vault id is empty or too long. Set one in Settings → Sync",
        ));
    }
    if id == "." || id == ".." || id.starts_with('.') {
        return Err(crate::SyncError::new(
            "the vault id cannot start with a dot. Set one in Settings → Sync",
        ));
    }
    if id.chars().any(|ch| {
        ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
    }) {
        return Err(crate::SyncError::new(
            "the vault id contains a character a folder name cannot. Set one in Settings → Sync",
        ));
    }
    Ok(id.to_string())
}

/// Content files always sync. `.notia` syncs only when `include_config` is
/// set, and even then the plugin folder, the sync history, and the sync
/// settings stay local.
pub fn is_synced(path: &str, include_config: bool) -> bool {
    let Some(path) = parse_rel_path(path) else {
        return false;
    };
    let text = path.as_str();
    if !text.split('/').any(|segment| segment.starts_with('.')) {
        return true;
    }
    include_config && is_config(text)
}

fn is_config(path: &str) -> bool {
    let Some(rest) = path.strip_prefix(&format!("{CONFIG_DIR}/")) else {
        return false;
    };
    if rest.is_empty() || rest.split('/').any(|segment| segment.starts_with('.')) {
        return false;
    }
    if rest == "sync" || rest.starts_with("sync/") {
        return false;
    }
    if rest == "plugins" || rest.starts_with("plugins/") {
        return false;
    }
    if path == SETTINGS_FILE {
        return false;
    }
    true
}

pub(crate) fn encode_path(path: &str) -> String {
    path.split('/')
        .map(encode_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn encode_segment(segment: &str) -> String {
    let mut out = String::new();
    for byte in segment.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push(HEX[(byte >> 4) as usize] as char);
                out.push(HEX[(byte & 0x0f) as usize] as char);
            }
        }
    }
    out
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// Percent-decode a URL path and accept it only when it is a safe relative
/// path. `+` is left encoded: in a path it is not a space.
#[cfg(any(test, feature = "server"))]
pub(crate) fn decode_path(input: &str) -> Option<String> {
    let raw = input.as_bytes();
    let mut bytes = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        match raw[index] {
            b'%' => {
                if index + 2 >= raw.len() {
                    return None;
                }
                let hex = std::str::from_utf8(&raw[index + 1..index + 3]).ok()?;
                bytes.push(u8::from_str_radix(hex, 16).ok()?);
                index += 3;
            }
            byte if byte < 0x20 || byte == 0x7f => return None,
            byte => {
                bytes.push(byte);
                index += 1;
            }
        }
    }
    let text = String::from_utf8(bytes).ok()?;
    parse_rel_path(&text)?;
    Some(text)
}

pub(crate) fn conflict_path(path: &str, stamp: &str, taken: &mut BTreeSet<String>) -> String {
    let vault = VaultPath::new(path);
    let parent = vault.parent().unwrap_or_else(VaultPath::root);
    let stem = vault.file_stem();
    let extension = vault.extension();
    for n in 0..1000 {
        let mark = if n == 0 {
            format!("sync-conflict-{stamp}")
        } else {
            format!("sync-conflict-{stamp}-{}", n + 1)
        };
        let name = match extension {
            Some(extension) => format!("{stem}.{mark}.{extension}"),
            None => format!("{stem}.{mark}"),
        };
        let candidate = parent.join(&name).as_str().to_string();
        if taken.insert(candidate.clone()) {
            return candidate;
        }
    }
    let fallback = parent
        .join(&format!("{stem}.sync-conflict-{stamp}-overflow"))
        .as_str()
        .to_string();
    taken.insert(fallback.clone());
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_reject_escapes() {
        assert!(parse_rel_path("Folder/Note.md").is_some());
        assert!(parse_rel_path(".notia/app.json").is_some());
        assert!(parse_rel_path("My Note.md").is_some());
        for raw in [
            "",
            "/etc/passwd",
            "../secret",
            "foo/../../secret",
            "foo/../secret",
            "foo\\bar",
            "foo//bar",
            "foo/./bar",
            ".",
            "..",
            "foo/",
        ] {
            assert!(parse_rel_path(raw).is_none(), "{raw}");
        }
    }

    #[test]
    fn config_sync_is_optional_and_skips_local_state() {
        assert!(is_synced("Welcome.md", false));
        assert!(is_synced("attachments/pic.png", false));
        assert!(!is_synced(".notia/app.json", false));
        assert!(!is_synced(".trash/old.md", false));
        assert!(!is_synced(".git/config", true));
        assert!(is_synced(".notia/app.json", true));
        assert!(is_synced(".notia/themes/midnight/theme.json", true));
        assert!(is_synced(".notia/plugin-data/other.json", true));
        assert!(!is_synced(".notia/plugin-data/sync.json", true));
        assert!(!is_synced(".notia/sync/state.json", true));
        assert!(!is_synced(".notia/plugins/demo/main.js", true));
        assert!(!is_synced("notes/.hidden.md", true));
    }

    #[test]
    fn vault_ids_are_single_folder_names() {
        assert_eq!(require_vault_id("  Notes ").unwrap(), "Notes");
        assert!(require_vault_id("My Notes").is_ok());
        assert!(require_vault_id("笔记").is_ok());
        assert!(require_vault_id("").is_err());
        assert!(require_vault_id(".").is_err());
        assert!(require_vault_id("..").is_err());
        assert!(require_vault_id(".hidden").is_err());
        assert!(require_vault_id("a/b").is_err());
        assert!(require_vault_id("a\\b").is_err());
    }

    #[test]
    fn percent_encoding_round_trips() {
        for path in [
            "Welcome.md",
            "Folder/My Note.md",
            "笔记/甲.md",
            "a b/c+d.md",
        ] {
            let decoded = decode_path(&encode_path(path)).unwrap();
            assert_eq!(decoded, path);
        }
        assert!(decode_path("%2e%2e/%2e%2e/secret").is_none());
        assert!(decode_path("foo/%2e%2e/secret").is_none());
    }

    #[test]
    fn conflict_names_keep_the_extension() {
        let mut taken = BTreeSet::from([
            "Folder/Note.md".to_string(),
            "Folder/Note.sync-conflict-T.md".to_string(),
        ]);
        let path = conflict_path("Folder/Note.md", "T", &mut taken);
        assert_eq!(path, "Folder/Note.sync-conflict-T-2.md");
        let again = conflict_path("Folder/Note.md", "T", &mut taken);
        assert_eq!(again, "Folder/Note.sync-conflict-T-3.md");
    }
}
