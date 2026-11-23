//! Open note tabs, the editor split, and dragging a tab to a new slot.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, WindowExt as _,
    menu::{PopupMenu, PopupMenuItem},
};
use gpui_kit::*;
use notia_plugin::{
    AppSettings, EditorCommand, EditorId, Notia, OpenOptions, PluginHost, ViewMode, core::VaultPath,
};

use super::Workspace;
use super::layout::{SplitAxis, SplitGroup};
use super::phone::PhoneScreen;
use crate::file_tab::{PluginFileTab, TabDocument};
use crate::note_editor::{NoteEditor, NoteEditorEvent};

/// Which group of tabs a note lives in. `Main` is the original pane. A
/// horizontal split keeps both rows in the title bar; a vertical split keeps
/// the second row on the lower pane.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum EditorPane {
    Main,
    Split,
}

pub(super) struct OpenTab {
    pub(super) document: TabDocument,
    _subscription: Subscription,
}

pub(super) struct OpenMenu {
    pub(super) menu: Option<Entity<PopupMenu>>,
    pub(super) position: Point<Pixels>,
    pub(super) _dismiss: Subscription,
}

/// The drag payload of the divider between two note panes.
#[derive(Clone)]
pub(super) struct EditorSplitDrag;

/// A note tab being dragged to a new place in its row, or into the other pane.
#[derive(Clone)]
pub(super) struct TabDrag {
    pub(super) pane: EditorPane,
    pub(super) index: usize,
    pub(super) title: SharedString,
}

/// Where a dragged tab would land. `index` is a position in the pane's
/// current list: before that tab, or just past the last one.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct TabSlot {
    pub(super) pane: EditorPane,
    pub(super) index: usize,
}

/// The tab drawn under the pointer while one is dragged.
pub(super) struct TabGhost {
    pub(super) title: SharedString,
}

impl Render for TabGhost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .max_w(px(220.))
            .px_2()
            .py_1()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .text_sm()
            .text_color(theme.foreground)
            .shadow_sm()
            .child(
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(self.title.clone()),
            )
    }
}

/// Insertion index after a tab is lifted out of its own pane.
/// `None` when the drop would leave the order unchanged.
pub(super) fn same_pane_landing(from: usize, to: usize) -> Option<usize> {
    if to == from || to == from + 1 {
        None
    } else if to > from {
        Some(to - 1)
    } else {
        Some(to)
    }
}

impl Render for EditorSplitDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

enum NewNoteKind {
    Markdown,
    Canvas,
    Kanban,
}

impl Workspace {
    pub(crate) fn active_editor(&self) -> Option<&Entity<NoteEditor>> {
        let pane = if gpui_kit::is_mobile() {
            EditorPane::Main
        } else {
            self.focused_pane()
        };
        self.active_of(pane)
            .and_then(|ix| self.pane_tabs(pane).get(ix))
            .and_then(|tab| tab.document.as_note())
    }

    pub(super) fn focused_pane(&self) -> EditorPane {
        if self.focus == EditorPane::Split && self.split.is_some() {
            EditorPane::Split
        } else {
            EditorPane::Main
        }
    }

    /// The pane whose editor holds focus, or the last one that did.
    pub(super) fn note_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> EditorPane {
        self.note_focus(window, cx);
        self.focused_pane()
    }

