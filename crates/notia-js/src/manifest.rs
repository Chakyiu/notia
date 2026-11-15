//! Finding the community plugins of a vault without running any of them.

use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use gpui_kit::SharedString;
use notia_plugin::core::VaultPath;
use serde::Deserialize;
use serde_json::Value;

/// The file Notia reads to list a plugin. Unknown fields, such as
/// Obsidian's `minAppVersion`, are ignored.
#[derive(Deserialize)]
struct ManifestFile {
    id: String,
    name: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    main: Option<String>,
}

/// An installed plugin, as described by its `manifest.json`.
#[derive(Debug)]
pub(crate) struct PluginInfo {
    pub(crate) id: SharedString,
    pub(crate) name: SharedString,
    pub(crate) version: SharedString,
    pub(crate) author: SharedString,
    pub(crate) description: SharedString,
    pub(crate) dir: PathBuf,
    /// The entry module, relative to `dir`.
    pub(crate) main: String,
    /// The manifest the plugin sees as `this.manifest`.
    pub(crate) manifest_json: String,
}

/// A plugin folder whose manifest could not be used.
#[derive(Debug)]
pub(crate) struct BrokenPlugin {
    /// The folder name, which stands in for the id.
    pub(crate) id: SharedString,
    pub(crate) error: SharedString,
}

/// Changes whenever a plugin's manifest or code changes, so that a
/// different fingerprint means the plugin must be reloaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    manifest: Option<String>,
    newest: Option<SystemTime>,
}

pub(crate) struct Found {
    /// The folder in the plugins folder, which may be a link.
    pub(crate) folder: PathBuf,
    pub(crate) plugin: Result<PluginInfo, BrokenPlugin>,
    pub(crate) fingerprint: Fingerprint,
}

impl Found {
    pub(crate) fn id(&self) -> &SharedString {
        match &self.plugin {
            Ok(plugin) => &plugin.id,
            Err(broken) => &broken.id,
        }
    }
}

/// Plugins write `data.json` whenever their settings change, and a `.css`
/// file is a stylesheet the plugin reads itself. Neither is the plugin's
/// code, so saving one must not reload it.
pub(crate) fn is_data_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name == "data.json" || name.ends_with(".notia-tmp") || name.ends_with(".css")
        })
}

/// Reads the manifest of every plugin folder in `dir`.
pub(crate) fn discover(dir: &Path) -> Vec<Found> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut folders: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && !is_hidden(path))
        .collect();
    folders.sort();
    folders
        .into_iter()
        .map(|folder| {
            let manifest = fs::read_to_string(folder.join("manifest.json"));
            let fingerprint = Fingerprint {
                manifest: manifest.as_ref().ok().cloned(),
                newest: newest_change(&folder, 0),
            };
            let plugin = manifest
                .map_err(|error| format!("Could not read manifest.json: {error}"))
                .and_then(|text| read_manifest(&folder, &text))
                .map_err(|error| BrokenPlugin {
                    id: folder_name(&folder).into(),
                    error: error.into(),
                });
            Found {
                folder,
                plugin,
                fingerprint,
            }
        })
        .collect()
}

fn read_manifest(folder: &Path, text: &str) -> Result<PluginInfo, String> {
    let file: ManifestFile =
        serde_json::from_str(text).map_err(|error| format!("manifest.json is invalid: {error}"))?;
    let id = file.id.trim();
    if id.is_empty() || id.contains([':', '/', '\\']) || id.contains(char::is_whitespace) {
        return Err(format!(
            "“{id}” is not a valid plugin id: use letters, digits and dashes"
        ));
    }
    if file.name.trim().is_empty() {
        return Err("manifest.json has no name".into());
    }
    // A path inside the plugin folder; `VaultPath` drops `..` segments.
    let main = VaultPath::new(file.main.as_deref().unwrap_or("main.js"));
    if main.is_root() {
        return Err("manifest.json has an empty main".into());
    }

    let mut manifest: Value = serde_json::from_str(text).unwrap_or_default();
    if let Value::Object(fields) = &mut manifest {
        let dir = format!(".notia/plugins/{}", folder_name(folder));
        fields.insert("dir".into(), dir.into());
    }
    Ok(PluginInfo {
        id: id.to_string().into(),
        name: file.name.trim().to_string().into(),
        version: file.version.unwrap_or_default().into(),
        author: file.author.unwrap_or_default().into(),
        description: file.description.unwrap_or_default().into(),
        dir: folder.to_path_buf(),
        main: main.as_str().to_string(),
        manifest_json: manifest.to_string(),
    })
}

/// The newest modification time of the files in `dir` that make up the
/// plugin, looking a few folders deep.
fn newest_change(dir: &Path, depth: usize) -> Option<SystemTime> {
    let entries = fs::read_dir(dir).ok()?;
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| !is_hidden(path) && !is_data_file(path))
        .filter_map(|path| {
            if path.is_dir() {
                (depth < 3 && folder_name(&path) != "node_modules")
                    .then(|| newest_change(&path, depth + 1))
                    .flatten()
            } else {
                fs::metadata(&path).and_then(|meta| meta.modified()).ok()
            }
        })
        .max()
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn is_hidden(path: &Path) -> bool {
    folder_name(path).starts_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_manifests_and_reports_broken_ones() {
        let dir = std::env::temp_dir().join(format!("notia-js-manifest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("good")).unwrap();
        fs::create_dir_all(dir.join("broken")).unwrap();
        fs::write(
            dir.join("good/manifest.json"),
            r#"{ "id": "good", "name": "Good", "version": "1.2.0", "main": "dist/../main.js", "minAppVersion": "1.0" }"#,
        )
        .unwrap();
        fs::write(dir.join("broken/manifest.json"), r#"{ "id": "has space" }"#).unwrap();

        let found = discover(&dir);
        assert_eq!(found.len(), 2);
        let broken = found[0].plugin.as_ref().unwrap_err();
        assert_eq!(broken.id.as_ref(), "broken");
        let good = found[1].plugin.as_ref().unwrap();
        assert_eq!((good.id.as_ref(), good.version.as_ref()), ("good", "1.2.0"));
        assert_eq!(good.main, "dist/main.js");
        let manifest: Value = serde_json::from_str(&good.manifest_json).unwrap();
        assert_eq!(manifest["dir"], ".notia/plugins/good");
        assert_eq!(manifest["minAppVersion"], "1.0");

        let before = found[1].fingerprint.clone();
        fs::write(dir.join("good/data.json"), "{}").unwrap();
        fs::write(dir.join("good/theme.css"), ".theme { background: #111; }").unwrap();
        assert_eq!(discover(&dir)[1].fingerprint, before);
        let _ = fs::remove_dir_all(&dir);
    }
}
