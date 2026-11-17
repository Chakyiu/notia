//! Workspace commands that the shell binds to keys and handles.

use gpui_kit::{Action, SharedString, actions};

/// The key context of the workspace. Plugin keybindings apply inside it.
pub const WORKSPACE_CONTEXT: &str = "Workspace";

actions!(
    notia,
    [
        /// Lists every command.
        OpenCommandPalette,
        /// Jumps to a note by name.
        OpenQuickSwitcher,
        OpenSettings,
        NewNote,
        NewCanvas,
        CloseTab,
        NextTab,
        PreviousTab,
        ToggleLeftSidebar,
        ToggleRightSidebar,
        /// Shows the active note as Markdown source.
        EditMode,
        /// Shows the Markdown and the rendered note side by side.
        HalfEditMode,
        /// Shows the active note rendered.
        PreviewMode,
        /// Cycles the active note through edit, half edit, and preview.
        ToggleReadingView,
        SaveNote,
    ]
);

/// Runs the plugin command with this qualified id.
#[derive(Clone, Debug, PartialEq, Action)]
#[action(namespace = notia, no_json)]
pub struct RunCommand {
    pub id: SharedString,
}
