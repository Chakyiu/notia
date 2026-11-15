//! JSON operations over one open vault.

use std::{fs, path::PathBuf, sync::Mutex};

use notia_core::{IndexedNote, SearchQuery, Vault, VaultIndex, VaultPath, search};
use notia_sync::{MAX_FILE_BYTES, SETTINGS_FILE, SyncOptions, sync_vault};
use serde::Deserialize;
use serde_json::{Value, json};

const WELCOME: &str = "\
# Welcome to Notia

Notes are plain Markdown files. This phone keeps them in the app. Sync copies them to a server you run, the same one Notia on your computer uses.

On the Sync tab, enter the server address and token from the computer, and the vault id. The vault id is the vault folder's name, unless you set a different one in Settings → Sync.

#welcome
";

struct Session {
    vault: Vault,
    index: VaultIndex,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

#[derive(Deserialize)]
struct Request {
    op: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    query: String,
    #[serde(default)]
    server: String,
    #[serde(default)]
    token: String,
    #[serde(default)]
    vault_id: String,
    #[serde(default)]
    include_config: bool,
    #[serde(default)]
    auto: bool,
}

/// Runs one request. The response is always a JSON object with `ok`.
pub fn handle(request: &str) -> String {
    match respond(request) {
        Ok(value) => {
            let mut object = json!({"ok": true});
            if let (Value::Object(response), Value::Object(extra)) = (&mut object, value) {
                response.extend(extra);
            }
            object.to_string()
        }
        Err(error) => json!({"ok": false, "error": error}).to_string(),
    }
}

fn respond(request: &str) -> Result<Value, String> {
    let request: Request =
        serde_json::from_str(request).map_err(|_| "The request was not valid.".to_string())?;
    let mut session = SESSION.lock().unwrap_or_else(|error| error.into_inner());
    match request.op.as_str() {
        "open" => open(&mut session, &request.path),
        "list" => {
            let session = opened(&mut session)?;
            refresh(session)?;
            Ok(json!({"notes": note_list(session)}))
        }
        "read" => {
            let session = opened(&mut session)?;
            let path = VaultPath::new(&request.path);
            let text = session
                .vault
                .read_note(&path)
                .map_err(|error| format!("Could not open {}: {error}", path.as_str()))?;
            Ok(json!({"path": path.as_str(), "title": path.file_stem(), "text": text}))
        }
        "write" => {
            let session = opened(&mut session)?;
            let path = VaultPath::new(&request.path);
            session
                .vault
                .write_note(&path, &request.text)
                .map_err(|error| format!("Could not save {}: {error}", path.as_str()))?;
            refresh(session)?;
            Ok(json!({}))
        }
        "create" => {
            let session = opened(&mut session)?;
            let title = if request.title.trim().is_empty() {
                "Untitled"
            } else {
                request.title.trim()
            };
            let path = session
                .vault
                .create_note(&VaultPath::root(), title, "")
                .map_err(|error| format!("Could not create the note: {error}"))?;
            refresh(session)?;
            Ok(json!({"path": path.as_str(), "title": path.file_stem()}))
        }
        "trash" => {
            let session = opened(&mut session)?;
            let path = VaultPath::new(&request.path);
            session
                .vault
                .trash(&path)
                .map_err(|error| format!("Could not delete {}: {error}", path.as_str()))?;
            refresh(session)?;
            Ok(json!({}))
        }
        "search" => {
            let session = opened(&mut session)?;
            refresh(session)?;
            Ok(json!({"matches": search_notes(session, &request.query)}))
        }
        "settings" => {
            let session = opened(&mut session)?;
            Ok(settings_json(&load_settings(session)))
        }
        "save_settings" => {
            let session = opened(&mut session)?;
            let mut settings = load_settings(session);
            settings.server_url = request.server.trim().to_string();
            settings.token = request.token.trim().to_string();
            settings.vault_id = request.vault_id.trim().to_string();
            settings.include_config = request.include_config;
            settings.auto = request.auto;
            save_settings(session, &settings)?;
            Ok(settings_json(&settings))
        }
        "sync" => {
            let session = opened(&mut session)?;
            let summary = sync_now(session)?;
            refresh(session)?;
            Ok(json!({"summary": summary}))
        }
        _ => Err("Unknown request.".into()),
    }
}

fn open(slot: &mut Option<Session>, path: &str) -> Result<Value, String> {
    if path.trim().is_empty() {
        return Err("The vault folder is missing.".into());
    }
    let vault = Vault::open(path).map_err(|error| format!("Could not open the vault: {error}"))?;
    let mut session = Session {
        vault,
        index: VaultIndex::default(),
    };
    refresh(&mut session)?;
    if session.index.is_empty()
        && session
            .vault
            .scan()
            .is_ok_and(|tree| tree.entries().is_empty())
    {
        session
            .vault
            .create_note(&VaultPath::root(), "Welcome", WELCOME)
            .map_err(|error| format!("Could not create the starter note: {error}"))?;
        refresh(&mut session)?;
    }
    let notes = note_list(&session);
    *slot = Some(session);
    Ok(json!({"notes": notes}))
}

fn opened(slot: &mut Option<Session>) -> Result<&mut Session, String> {
    slot.as_mut()
        .ok_or_else(|| "The vault is not open.".to_string())
}

fn refresh(session: &mut Session) -> Result<(), String> {
    let tree = session
        .vault
        .scan()
        .map_err(|error| format!("Could not read the vault: {error}"))?;
    let known = session.index.snapshot();
    let changes = VaultIndex::read_changes(&session.vault, &tree, &known);
    session.index.apply(changes);
    Ok(())
}

fn note_list(session: &Session) -> Vec<Value> {
    let mut notes: Vec<&IndexedNote> = session.index.notes().collect();
    notes.sort_by(|left, right| {
        left.title()
            .to_lowercase()
            .cmp(&right.title().to_lowercase())
            .then_with(|| left.path.as_str().cmp(right.path.as_str()))
    });
    notes
        .into_iter()
        .map(|note| json!({"path": note.path.as_str(), "title": note.title()}))
        .collect()
}

fn search_notes(session: &Session, query: &str) -> Vec<Value> {
    let query = SearchQuery::parse(query);
    if query.is_empty() {
        return Vec::new();
    }
    let notes: Vec<&IndexedNote> = session.index.notes().collect();
    search(notes, &query, 1)
        .into_iter()
        .take(50)
        .map(|found| {
            json!({
                "path": found.path.as_str(),
                "title": found.path.file_stem(),
                "line": found.lines.first().map(|line| line.text.as_str()).unwrap_or(""),
            })
        })
        .collect()
}

#[derive(Clone, serde::Serialize)]
struct SyncSettings {
    server_url: String,
    token: String,
    vault_id: String,
    auto: bool,
    interval_seconds: u64,
    show_in_tab_bar: bool,
    include_config: bool,
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self {
            server_url: String::new(),
            token: String::new(),
            vault_id: String::new(),
            auto: false,
            interval_seconds: 30,
            show_in_tab_bar: false,
            include_config: false,
        }
    }
}

