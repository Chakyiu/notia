//! The core plugins of Notia and its bundled themes. Every one of them uses
//! only the public plugin API, the same one a community plugin would.

extern crate gpui_kit as gpui;

mod backlinks;
mod daily_notes;
mod date_time;
mod file_explorer;
mod file_icons;
mod outline;
mod search;
mod tags;
mod theme_gallery;
mod ui;
mod word_count;

use gpui_kit::App;
use notia_plugin::{
    PluginHost,
    theme::{ThemeCatalog, ThemePack, ThemeSource},
};

pub use backlinks::BacklinksPlugin;
pub use daily_notes::DailyNotesPlugin;
pub use date_time::DateTimePlugin;
pub use file_explorer::FileExplorerPlugin;
pub use file_icons::FileIconsPlugin;
pub use outline::OutlinePlugin;
pub use search::SearchPlugin;
pub use tags::TagsPlugin;
pub use theme_gallery::ThemeGalleryPlugin;
pub use word_count::WordCountPlugin;

/// The default theme, which is always installed.
const NOTIA_THEME: &str = include_str!("../themes/notia.json");

/// Installs the default theme. Safe to call before a vault exists, so the
/// first-run window can use it.
pub fn install_bundled_theme(cx: &mut App) {
    match ThemePack::from_json(NOTIA_THEME, ThemeSource::Bundled) {
        Ok(pack) => ThemeCatalog::register(pack, cx),
        Err(error) => log::error!("the bundled theme is invalid: {error:#}"),
    }
}

/// Installs the default theme and registers every core plugin.
pub fn register(cx: &mut App) {
    install_bundled_theme(cx);

    PluginHost::register(FileExplorerPlugin, cx);
    PluginHost::register(FileIconsPlugin, cx);
    PluginHost::register(SearchPlugin, cx);
    PluginHost::register(OutlinePlugin, cx);
    PluginHost::register(BacklinksPlugin, cx);
    PluginHost::register(TagsPlugin, cx);
    PluginHost::register(WordCountPlugin, cx);
    PluginHost::register(DailyNotesPlugin, cx);
    PluginHost::register(DateTimePlugin, cx);
    PluginHost::register(ThemeGalleryPlugin, cx);
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::{ThemeMode, try_parse_color};
    use notia_plugin::theme::{ThemePack, ThemeSource};

    #[test]
    fn bundled_themes_are_valid() {
        let themes =
            std::iter::once(super::NOTIA_THEME).chain(crate::theme_gallery::THEMES.iter().copied());
        for json in themes {
            let pack = ThemePack::from_json(json, ThemeSource::Bundled).unwrap();
            assert!(pack.has_mode(ThemeMode::Light) || pack.has_mode(ThemeMode::Dark));

            let file: serde_json::Value = serde_json::from_str(json).unwrap();
            for variant in file["themes"].as_array().unwrap() {
                for (key, value) in variant["colors"].as_object().unwrap() {
                    let color = value.as_str().unwrap();
                    assert!(
                        try_parse_color(color).is_ok(),
                        "{}: {key} = {color}",
                        pack.name()
                    );
                }
            }
        }
    }
}