    pub(super) fn note_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if gpui_kit::is_mobile() {
            self.focus = EditorPane::Main;
            return;
        }
        let detected = if self.split.as_ref().is_some_and(|split| {
            split
                .tabs
                .iter()
                .any(|tab| tab.document.contains_focus(window, cx))
        }) {
            EditorPane::Split
        } else if self
            .tabs
            .iter()
            .any(|tab| tab.document.contains_focus(window, cx))
        {
            EditorPane::Main
        } else {
            self.focus
        };
        if detected == self.focus {
            return;
        }
        self.focus = detected;
        // Publishing from inside a render borrows the workspace. The next
        // frame installs toolbars and tells plugins which note is active.
        let workspace = cx.entity().downgrade();
        window.defer(cx, move |_, cx| {
            if let Some(workspace) = workspace.upgrade() {
                workspace.update(cx, |this, cx| {
                    this.attach_toolbars(cx);
                    this.publish_active_note(cx);
                    cx.notify();
                });
            }
        });
    }

    pub(super) fn pane_tabs(&self, pane: EditorPane) -> &[OpenTab] {
        match pane {
            EditorPane::Main => &self.tabs,
            EditorPane::Split => self
                .split
                .as_ref()
                .map(|split| split.tabs.as_slice())
                .unwrap_or(&[]),
        }
    }

    pub(super) fn active_of(&self, pane: EditorPane) -> Option<usize> {
        match pane {
            EditorPane::Main => self.active_tab,
            EditorPane::Split => self.split.as_ref().and_then(|split| split.active),
        }
    }

    pub(super) fn set_active(&mut self, pane: EditorPane, active: Option<usize>) {
        match pane {
            EditorPane::Main => self.active_tab = active,
            EditorPane::Split => {
                if let Some(split) = &mut self.split {
                    split.active = active;
                }
            }
        }
    }

    pub(super) fn all_editors(&self) -> Vec<Entity<NoteEditor>> {
        self.tabs
            .iter()
            .chain(self.split.iter().flat_map(|split| split.tabs.iter()))
            .filter_map(|tab| tab.document.as_note().cloned())
            .collect()
    }

    pub(super) fn find_tab(&self, pane: EditorPane, path: &VaultPath, cx: &App) -> Option<usize> {
        self.pane_tabs(pane)
            .iter()
            .position(|tab| &tab.document.path(cx) == path)
    }

    pub(super) fn make_tab(
        &mut self,
        editor: Entity<NoteEditor>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> OpenTab {
        let subscription = cx.subscribe_in(&editor, window, |this, editor, event, _, cx| {
            if matches!(event, NoteEditorEvent::Moved) {
                this.schedule_layout_save(cx);
                cx.notify();
            }
            if this.active_editor() == Some(editor) {
                this.publish_active_note(cx);
            }
        });
        OpenTab {
            document: TabDocument::Note(editor),
            _subscription: subscription,
        }
    }

    pub(super) fn make_file_tab(
        &mut self,
        file: Entity<PluginFileTab>,
        cx: &mut Context<Self>,
    ) -> OpenTab {
        let subscription = cx.observe(&file, |_, _, _| {});
        OpenTab {
            document: TabDocument::File(file),
            _subscription: subscription,
        }
    }

    /// Opens a note in the pane that last had focus. A note already open
    /// there is shown again. Otherwise it is opened in that pane even when
    /// the other pane already has it. Without `in_new_tab`, it replaces that
    /// pane's active tab.
    pub fn open_note(
        &mut self,
        path: VaultPath,
        options: OpenOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = self.note_pane(window, cx);
        self.open_in_pane(pane, path, options, window, cx);
    }

    pub(super) fn open_in_pane(
        &mut self,
        pane: EditorPane,
        path: VaultPath,
        options: OpenOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !path.is_note() {
            if let Some(body) = PluginHost::open_file_for_path(&path, window, cx) {
                if pane == EditorPane::Split && self.split.is_none() {
                    self.ensure_split(SplitAxis::Right);
                }
                let ix = if let Some(ix) = self.find_tab(pane, &path, cx) {
                    ix
                } else {
                    self.insert_file(pane, path.clone(), body, options.is_new_tab(), window, cx)
                };
                self.finish_open(pane, ix, &path, None, window, cx);
                return;
            }
            let abs = self.vault.read(cx).vault().abs_path(&path);
            cx.open_with_system(&abs);
            return;
        }
        if pane == EditorPane::Split && self.split.is_none() {
            self.ensure_split(SplitAxis::Right);
        }
        // The last focused pane wins. A copy in the other pane stays where
        // it is; this pane gets its own tab.
        let ix = if let Some(ix) = self.find_tab(pane, &path, cx) {
            ix
        } else {
            self.insert_note(pane, path.clone(), options.is_new_tab(), window, cx)
        };
        self.finish_open(pane, ix, &path, options.line(), window, cx);
    }

    pub(super) fn insert_note(
        &mut self,
        pane: EditorPane,
        path: VaultPath,
        new_tab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        let editor = cx.new(|cx| NoteEditor::new(path, window, cx));
        let tab = self.make_tab(editor, window, cx);
        if !new_tab
            && let Some(active) = self.active_of(pane)
            && self.pane_tabs(pane).get(active).is_some()
        {
            let mut previous = self.pane_tabs(pane)[active].document.clone();
            previous.apply_edits(window, cx);
            previous.save(cx);
            match pane {
                EditorPane::Main => self.tabs[active] = tab,
                EditorPane::Split => {
                    if let Some(split) = &mut self.split {
                        split.tabs[active] = tab;
                    }
                }
            }
            return active;
        }
        let ix = self
            .active_of(pane)
            .map_or(self.pane_tabs(pane).len(), |active| active + 1);
        match pane {
            EditorPane::Main => self.tabs.insert(ix, tab),
            EditorPane::Split => {
                if self.split.is_none() {
                    self.ensure_split(SplitAxis::Right);
                }
                if let Some(split) = &mut self.split {
                    split.tabs.insert(ix, tab);
                }
            }
        }
        ix
    }

    pub(super) fn insert_file(
        &mut self,
        pane: EditorPane,
        path: VaultPath,
        body: AnyView,
        new_tab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        let file = cx.new(|cx| PluginFileTab::new(path, body, cx));
        let tab = self.make_file_tab(file, cx);
        if !new_tab
            && let Some(active) = self.active_of(pane)
            && self.pane_tabs(pane).get(active).is_some()
        {
            let mut previous = self.pane_tabs(pane)[active].document.clone();
            previous.apply_edits(window, cx);
            previous.save(cx);
            match pane {
                EditorPane::Main => self.tabs[active] = tab,
                EditorPane::Split => {
                    if let Some(split) = &mut self.split {
                        split.tabs[active] = tab;
                    }
                }
            }
            return active;
        }
        let ix = self
            .active_of(pane)
            .map_or(self.pane_tabs(pane).len(), |active| active + 1);
        match pane {
            EditorPane::Main => self.tabs.insert(ix, tab),
            EditorPane::Split => {
                if self.split.is_none() {
                    self.ensure_split(SplitAxis::Right);
                }
                if let Some(split) = &mut self.split {
                    split.tabs.insert(ix, tab);
                }
            }
        }
        ix
    }

    /// Removes a tab and keeps the pane's selection pointing at a tab that
    /// is still there. The pane itself stays, even when this was its last tab.
    pub(super) fn take_tab(&mut self, pane: EditorPane, ix: usize) -> Option<OpenTab> {
        let tab = match pane {
            EditorPane::Main => {
                if ix >= self.tabs.len() {
                    return None;
                }
                self.tabs.remove(ix)
            }
            EditorPane::Split => {
                let split = self.split.as_mut()?;
                if ix >= split.tabs.len() {
                    return None;
                }
                split.tabs.remove(ix)
            }
        };
        let len = self.pane_tabs(pane).len();
        let next = match self.active_of(pane) {
            _ if len == 0 => None,
            Some(active) if active > ix => Some(active - 1),
            Some(active) if active == ix => Some(ix.min(len - 1)),
            other => other,
        };
        self.set_active(pane, next);
        Some(tab)
    }

    pub(super) fn insert_tab(&mut self, pane: EditorPane, ix: usize, tab: OpenTab) -> usize {
        match pane {
            EditorPane::Main => {
                let ix = ix.min(self.tabs.len());
                self.tabs.insert(ix, tab);
                if let Some(active) = self.active_tab
                    && active >= ix
                {
                    self.active_tab = Some(active + 1);
                }
                ix
            }
            EditorPane::Split => {
                if self.split.is_none() {
                    self.ensure_split(SplitAxis::Right);
                }
                let Some(split) = &mut self.split else {
                    return 0;
                };
                let ix = ix.min(split.tabs.len());
                split.tabs.insert(ix, tab);
                if let Some(active) = split.active
                    && active >= ix
                {
                    split.active = Some(active + 1);
                }
                ix
            }
        }
    }

    /// Moves a tab within its row or into the other pane. The dragged note
    /// stays the same editor, so unsaved text comes with it. A note the
    /// destination already has is revealed there and the dragged tab closes.
    pub(super) fn move_tab(
        &mut self,
        from: EditorPane,
        from_ix: usize,
        to: EditorPane,
        to_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if from_ix >= self.pane_tabs(from).len() {
            cx.notify();
            return;
        }
        if from == to {
            let to_ix = to_ix.min(self.pane_tabs(from).len());
            let Some(dest) = same_pane_landing(from_ix, to_ix) else {
                self.activate(from, from_ix, window, cx);
                return;
            };
            let Some(tab) = self.take_tab(from, from_ix) else {
                return;
            };
            let ix = self.insert_tab(from, dest, tab);
            self.activate(from, ix, window, cx);
            return;
        }
        let path = self.pane_tabs(from)[from_ix].document.path(cx).clone();
        if let Some(existing) = self.find_tab(to, &path, cx) {
            self.close_in(from, from_ix, window, cx);
            if self.pane_tabs(to).get(existing).is_some() {
                self.activate(to, existing, window, cx);
            }
            return;
        }
        if to == EditorPane::Split && self.split.is_none() {
            cx.notify();
            return;
        }
        let Some(tab) = self.take_tab(from, from_ix) else {
            return;
        };
        let ix = self.insert_tab(to, to_ix, tab);
        if from == EditorPane::Split
            && self
                .split
                .as_ref()
                .is_some_and(|split| split.tabs.is_empty())
        {
            self.split = None;
        }
        self.activate(to, ix, window, cx);
    }

    pub(super) fn show_tab_slot(
        &mut self,
        from: (EditorPane, usize),
        pane: EditorPane,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let slot = if from.0 == pane && (index == from.1 || index == from.1 + 1) {
            None
        } else {
            Some(TabSlot { pane, index })
        };
        if self.tab_drop != slot {
            self.tab_drop = slot;
            cx.notify();
        }
    }

    /// The empty part of a tab row: a drop here puts the tab at the end.
    pub(super) fn hover_tab_end(
        &mut self,
        pane: EditorPane,
        event: &DragMoveEvent<TabDrag>,
        cx: &mut Context<Self>,
    ) {
        if !event.bounds.contains(&event.event.position) {
            return;
        }
        let from = (event.drag(cx).pane, event.drag(cx).index);
        let index = self.pane_tabs(pane).len();
        self.show_tab_slot(from, pane, index, cx);
    }

    /// The left half of a tab inserts before it, the right half after it.
    pub(super) fn hover_tab_edge(
        &mut self,
        pane: EditorPane,
        index: usize,
        event: &DragMoveEvent<TabDrag>,
        cx: &mut Context<Self>,
    ) {
        if !event.bounds.contains(&event.event.position) {
            return;
        }
        let from = (event.drag(cx).pane, event.drag(cx).index);
        let mid = event.bounds.left().as_f32() + event.bounds.size.width.as_f32() * 0.5;
        let at = if event.event.position.x.as_f32() > mid {
            index + 1
        } else {
            index
        };
        self.show_tab_slot(from, pane, at, cx);
    }

    /// Dropping on another pane's note moves the tab to the end of that pane.
    /// The note under a tab's own pane is not a new position; the row is.
    pub(super) fn hover_pane_body(
        &mut self,
        pane: EditorPane,
        event: &DragMoveEvent<TabDrag>,
        cx: &mut Context<Self>,
    ) {
        if !event.bounds.contains(&event.event.position) {
            return;
        }
        let from = (event.drag(cx).pane, event.drag(cx).index);
        if from.0 == pane {
            self.show_tab_slot(from, pane, from.1, cx);
            return;
        }
        let index = self.pane_tabs(pane).len();
        self.show_tab_slot(from, pane, index, cx);
    }

    pub(super) fn drop_tab(&mut self, drag: &TabDrag, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_dragging = None;
        let Some(slot) = self.tab_drop.take() else {
            cx.notify();
            return;
        };
        self.move_tab(drag.pane, drag.index, slot.pane, slot.index, window, cx);
    }

    pub(super) fn finish_open(
        &mut self,
        pane: EditorPane,
        ix: usize,
        path: &VaultPath,
        line: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate(pane, ix, window, cx);
        if let Some(line) = line
            && let Some(tab) = self.pane_tabs(pane).get(ix)
        {
            let mut document = tab.document.clone();
            document.reveal_line(line, window, cx);
        }
        self.recent.retain(|recent| recent != path);
        self.recent.insert(0, path.clone());
        self.recent.truncate(50);
        if gpui_kit::is_mobile() {
            self.phone = PhoneScreen::Note;
        }
    }

    /// Opens what a link points at; a link to a missing note creates it.
    pub fn open_link(
        &mut self,
        target: &str,
        from: Option<VaultPath>,
        new_tab: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (note, heading) = match target.split_once('#') {
            Some((note, heading)) => (note.trim(), Some(heading.trim())),
            None => (target.trim(), None),
        };
        let resolved = self.vault.read(cx).index().resolve(note, from.as_ref());
        let path = match resolved {
            Some(path) => path,
            None if note.is_empty() => return,
            None => match self.create_named_note(note, cx) {
                Ok(path) => path,
                Err(error) => {
                    window.push_notification(format!("Could not create “{note}”: {error}"), cx);
                    return;
                }
            },
        };
        let line = heading.and_then(|heading| {
            self.vault.read(cx).index().get(&path).and_then(|note| {
                note.meta
                    .headings
                    .iter()
                    .find(|candidate| candidate.text.eq_ignore_ascii_case(heading))
                    .map(|candidate| candidate.line)
            })
        });
        let mut options = OpenOptions::new();
        if new_tab {
            options = options.in_new_tab();
        }
        if let Some(line) = line {
            options = options.at_line(line);
        }
        self.open_note(path, options, window, cx);
    }

    /// Creates a note from a link target or a typed name such as
    /// `Projects/Plan`, in the configured folder unless it names its own.
    pub(crate) fn create_named_note(
        &mut self,
        name: &str,
        cx: &mut Context<Self>,
    ) -> std::io::Result<VaultPath> {
        let target = VaultPath::new(name);
        let folder = if name.contains('/') {
            target.parent().unwrap_or_else(VaultPath::root)
        } else {
            VaultPath::new(&AppSettings::global(cx).files.new_note_folder)
        };
        let title = target.file_stem().to_string();
        self.vault
            .update(cx, |vault, cx| vault.create_note(&folder, &title, "", cx))
    }

    pub fn new_note(
        &mut self,
        folder: Option<VaultPath>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_named(folder, NewNoteKind::Markdown, None, window, cx);
    }

    pub(super) fn new_note_in(
        &mut self,
        pane: EditorPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_named(None, NewNoteKind::Markdown, Some(pane), window, cx);
    }

    /// Creates an empty Canvas note in `folder`, or the configured folder.
    pub fn new_canvas(
        &mut self,
        folder: Option<VaultPath>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_named(folder, NewNoteKind::Canvas, None, window, cx);
    }

    /// Creates a Kanban note in `folder`, or the configured folder.
    pub fn new_kanban(
        &mut self,
        folder: Option<VaultPath>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_named(folder, NewNoteKind::Kanban, None, window, cx);
    }

    fn new_named(
        &mut self,
        folder: Option<VaultPath>,
        kind: NewNoteKind,
        pane: Option<EditorPane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = pane.unwrap_or_else(|| self.note_pane(window, cx));
        let folder = folder
            .unwrap_or_else(|| VaultPath::new(&AppSettings::global(cx).files.new_note_folder));
        let created = self.vault.update(cx, |vault, cx| match kind {
            NewNoteKind::Markdown => vault.create_note(&folder, "Untitled", "", cx),
            NewNoteKind::Canvas => vault.create_canvas(&folder, "Untitled", "{}", cx),
            NewNoteKind::Kanban => {
                vault.create_kanban(&folder, "Untitled", &crate::kanban::starter_json(), cx)
            }
        });
        match created {
            Ok(path) => {
                self.open_in_pane(pane, path, OpenOptions::new().in_new_tab(), window, cx);
                if let Some(editor) = self.active_editor().cloned() {
                    editor.update(cx, |editor, cx| editor.focus_title(window, cx));
                }
            }
            Err(error) => window.push_notification(format!("Could not create a note: {error}"), cx),
        }
    }

    pub(crate) fn activate_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.activate(EditorPane::Main, ix, window, cx);
    }

    pub(super) fn activate(
        &mut self,
        pane: EditorPane,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(document) = self.pane_tabs(pane).get(ix).map(|tab| tab.document.clone()) else {
            return;
        };
        self.focus = pane;
        self.set_active(pane, Some(ix));
        document.focus(window, cx);
        self.attach_toolbars(cx);
        self.publish_active_note(cx);
        self.schedule_layout_save(cx);
        if gpui_kit::is_mobile() {
            self.phone = PhoneScreen::Note;
        }
        cx.notify();
    }

    pub(super) fn close_in(
        &mut self,
        pane: EditorPane,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = match pane {
            EditorPane::Main => {
                if ix >= self.tabs.len() {
                    return;
                }
                self.tabs.remove(ix)
            }
            EditorPane::Split => {
                let Some(split) = &mut self.split else {
                    return;
                };
                if ix >= split.tabs.len() {
                    return;
                }
                split.tabs.remove(ix)
            }
        };
        let mut document = tab.document;
        document.apply_edits(window, cx);
        document.save(cx);
        let len = self.pane_tabs(pane).len();
        let active = self.active_of(pane);
        let next = match active {
            _ if len == 0 => None,
            Some(active) if active > ix => Some(active - 1),
            Some(active) if active == ix => Some(ix.min(len - 1)),
            other => other,
        };
        self.set_active(pane, next);
        if len == 0 {
            self.focus_after_empty(pane, window, cx);
            return;
        }
        if self.focus == pane
            && let Some(active) = next
        {
            self.activate(pane, active, window, cx);
        } else {
            self.schedule_layout_save(cx);
            cx.notify();
        }
    }

    /// An emptied pane gives the keyboard to the one that still has notes.
    pub(super) fn focus_after_empty(
        &mut self,
        pane: EditorPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if pane == EditorPane::Split {
            self.split = None;
        }
        let fallback = match pane {
            EditorPane::Split => EditorPane::Main,
            EditorPane::Main if self.split.is_some() => EditorPane::Split,
            EditorPane::Main => EditorPane::Main,
        };
        self.focus = fallback;
        if let Some(active) = self.active_of(fallback) {
            self.activate(fallback, active, window, cx);
        } else {
            self.focus_handle.focus(window, cx);
            self.publish_active_note(cx);
            self.schedule_layout_save(cx);
            cx.notify();
        }
    }

    pub(crate) fn close_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.note_pane(window, cx);
        if let Some(ix) = self.active_of(pane) {
            self.close_in(pane, ix, window, cx);
        }
    }

    pub(super) fn close_others(
        &mut self,
        pane: EditorPane,
        keep: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let len = self.pane_tabs(pane).len();
        for ix in (keep + 1..len).rev() {
            self.close_in(pane, ix, window, cx);
        }
        for _ in 0..keep {
            self.close_in(pane, 0, window, cx);
        }
    }

    pub(super) fn close_to_the_right(
        &mut self,
        pane: EditorPane,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let len = self.pane_tabs(pane).len();
        for index in (ix + 1..len).rev() {
            self.close_in(pane, index, window, cx);
        }
    }

    pub(super) fn close_all_in(
        &mut self,
        pane: EditorPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        while !self.pane_tabs(pane).is_empty() {
            let last = self.pane_tabs(pane).len() - 1;
            self.close_in(pane, last, window, cx);
        }
    }

    pub(super) fn close_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_all_in(EditorPane::Split, window, cx);
    }

    pub(super) fn ensure_split(&mut self, axis: SplitAxis) {
        if let Some(split) = &mut self.split {
            split.axis = axis;
            return;
        }
        self.split = Some(SplitGroup {
            axis,
            ratio: 0.5,
            tabs: Vec::new(),
            active: None,
            scroll: ScrollHandle::new(),
        });
    }

    /// Shows `pane`'s tab in a second pane. The same note stays open where it
    /// was; the new pane reads the saved text, so unsaved edits are written
    /// first.
    pub(super) fn split_tab(
        &mut self,
        pane: EditorPane,
        ix: usize,
        axis: SplitAxis,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self
            .pane_tabs(pane)
            .get(ix)
            .map(|tab| tab.document.path(cx).clone())
        else {
            return;
        };
        if let Some(tab) = self.pane_tabs(pane).get(ix) {
            let mut document = tab.document.clone();
            document.apply_edits(window, cx);
            document.save(cx);
        }
        self.ensure_split(axis);
        if let Some(existing) = self.find_tab(EditorPane::Split, &path, cx) {
            self.activate(EditorPane::Split, existing, window, cx);
            return;
        }
        if path.is_note() {
            let index = self.insert_note(EditorPane::Split, path, true, window, cx);
            self.activate(EditorPane::Split, index, window, cx);
        } else {
            self.open_in_pane(
                EditorPane::Split,
                path,
                OpenOptions::new().in_new_tab(),
                window,
                cx,
            );
        }
    }

    pub(crate) fn cycle_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let pane = self.note_pane(window, cx);
        let count = self.pane_tabs(pane).len();
        if let Some(active) = self.active_of(pane).filter(|_| count > 1) {
            let next = if forward {
                (active + 1) % count
            } else {
                (active + count - 1) % count
            };
            self.activate(pane, next, window, cx);
        }
    }

    pub(crate) fn set_view_mode(
        &mut self,
        mode: ViewMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if mode == ViewMode::Half && !AppSettings::global(cx).editor.split_view {
            return;
        }
        self.note_focus(window, cx);
        if let Some(editor) = self.active_editor().cloned() {
            editor.update(cx, |editor, cx| editor.set_mode(mode, window, cx));
        }
    }

    pub(crate) fn toggle_reading_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.note_focus(window, cx);
        if let Some(editor) = self.active_editor().cloned() {
            editor.update(cx, |editor, cx| editor.toggle_mode(window, cx));
        }
    }

    pub(crate) fn save_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.note_focus(window, cx);
        if let Some(editor) = self.active_editor().cloned() {
            editor.update(cx, |editor, cx| editor.save(cx));
        }
    }

    pub(crate) fn reveal_line(&mut self, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.note_focus(window, cx);
        if let Some(editor) = self.active_editor().cloned() {
            editor.update(cx, |editor, cx| editor.reveal_line(line, window, cx));
        }
    }

    /// The editor with this id, or the active one for `None`.
    pub(crate) fn editor(&self, id: Option<EditorId>, cx: &App) -> Option<&Entity<NoteEditor>> {
        match id {
            Some(id) => [EditorPane::Main, EditorPane::Split]
                .into_iter()
                .find_map(|pane| {
                    self.pane_tabs(pane)
                        .iter()
                        .filter_map(|tab| tab.document.as_note())
                        .find(|editor| editor.read(cx).id() == id)
                }),
            None => self.active_editor(),
        }
    }

    /// Applies the edits plugins queued on any open note.
    pub(crate) fn apply_edits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for editor in self.all_editors() {
            editor.update(cx, |editor, cx| editor.apply_edits(window, cx));
        }
    }

    pub(crate) fn run_editor_command(
        &mut self,
        id: EditorId,
        command: EditorCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.editor(Some(id), cx).cloned() {
            editor.update(cx, |editor, cx| editor.run_command(command, window, cx));
        }
    }
}