impl<'de> Deserialize<'de> for SyncSettings {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Stored {
            #[serde(default)]
            server_url: String,
            #[serde(default)]
            token: String,
            #[serde(default)]
            vault_id: String,
            #[serde(default)]
            auto: bool,
            interval_seconds: Option<u64>,
            interval_minutes: Option<u64>,
            #[serde(default)]
            show_in_tab_bar: bool,
            #[serde(default)]
            include_config: bool,
        }
        let stored = Stored::deserialize(deserializer)?;
        let interval_seconds = stored
            .interval_seconds
            .or_else(|| {
                stored
                    .interval_minutes
                    .map(|minutes| minutes.saturating_mul(60))
            })
            .unwrap_or(30)
            .clamp(1, 86_400);
        Ok(Self {
            server_url: stored.server_url,
            token: stored.token,
            vault_id: stored.vault_id,
            auto: stored.auto,
            interval_seconds,
            show_in_tab_bar: stored.show_in_tab_bar,
            include_config: stored.include_config,
        })
    }
}

fn settings_file(vault: &Vault) -> PathBuf {
    vault.root().join(SETTINGS_FILE)
}

fn load_settings(session: &Session) -> SyncSettings {
    let Ok(text) = fs::read_to_string(settings_file(&session.vault)) else {
        return SyncSettings::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn save_settings(session: &Session, settings: &SyncSettings) -> Result<(), String> {
    let path = settings_file(&session.vault);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)
            .map_err(|error| format!("Could not save sync settings: {error}"))?;
    }
    let json = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("Could not save sync settings: {error}"))?;
    fs::write(path, json).map_err(|error| format!("Could not save sync settings: {error}"))
}

