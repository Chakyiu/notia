//! Community plugins: JavaScript plugins installed in a vault, like
//! Obsidian's.
//!
//! A community plugin is a folder in `<vault>/.notia/plugins`, or a link
//! there to a folder elsewhere, holding a `manifest.json` and an entry
//! module, `main.js` unless the manifest's `main` names another:
//!
//! ```js
//! import { Plugin, Notice } from "notia";
//!
//! export default class Hello extends Plugin {
//!   onload() {
//!     this.addCommand({ id: "hello", name: "Say hello", callback: () => new Notice("Hello!") });
//!   }
//! }
//! ```
//!
//! The *Community plugins* core plugin lists every plugin folder by reading
//! its manifest, and nothing else. A plugin's code is loaded only when it is
//! enabled: it then runs in a QuickJS runtime of its own, with limits on its
//! time and memory, and reaches Notia only through the API described in
//! `notia.d.ts`. Disabling the plugin calls its `onunload`, removes
//! everything it registered and drops its runtime. Saving any of its files
//! reloads it.

mod file_icons;
mod host;
mod loader;
mod manifest;
mod offsets;
mod runtime;
mod scope;
mod settings;
mod view;

use gpui_kit::{App, AssetSource as _, assets::IconName as Lucide, component::Icon};
use notia_plugin::PluginHost;

pub use loader::CommunityPluginsPlugin;

/// TypeScript declarations of the plugin API, which give editors
/// completions while writing a plugin.
pub const TYPE_DEFINITIONS: &str = include_str!("../notia.d.ts");

/// Registers the core plugin that loads community plugins.
pub fn register(cx: &mut App) {
    PluginHost::register(CommunityPluginsPlugin, cx);
}

/// A Lucide icon by name, such as `dices` or `chart-pie`, or a puzzle piece
/// for names Notia does not know.
fn icon(name: Option<&str>) -> Icon {
    name.map(|name| format!("icons/{name}.svg"))
        .filter(|path| {
            gpui_kit::assets::AllAssets
                .load(path)
                .is_ok_and(|asset| asset.is_some())
        })
        .map(|path| Icon::empty().path(path))
        .unwrap_or_else(|| Icon::new(Lucide::Puzzle))
}
