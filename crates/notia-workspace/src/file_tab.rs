//! Tabs that show a plugin-provided view for a vault file (such as a PDF).

use gpui_kit::*;
use notia_plugin::{ViewMode, core::VaultPath};

use crate::note_editor::NoteEditor;

/// A non-note file open in the workspace.
pub struct PluginFileTab {
    focus_handle: FocusHandle,
    path: VaultPath,
    body: AnyView,
}

impl PluginFileTab {
    pub fn new(path: VaultPath, body: AnyView, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            path,
            body,
        }
    }

    pub fn path(&self) -> &VaultPath {
        &self.path
    }

    pub fn title(&self) -> SharedString {
        self.path.file_stem().into()
    }

    pub fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }

    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.contains_focused(window, cx)
    }

    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
    }
}

impl Render for PluginFileTab {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.body.clone()
    }
}

/// What one open tab shows: a note editor or a plugin file view.
#[derive(Clone)]
pub(super) enum TabDocument {
    Note(Entity<NoteEditor>),
    File(Entity<PluginFileTab>),
}

impl TabDocument {
    pub fn path(&self, cx: &App) -> VaultPath {
        match self {
            Self::Note(editor) => editor.read(cx).path().clone(),
            Self::File(file) => file.read(cx).path().clone(),
        }
    }

    pub fn title(&self, cx: &App) -> SharedString {
        match self {
            Self::Note(editor) => editor.read(cx).title(),
            Self::File(file) => file.read(cx).title(),
        }
    }

    pub fn as_note(&self) -> Option<&Entity<NoteEditor>> {
        match self {
            Self::Note(editor) => Some(editor),
            Self::File(_) => None,
        }
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self {
            Self::Note(editor) => editor.read(cx).focus_handle(cx),
            Self::File(file) => file.read(cx).focus_handle(cx),
        }
    }

    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        match self {
            Self::Note(editor) => editor.read(cx).contains_focus(window, cx),
            Self::File(file) => file.read(cx).contains_focus(window, cx),
        }
    }

    pub fn into_any_element(self) -> AnyElement {
        match self {
            Self::Note(editor) => editor.into_any_element(),
            Self::File(file) => file.into_any_element(),
        }
    }

    pub fn save(&mut self, cx: &mut App) {
        if let Self::Note(editor) = self {
            editor.update(cx, |editor, cx| editor.save(cx));
        }
    }

    pub fn apply_edits(&mut self, window: &mut Window, cx: &mut App) {
        if let Self::Note(editor) = self {
            editor.update(cx, |editor, cx| editor.apply_edits(window, cx));
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        match self {
            Self::Note(editor) => {
                editor.update(cx, |editor, cx| editor.focus(window, cx));
            }
            Self::File(file) => file.update(cx, |file, cx| file.focus(window, cx)),
        }
    }

    pub fn text(&self, cx: &App) -> SharedString {
        match self {
            Self::Note(editor) => editor.read(cx).text(cx),
            Self::File(_) => SharedString::default(),
        }
    }

    pub fn mode(&self, cx: &App) -> ViewMode {
        match self {
            Self::Note(editor) => editor.read(cx).mode(),
            Self::File(_) => ViewMode::Reading,
        }
    }

    pub fn cursor_line(&self, cx: &App) -> usize {
        match self {
            Self::Note(editor) => editor.read(cx).cursor_line(cx),
            Self::File(_) => 0,
        }
    }

    pub fn reveal_line(&mut self, line: usize, window: &mut Window, cx: &mut App) {
        if let Self::Note(editor) = self {
            editor.update(cx, |editor, cx| editor.reveal_line(line, window, cx));
        }
    }

    pub fn starts_with(&self, prefix: &VaultPath, cx: &App) -> bool {
        self.path(cx).starts_with(prefix)
    }
}
