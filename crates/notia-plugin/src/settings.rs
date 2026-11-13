use std::path::PathBuf;

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
    /// The next mode in the cycle: edit, half edit, preview.
    pub fn next(self) -> Self {
        match self {
            Self::Editing => Self::Half,
            Self::Half => Self::Reading,
            Self::Reading => Self::Editing,
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
    pub line_numbers: bool,
    /// How long typing must pause before a note is saved.
    pub autosave_delay_ms: u64,
    pub tab_size: usize,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            default_view: ViewMode::Editing,
            line_numbers: false,
            autosave_delay_ms: 800,
            tab_size: 4,
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

/// Vault-wide editor and file settings, saved to `.notia/app.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub editor: EditorSettings,
    pub files: FileSettings,
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