pub(super) fn tab_context_menu(
    menu: PopupMenu,
    workspace: Entity<Workspace>,
    pane: EditorPane,
    ix: usize,
    count: usize,
    split_open: bool,
) -> PopupMenu {
    let close = workspace.clone();
    let others = workspace.clone();
    let right = workspace.clone();
    let all = workspace.clone();
    let split_right = workspace.clone();
    let split_down = workspace.clone();
    let close_split = workspace.clone();
    let copy = workspace.clone();
    let reveal = workspace.clone();
    let reveal_label = if cfg!(target_os = "macos") {
        "Reveal in Finder"
    } else {
        "Show in system explorer"
    };
    let menu =
        menu.item(PopupMenuItem::new("Close").icon(IconName::Close).on_click(
            move |_, window, cx| {
                close.update(cx, |this, cx| this.close_in(pane, ix, window, cx));
            },
        ))
        .item(
            PopupMenuItem::new("Close others")
                .disabled(count < 2)
                .on_click(move |_, window, cx| {
                    others.update(cx, |this, cx| this.close_others(pane, ix, window, cx));
                }),
        )
        .item(
            PopupMenuItem::new("Close to the right")
                .disabled(ix + 1 >= count)
                .on_click(move |_, window, cx| {
                    right.update(cx, |this, cx| this.close_to_the_right(pane, ix, window, cx));
                }),
        )
        .item(
            PopupMenuItem::new("Close all").on_click(move |_, window, cx| {
                all.update(cx, |this, cx| this.close_all_in(pane, window, cx));
            }),
        )
        .separator()
        .item(
            PopupMenuItem::new("Split right")
                .icon(Icon::new(Lucide::Columns2))
                .on_click(move |_, window, cx| {
                    split_right.update(cx, |this, cx| {
                        this.split_tab(pane, ix, SplitAxis::Right, window, cx);
                    });
                }),
        )
        .item(
            PopupMenuItem::new("Split down")
                .icon(Icon::new(Lucide::Rows2))
                .on_click(move |_, window, cx| {
                    split_down.update(cx, |this, cx| {
                        this.split_tab(pane, ix, SplitAxis::Down, window, cx);
                    });
                }),
        );
    let menu = if split_open {
        menu.item(
            PopupMenuItem::new("Close split").on_click(move |_, window, cx| {
                close_split.update(cx, |this, cx| this.close_split(window, cx));
            }),
        )
    } else {
        menu
    };
    menu.separator()
        .item(
            PopupMenuItem::new("Copy path")
                .icon(IconName::Copy)
                .on_click(move |_, _, cx| {
                    let Some(path) = copy.update(cx, |this, cx| {
                        this.pane_tabs(pane)
                            .get(ix)
                            .map(|tab| tab.document.path(cx).to_string())
                    }) else {
                        return;
                    };
                    cx.write_to_clipboard(ClipboardItem::new_string(path));
                }),
        )
        .item(PopupMenuItem::new(reveal_label).on_click(move |_, _, cx| {
            let Some(path) = reveal.update(cx, |this, cx| {
                this.pane_tabs(pane)
                    .get(ix)
                    .map(|tab| tab.document.path(cx).clone())
            }) else {
                return;
            };
            let abs = Notia::vault(cx).read(cx).vault().abs_path(&path);
            cx.reveal_path(&abs);
        }))
}
