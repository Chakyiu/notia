//! The plugin API of Notia.
//!
//! Everything a user sees beyond the bare window frame — the file explorer,
//! search, outline, backlinks, the status bar counters, and every theme — is
//! contributed by a plugin. A plugin implements [`Plugin`] and, when it is
//! enabled, registers its contributions through a [`PluginContext`]:
//!
//! ```ignore
//! struct HelloPlugin;
//!
//! impl Plugin for HelloPlugin {
//!     fn manifest(&self) -> PluginManifest {
//!         PluginManifest::new("hello", "Hello").with_description("Says hello.")
//!     }
//!
//!     fn activate(&self, ctx: &mut PluginContext, _: &mut App) {
//!         ctx.add_command(
//!             Command::new("say-hello", "Say hello", |window, cx| {
//!                 window.push_notification("Hello from a plugin!", cx);
//!             })
//!             .with_keybinding("secondary-shift-h"),
//!         );
//!     }
//! }
//! ```
//!
//! Disabling a plugin removes everything it registered and drops the
//! subscriptions and tasks it handed to the context, so plugins rarely need
//! cleanup code of their own.
//!
//! Plugins reach the vault through [`Notia::vault`], follow the open note
//! through [`Notia::workspace`], and drive the interface through the
//! functions in [`workspace`]. Those functions also read and edit the text
//! of open notes, as described in [`editor`].

extern crate gpui_kit as gpui;

pub mod actions;
mod context;
mod contribution;
pub mod editor;
mod host;
pub mod hotkeys;
mod icons;
mod manifest;
mod settings;
pub mod settings_tab;
mod vault;
pub mod workspace;

use gpui_kit::{App, AppContext as _, Entity, Global};

pub use context::PluginContext;
pub use contribution::{
    Command, NavigationIcon, Panel, PanelPlacement, RibbonItem, StatusItem, TabBarItem,
};
pub use editor::{
    EditorChange, EditorCommand, EditorId, EditorSelection, EditorSnapshot, EditorTransaction,
};
pub use host::PluginHost;
pub use hotkeys::PaletteCommand;
pub use icons::entry_icon;
pub use manifest::{PluginKind, PluginManifest};
pub use notia_core as core;
pub use notia_theme as theme;
pub use settings::{AppSettings, EditorSettings, FileSettings, PaletteSettings, ViewMode};
pub use settings_tab::{Setting, SettingControl, SettingValue, SettingsSection, SettingsTab};
pub use vault::{VaultEvent, VaultModel};
pub use workspace::{ActiveNote, OpenOptions, WorkspaceEvent, WorkspaceHandle, WorkspaceModel};

/// A unit of functionality that can be enabled and disabled at runtime.
pub trait Plugin: 'static {
    fn manifest(&self) -> PluginManifest;

    /// Registers the plugin's contributions. Called every time the plugin is
    /// enabled; everything registered through `ctx` is removed when it is
    /// disabled.
    fn activate(&self, ctx: &mut PluginContext, cx: &mut App);
}

/// The application models plugins work with.
pub struct Notia {
    vault: Entity<VaultModel>,
    workspace: Entity<WorkspaceModel>,
}

impl Global for Notia {}

impl Notia {
    /// The open vault: its files, the link index and file operations.
    pub fn vault(cx: &App) -> Entity<VaultModel> {
        cx.global::<Self>().vault.clone()
    }

    /// What the workspace shows, such as the active note.
    pub fn workspace(cx: &App) -> Entity<WorkspaceModel> {
        cx.global::<Self>().workspace.clone()
    }

    /// What a plugin saved with [`Notia::save_plugin_data`], or the default
    /// when it saved nothing.
    pub fn load_plugin_data<T: serde::de::DeserializeOwned + Default>(
        plugin_id: &str,
        cx: &App,
    ) -> T {
        read_json(&Self::plugin_data_path(plugin_id, cx))
    }

    /// Keeps a plugin's settings in the vault, in
    /// `.notia/plugin-data/<plugin id>.json`.
    pub fn save_plugin_data<T: serde::Serialize>(plugin_id: &str, data: &T, cx: &App) {
        write_json(&Self::plugin_data_path(plugin_id, cx), data);
    }

    fn plugin_data_path(plugin_id: &str, cx: &App) -> std::path::PathBuf {
        Self::vault(cx)
            .read(cx)
            .vault()
            .config_dir()
            .join("plugin-data")
            .join(format!("{plugin_id}.json"))
    }
}

/// Opens `vault` and sets up settings, the plugin host and the shared
/// models. Plugins are registered afterwards with [`PluginHost::register`].
pub fn init(vault: notia_core::Vault, cx: &mut App) {
    let config_dir = vault.config_dir();
    notia_theme::ThemeCatalog::load_appearance(config_dir.join("appearance.json"), cx);
    notia_theme::ThemeCatalog::watch_folder(config_dir.join("themes"), cx);
    AppSettings::load(config_dir.join("app.json"), cx);
    PluginHost::init(config_dir.join("plugins.json"), cx);

    let vault = cx.new(|cx| VaultModel::new(vault, cx));
    let workspace = cx.new(|_| WorkspaceModel::default());
    cx.set_global(Notia { vault, workspace });
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned + Default>(path: &std::path::Path) -> T {
    match std::fs::read_to_string(path) {
        Ok(json) => serde_json::from_str(&json).unwrap_or_else(|error| {
            log::warn!("ignoring the invalid {}: {error}", path.display());
            T::default()
        }),
        Err(_) => T::default(),
    }
}

pub(crate) fn write_json<T: serde::Serialize>(path: &std::path::Path, value: &T) {
    let result = serde_json::to_string_pretty(value)
        .map_err(anyhow::Error::from)
        .and_then(|json| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            Ok(std::fs::write(path, json)?)
        });
    if let Err(error) = result {
        log::error!("could not save {}: {error:#}", path.display());
    }
}
