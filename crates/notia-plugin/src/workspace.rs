//! What the workspace shows, and how plugins ask it to do things.
//!
//! The shell implements [`WorkspaceHandle`] and installs it with
//! [`set_handle`]; plugins call the free functions here, so they never
//! depend on the shell itself.

use std::rc::Rc;

use gpui_kit::{App, Context, EventEmitter, Global, SharedString, Window};
use notia_core::VaultPath;

use crate::{
    ViewMode,
    editor::{EditorCommand, EditorId, EditorSnapshot, EditorTransaction},
};

/// The note in the active tab, as plugins see it.
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveNote {
    path: VaultPath,
    text: SharedString,
    cursor_line: usize,
    mode: ViewMode,
}

impl ActiveNote {
    pub fn new(path: VaultPath, text: SharedString, mode: ViewMode) -> Self {
        Self {
            path,
            text,
            cursor_line: 0,
            mode,
        }
    }

    pub fn with_cursor_line(mut self, line: usize) -> Self {
        self.cursor_line = line;
        self
    }

    pub fn path(&self) -> &VaultPath {
        &self.path
    }

    /// The text in the editor, including unsaved edits.
    pub fn text(&self) -> &SharedString {
        &self.text
    }

    /// The 0-based line of the cursor.
    pub fn cursor_line(&self) -> usize {
        self.cursor_line
    }

    pub fn mode(&self) -> ViewMode {
        self.mode
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceEvent {
    /// Another note, or no note, became active.
    ActiveNoteChanged,
    /// The active note's text changed.
    ActiveNoteEdited,
    /// The cursor moved or the view mode changed.
    ActiveNoteMoved,
    /// Something asked to search the vault, for example a click on a tag.
    SearchRequested(SharedString),
}

/// Published by the shell; observe it to follow the active note.
#[derive(Default)]
pub struct WorkspaceModel {
    active: Option<ActiveNote>,
}

impl EventEmitter<WorkspaceEvent> for WorkspaceModel {}

impl WorkspaceModel {
    pub fn active_note(&self) -> Option<&ActiveNote> {
        self.active.as_ref()
    }

    pub fn active_path(&self) -> Option<&VaultPath> {
        self.active.as_ref().map(ActiveNote::path)
    }

    /// Called by the shell whenever the active note, its text, its cursor
    /// line or its mode changes.
    pub fn set_active_note(&mut self, note: Option<ActiveNote>, cx: &mut Context<Self>) {
        if note == self.active {
            return;
        }
        let event = match (&self.active, &note) {
            (Some(before), Some(after)) if before.path == after.path => {
                if before.text != after.text {
                    WorkspaceEvent::ActiveNoteEdited
                } else {
                    WorkspaceEvent::ActiveNoteMoved
                }
            }
            _ => WorkspaceEvent::ActiveNoteChanged,
        };
        self.active = note;
        cx.emit(event);
        cx.notify();
    }

    /// Asks the search plugin, if one is enabled, to run `query`.
    pub fn request_search(&mut self, query: SharedString, cx: &mut Context<Self>) {
        cx.emit(WorkspaceEvent::SearchRequested(query));
    }
}

/// How to open a note.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OpenOptions {
    line: Option<usize>,
    new_tab: bool,
}

impl OpenOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Places the cursor on this 0-based line and scrolls to it.
    pub fn at_line(mut self, line: usize) -> Self {
        self.line = Some(line);
        self
    }

    /// Opens the note in a new tab instead of the active one.
    pub fn in_new_tab(mut self) -> Self {
        self.new_tab = true;
        self
    }

    pub fn line(&self) -> Option<usize> {
        self.line
    }

    pub fn is_new_tab(&self) -> bool {
        self.new_tab
    }
}

