//! Theme plugins.
//!
//! A theme plugin is a JSON file that controls how Notia looks: every color
//! of the interface, syntax colors for the Markdown editor, fonts, corner
//! radii, and the reading typography of notes. Themes come from three places:
//! the themes bundled with the app, code plugins that contribute themes, and
//! the vault's `.notia/themes` folder, which is watched so a theme reloads as
//! soon as its file is saved.
//!
//! The file format is a GPUI Component theme set with a manifest on top and
//! optional `notia` typography keys per variant:
//!
//! ```json
//! {
//!   "id": "paper",
//!   "name": "Paper",
//!   "version": "1.0.0",
//!   "author": "You",
//!   "themes": [
//!     {
//!       "mode": "light",
//!       "colors": { "background": "#fdfcf8", "primary.background": "#b4552d" },
//!       "highlight": { "syntax": { "title": { "color": "#b4552d", "font_weight": 700 } } },
//!       "notia": { "editor.font.size": 17, "editor.line_height": 1.7 }
//!     }
//!   ]
//! }
//! ```

mod appearance;
mod catalog;
mod pack;

pub use appearance::{Appearance, ColorScheme, DEFAULT_THEME_ID, EditorAppearance};
pub use catalog::{ThemeCatalog, ThemeLoadError};
pub use pack::{EditorTheme, ThemeManifest, ThemePack, ThemeSource, ThemeVariant};

/// Installs the theme catalog. Call after `gpui_kit::init`.
pub fn init(cx: &mut gpui_kit::App) {
    ThemeCatalog::init(cx);
}
