use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use gpui_kit::{App, Global};
use serde::{Deserialize, Serialize};

/// How a note is shown: source, rendered, or both at once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    #[default]
    Editing,
    /// Markdown on the left, the rendered note on the right.
    Half,
    Reading,
}

impl ViewMode {
    /// The next mode. Split is included only when `split` is on.
    pub fn next(self, split: bool) -> Self {
        match (self, split) {
            (Self::Editing, true) => Self::Half,
            (Self::Editing, false) | (Self::Half, _) => Self::Reading,
            (Self::Reading, _) => Self::Editing,
        }
    }

    /// Whether this mode shows the rendered note, which must stay current.
    pub fn shows_preview(self) -> bool {
        matches!(self, Self::Half | Self::Reading)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorSettings {
    /// How newly opened notes are shown.
    pub default_view: ViewMode,
    /// Whether the note button also offers a split of the source and the preview.
    pub split_view: bool,
    pub line_numbers: bool,
    /// How long typing must pause before a note is saved.
    pub autosave_delay_ms: u64,
    pub tab_size: usize,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            default_view: ViewMode::Editing,
            split_view: false,
            line_numbers: false,
            autosave_delay_ms: 800,
            tab_size: 4,
        }
    }
}

impl EditorSettings {
    /// The view a new tab opens in. Split is left out while that mode is off.
    pub fn opening_view(&self) -> ViewMode {
        if self.default_view == ViewMode::Half && !self.split_view {
            ViewMode::Editing
        } else {
            self.default_view
        }
    }
}

fn default_attachment_folder() -> String {
    crate::core::ATTACHMENTS_DIR.to_string()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileSettings {
    /// The vault folder new notes are created in; empty for the vault root.
    pub new_note_folder: String,
    /// The vault folder pasted images are saved in; empty for the vault root.
    #[serde(default = "default_attachment_folder")]
    pub attachment_folder: String,
    /// Whether deleting asks for confirmation first.
    pub confirm_trash: bool,
}

impl Default for FileSettings {
    fn default() -> Self {
        Self {
            new_note_folder: String::new(),
            attachment_folder: default_attachment_folder(),
            confirm_trash: true,
        }
    }
}

/// Which commands the palette lists, and the shortcuts that run them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaletteSettings {
    /// Whether the shortcut opens the command palette.
    pub enabled: bool,
    /// Command ids left out of the palette. Their shortcuts still run them.
    pub hidden: BTreeSet<String>,
    /// Replacement shortcuts, by command id. An empty string clears the default.
    pub hotkeys: BTreeMap<String, String>,
}

impl Default for PaletteSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            hidden: BTreeSet::new(),
            hotkeys: BTreeMap::new(),
        }
    }
}

/// Vault-wide editor, file, and palette settings, saved to `.notia/app.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub editor: EditorSettings,
    pub files: FileSettings,
    pub palette: PaletteSettings,
}

impl Global for AppSettings {}

struct SettingsPath(PathBuf);

impl Global for SettingsPath {}

impl AppSettings {
    pub(crate) fn load(path: PathBuf, cx: &mut App) {
        cx.set_global::<AppSettings>(crate::read_json(&path));
        cx.set_global(SettingsPath(path));
    }

    pub fn global(cx: &App) -> &AppSettings {
        cx.global::<AppSettings>()
    }

    /// Edits the settings and saves them. Observers of the `AppSettings`
    /// global are notified.
    pub fn update(cx: &mut App, edit: impl FnOnce(&mut AppSettings)) {
        let mut settings = Self::global(cx).clone();
        edit(&mut settings);
        if settings != *Self::global(cx) {
            crate::write_json(&cx.global::<SettingsPath>().0, &settings);
            cx.set_global(settings);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_palette_settings_stay_enabled() {
        let settings: AppSettings = serde_json::from_str("{}").unwrap();
        assert!(settings.palette.enabled);
        assert!(settings.palette.hidden.is_empty());
        assert!(settings.palette.hotkeys.is_empty());
    }

    #[test]
    fn split_stays_out_of_the_cycle_until_it_is_turned_on() {
        let settings: EditorSettings = serde_json::from_str("{}").unwrap();
        assert!(!settings.split_view);
        assert_eq!(settings.opening_view(), ViewMode::Editing);
        assert_eq!(ViewMode::Editing.next(false), ViewMode::Reading);
        assert_eq!(ViewMode::Reading.next(false), ViewMode::Editing);
        assert_eq!(ViewMode::Editing.next(true), ViewMode::Half);
        assert_eq!(ViewMode::Half.next(true), ViewMode::Reading);
    }
}