fn settings_json(settings: &SyncSettings) -> Value {
    json!({
        "server": settings.server_url,
        "token": settings.token,
        "vault_id": settings.vault_id,
        "include_config": settings.include_config,
        "auto": settings.auto,
    })
}

fn sync_now(session: &Session) -> Result<String, String> {
    let settings = load_settings(session);
    if settings.server_url.is_empty() || settings.token.is_empty() {
        return Err("Enter the server address and token first.".into());
    }
    if settings.vault_id.is_empty() {
        return Err(
            "Enter the vault id from your computer. It is the vault folder's name unless you set one in Settings → Sync."
                .into(),
        );
    }
    let report = sync_vault(&SyncOptions {
        root: session.vault.root().to_path_buf(),
        server_url: settings.server_url,
        token: settings.token,
        vault_id: settings.vault_id,
        include_config: settings.include_config,
        max_bytes: MAX_FILE_BYTES,
    })
    .map_err(|error| error.to_string())?;
    Ok(report.summary())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn call(request: Value) -> Value {
        serde_json::from_str(&handle(&request.to_string())).unwrap()
    }

    #[test]
    fn a_phone_can_create_search_and_keep_a_note() {
        let dir = tempfile::tempdir().unwrap();
        let opened = call(json!({"op": "open", "path": dir.path()}));
        assert_eq!(opened["ok"], true, "{opened}");
        assert_eq!(opened["notes"][0]["path"], "Welcome.md");

        let read = call(json!({"op": "read", "path": "Welcome.md"}));
        assert_eq!(read["ok"], true, "{read}");
        assert!(read["text"].as_str().unwrap().contains("vault id"));

        let created = call(json!({"op": "create", "title": "Market"}));
        assert_eq!(created["path"], "Market.md");
        let written = call(json!({
            "op": "write",
            "path": "Market.md",
            "text": "Buy #apples and pears.\n",
        }));
        assert_eq!(written["ok"], true, "{written}");

        let found = call(json!({"op": "search", "query": "pears"}));
        assert_eq!(found["matches"][0]["path"], "Market.md");

        let outside = call(json!({"op": "read", "path": "../../etc/passwd"}));
        assert_eq!(outside["ok"], false);

        let saved = call(json!({
            "op": "save_settings",
            "server": "http://192.168.1.20:8787",
            "token": "secret",
            "vault_id": "Notes",
            "include_config": false,
        }));
        assert_eq!(saved["vault_id"], "Notes");
        let loaded = call(json!({"op": "settings"}));
        assert_eq!(loaded["server"], "http://192.168.1.20:8787");
        assert_eq!(loaded["token"], "secret");

        // Clearing the server must fail before any network call.
        let _ = call(json!({
            "op": "save_settings",
            "server": "",
            "token": "secret",
            "vault_id": "Notes",
        }));
        let refused = call(json!({"op": "sync"}));
        assert_eq!(refused["ok"], false, "{refused}");
        assert!(
            refused["error"].as_str().unwrap().contains("server"),
            "{refused}"
        );

        let removed = call(json!({"op": "trash", "path": "Market.md"}));
        assert_eq!(removed["ok"], true, "{removed}");
        let listed = call(json!({"op": "list"}));
        let paths: Vec<_> = listed["notes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|note| note["path"].as_str())
            .collect();
        assert!(paths.contains(&"Welcome.md"));
        assert!(!paths.contains(&"Market.md"));
    }

    #[test]
    fn a_request_that_is_not_json_fails() {
        let response = call_raw("nope");
        assert_eq!(response["ok"], false);
    }

    fn call_raw(request: &str) -> Value {
        serde_json::from_str(&handle(request)).unwrap()
    }
}