/// The operations the shell performs on behalf of plugins.
pub trait WorkspaceHandle {
    fn open_note(&self, path: VaultPath, options: OpenOptions, window: &mut Window, cx: &mut App);
    /// Opens the note a link points at, creating it when it does not exist.
    fn open_link(
        &self,
        target: &str,
        from: Option<VaultPath>,
        new_tab: bool,
        window: &mut Window,
        cx: &mut App,
    );
    /// Moves the cursor of the active note to a 0-based line.
    fn reveal_line(&self, line: usize, window: &mut Window, cx: &mut App);
    /// The editor of an open note, or of the active note when `id` is
    /// `None`, with the edits it has yet to apply.
    fn editor(&self, id: Option<EditorId>, cx: &App) -> Option<EditorSnapshot>;
    /// Queues an edit of an open note. [`Self::editor`] includes it at once;
    /// the editor applies it, in order, when the current update is over.
    fn edit(
        &self,
        id: EditorId,
        transaction: EditorTransaction,
        cx: &mut App,
    ) -> anyhow::Result<()>;
    /// Runs `command` in an open note's editor, after its queued edits.
    fn run_editor_command(&self, id: EditorId, command: EditorCommand, cx: &mut App);
    /// Creates a note in `folder`, or the configured folder, and opens it.
    fn new_note(&self, folder: Option<VaultPath>, window: &mut Window, cx: &mut App);
    /// Creates a Canvas note in `folder`, or the configured folder, and opens it.
    fn new_canvas(&self, folder: Option<VaultPath>, window: &mut Window, cx: &mut App);
    /// Creates a Kanban note in `folder`, or the configured folder, and opens it.
    fn new_kanban(&self, folder: Option<VaultPath>, window: &mut Window, cx: &mut App);
    /// Opens the sidebar holding a panel and selects the panel.
    fn show_panel(&self, panel_id: &str, window: &mut Window, cx: &mut App);
    fn run_command(&self, command_id: &str, window: &mut Window, cx: &mut App);
}

struct HandleSlot(Rc<dyn WorkspaceHandle>);

impl Global for HandleSlot {}

/// Installs the shell's implementation of the workspace operations.
pub fn set_handle(handle: Rc<dyn WorkspaceHandle>, cx: &mut App) {
    cx.set_global(HandleSlot(handle));
}

fn handle(cx: &App) -> Option<Rc<dyn WorkspaceHandle>> {
    let handle = cx.try_global::<HandleSlot>().map(|slot| slot.0.clone());
    if handle.is_none() {
        log::warn!("the workspace is not ready");
    }
    handle
}

pub fn open_note(path: VaultPath, options: OpenOptions, window: &mut Window, cx: &mut App) {
    if let Some(handle) = handle(cx) {
        handle.open_note(path, options, window, cx);
    }
}

pub fn open_link(
    target: &str,
    from: Option<VaultPath>,
    new_tab: bool,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(handle) = handle(cx) {
        handle.open_link(target, from, new_tab, window, cx);
    }
}

pub fn reveal_line(line: usize, window: &mut Window, cx: &mut App) {
    if let Some(handle) = handle(cx) {
        handle.reveal_line(line, window, cx);
    }
}

/// Inserts text at the cursor of the active note and focuses it.
pub fn insert_text(text: &str, _: &mut Window, cx: &mut App) {
    let Some(editor) = active_editor(cx) else {
        return;
    };
    let insert = EditorTransaction::insert(editor.selection.head, text);
    match edit(editor.id, insert, cx) {
        Ok(()) => run_editor_command(editor.id, EditorCommand::Focus, cx),
        Err(error) => log::warn!("could not insert text: {error:#}"),
    }
}

/// The text and selection of the active note, if one is open.
pub fn active_editor(cx: &App) -> Option<EditorSnapshot> {
    handle(cx)?.editor(None, cx)
}

/// The text and selection of an open note.
pub fn editor(id: EditorId, cx: &App) -> Option<EditorSnapshot> {
    handle(cx)?.editor(Some(id), cx)
}

/// Changes the text or selection of an open note; see [`EditorTransaction`].
/// Fails when the note is closed or the edit does not fit its text.
pub fn edit(id: EditorId, transaction: EditorTransaction, cx: &mut App) -> anyhow::Result<()> {
    match handle(cx) {
        Some(handle) => handle.edit(id, transaction, cx),
        None => Err(anyhow::anyhow!("the workspace is not ready")),
    }
}

pub fn run_editor_command(id: EditorId, command: EditorCommand, cx: &mut App) {
    if let Some(handle) = handle(cx) {
        handle.run_editor_command(id, command, cx);
    }
}

pub fn new_note(folder: Option<VaultPath>, window: &mut Window, cx: &mut App) {
    if let Some(handle) = handle(cx) {
        handle.new_note(folder, window, cx);
    }
}

pub fn new_canvas(folder: Option<VaultPath>, window: &mut Window, cx: &mut App) {
    if let Some(handle) = handle(cx) {
        handle.new_canvas(folder, window, cx);
    }
}

pub fn new_kanban(folder: Option<VaultPath>, window: &mut Window, cx: &mut App) {
    if let Some(handle) = handle(cx) {
        handle.new_kanban(folder, window, cx);
    }
}

pub fn show_panel(panel_id: &str, window: &mut Window, cx: &mut App) {
    if let Some(handle) = handle(cx) {
        handle.show_panel(panel_id, window, cx);
    }
}

pub fn run_command(command_id: &str, window: &mut Window, cx: &mut App) {
    if let Some(handle) = handle(cx) {
        handle.run_command(command_id, window, cx);
    }
}
