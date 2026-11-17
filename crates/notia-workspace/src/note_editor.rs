use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{
        Editor, EditorState, Input, InputEvent, InputState, Paste, Position, Redo, TabSize, Undo,
    },
    menu::{DropdownMenu as _, PopupMenuItem},
    notification::Notification,
    text::{MarkdownExtensions, TextView, TextViewState},
    v_flex,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use notia_plugin::{
    AppSettings, EditorCommand, EditorId, EditorSelection, EditorSnapshot, EditorTransaction,
    Notia, OpenOptions, VaultEvent, ViewMode,
    actions::{ToggleReadingView, WORKSPACE_CONTEXT},
    core::VaultPath,
    theme::EditorAppearance,
    workspace,
};

use crate::attachments::{self, image_in_vault};
use crate::canvas::{CanvasEvent, CanvasView};
use crate::kanban::{KanbanEvent, KanbanView};
use crate::reading::{CALLOUTS, NoteUrl, ReadingMarkdown, parse_note_url, to_reading_markdown};

/// Small, so that plugins written in JavaScript hold editor ids exactly.
static NEXT_EDITOR_ID: AtomicU64 = AtomicU64::new(1);

pub enum NoteEditorEvent {
    /// The text changed.
    Edited,
    /// The view mode changed.
    ModeChanged,
    /// The note was renamed or moved.
    Moved,
}

/// One open note: its title, its Markdown source, and its rendered reading
/// view. Edits are saved shortly after typing pauses.
pub struct NoteEditor {
    focus_handle: FocusHandle,
    id: EditorId,
    path: VaultPath,
    title: Entity<InputState>,
    editor: Entity<EditorState>,
    /// The board of a Canvas note. Such a note has no Markdown text, so this
    /// view draws the body and does the saving.
    canvas: Option<Entity<CanvasView>>,
    /// The columns of a Kanban note. It saves itself, like a canvas.
    kanban: Option<Entity<KanbanView>>,
    /// The Kanban board as JSON. Kept on the note so a publish can use the
    /// text the board just emitted, without reading that view again.
    kanban_text: Option<SharedString>,
    reading: Entity<TextViewState>,
    reading_markdown: ReadingMarkdown,
    extensions: MarkdownExtensions,
    mode: ViewMode,
    dirty: bool,
    reading_stale: bool,
    save_task: Option<Task<()>>,
    error: Option<SharedString>,
    /// Edits plugins made, which need a window to apply. Reads include them.
    pending_edits: Vec<EditorTransaction>,
    /// Format bars contributed by plugins, drawn above the title.
    toolbars: Vec<AnyView>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<NoteEditorEvent> for NoteEditor {}

impl NoteEditor {
    pub fn new(path: VaultPath, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let vault = Notia::vault(cx);
        let (text, error) = match vault.read(cx).read_note(&path) {
            Ok(text) => (text, None),
            Err(error) => (
                String::new(),
                Some(format!("Could not open this note: {error}").into()),
            ),
        };
        let settings = AppSettings::global(cx).editor.clone();
        let canvas = path
            .is_canvas()
            .then(|| cx.new(|cx| CanvasView::new(path.clone(), &text, cx)));
        let kanban = path
            .is_kanban()
            .then(|| cx.new(|cx| KanbanView::new(path.clone(), &text, cx)));
        let kanban_text = kanban.as_ref().map(|board| board.read(cx).document());
        let board = canvas.is_some() || kanban.is_some();
        let title =
            cx.new(|cx| InputState::new(window, cx).default_value(path.file_stem().to_string()));
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("markdown")
                .line_number(settings.line_numbers)
                .tab_size(TabSize {
                    tab_size: settings.tab_size,
                    hard_tabs: false,
                })
                .placeholder("Start writing…")
                .default_value(if board { "" } else { &text })
        });
        let reading_markdown = to_reading_markdown(if board { "" } else { &text });
        let reading = cx.new(|cx| TextViewState::markdown(&reading_markdown.text, cx));

        let mut subscriptions = vec![
            cx.subscribe_in(&editor, window, Self::on_editor_event),
            cx.subscribe_in(&title, window, Self::on_title_event),
            cx.subscribe_in(&vault, window, Self::on_vault_event),
            cx.observe_global_in::<AppSettings>(window, Self::apply_settings),
        ];
        if let Some(board) = &canvas {
            subscriptions.push(cx.subscribe(board, |_, _, _: &CanvasEvent, cx| {
                cx.emit(NoteEditorEvent::Edited);
            }));
        }
        if let Some(board) = &kanban {
            subscriptions.push(cx.subscribe(board, |this, _, event: &KanbanEvent, cx| {
                this.kanban_text = Some(event.document.clone());
                cx.emit(NoteEditorEvent::Edited);
            }));
        }

        Self {
            focus_handle: cx.focus_handle(),
            id: EditorId::new(NEXT_EDITOR_ID.fetch_add(1, Ordering::Relaxed)),
            path,
            title,
            editor,
            canvas,
            kanban,
            kanban_text,
            reading,
            reading_markdown,
            extensions: callout_extensions(),
            mode: settings.default_view,
            dirty: false,
            reading_stale: false,
            save_task: None,
            error,
            pending_edits: Vec::new(),
            toolbars: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    pub fn id(&self) -> EditorId {
        self.id
    }

    /// Replaces the format bars shown above the title. Only the visible note
    /// should hold them, because a view can have one parent.
    pub fn set_toolbars(&mut self, toolbars: Vec<AnyView>, cx: &mut Context<Self>) {
        self.toolbars = toolbars;
        cx.notify();
    }

    pub fn path(&self) -> &VaultPath {
        &self.path
    }

    pub fn title(&self) -> SharedString {
        self.path.file_stem().to_string().into()
    }

    pub fn mode(&self) -> ViewMode {
        self.mode
    }

    /// The text in the editor, including unsaved edits. A kanban note reports
    /// its board, which is what gets written to the file.
    pub fn text(&self, cx: &App) -> SharedString {
        if let Some(canvas) = &self.canvas {
            return canvas.read(cx).document();
        }
        if let Some(text) = &self.kanban_text {
            return text.clone();
        }
        self.editor.read(cx).value()
    }

    fn is_board(&self) -> bool {
        self.canvas.is_some() || self.kanban.is_some()
    }

    pub fn cursor_line(&self, cx: &App) -> usize {
        self.editor.read(cx).cursor_position().line as usize
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        if let Some(canvas) = &self.canvas {
            let handle = canvas.read(cx).focus_handle(cx);
            handle.focus(window, cx);
            return;
        }
        if let Some(kanban) = &self.kanban {
            kanban.read(cx).focus_handle(cx).focus(window, cx);
            return;
        }
        match self.mode {
            ViewMode::Editing | ViewMode::Half => self
                .editor
                .update(cx, |editor, cx| editor.focus(window, cx)),
            ViewMode::Reading => self.focus_handle.focus(window, cx),
        }
    }

    /// Focuses the title with its text selected, for naming a new note.
    pub fn focus_title(&self, window: &mut Window, cx: &mut App) {
        self.title.update(cx, |title, cx| {
            title.focus(window, cx);
            title.select_all(window, cx);
        });
    }

    pub fn set_mode(&mut self, mode: ViewMode, window: &mut Window, cx: &mut Context<Self>) {
        if mode == self.mode {
            return;
        }
        if mode.shows_preview() && self.reading_stale {
            self.refresh_reading(cx);
        }
        self.mode = mode;
        self.focus(window, cx);
        cx.emit(NoteEditorEvent::ModeChanged);
        cx.notify();
    }

    pub fn toggle_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_mode(self.mode.next(), window, cx);
    }

    /// Moves to a 0-based source line, in either mode.
    pub fn reveal_line(&mut self, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_edits(window, cx);
        match self.mode {
            ViewMode::Editing | ViewMode::Half => self.editor.update(cx, |editor, cx| {
                editor.set_cursor_position(Position::new(line as u32, 0), window, cx);
            }),
            ViewMode::Reading => {
                let Some(source) = self.reading_markdown.line_range(line) else {
                    return;
                };
                self.reading.update(cx, |reading, cx| {
                    if let Some(range) = reading.rendered_text().range_for_source(source) {
                        _ = reading.reveal_range(range, cx);
                    }
                });
            }
        }
    }

    /// The text and selection as plugins see them: with their queued edits.
    pub fn snapshot(&self, cx: &App) -> EditorSnapshot {
        if let Some(canvas) = &self.canvas {
            return EditorSnapshot {
                id: self.id,
                path: self.path.clone(),
                mode: self.mode,
                text: canvas.read(cx).document(),
                selection: EditorSelection::cursor(0),
            };
        }
        if let Some(text) = &self.kanban_text {
            return EditorSnapshot {
                id: self.id,
                path: self.path.clone(),
                mode: self.mode,
                text: text.clone(),
                selection: EditorSelection::cursor(0),
            };
        }
        let state = self.editor.read(cx);
        let mut text = state.value();
        let mut selection = selection_of(state);
        if !self.pending_edits.is_empty() {
            let mut edited = text.to_string();
            for transaction in &self.pending_edits {
                if let Ok((after, moved)) = transaction.apply(&edited, selection) {
                    (edited, selection) = (after, moved);
                }
            }
            text = edited.into();
        }
        EditorSnapshot {
            id: self.id,
            path: self.path.clone(),
            mode: self.mode,
            text,
            selection,
        }
    }

    /// Queues a plugin's edit, which needs a window to apply. It must fit
    /// the text as it will be after the edits queued before it.
    pub fn queue_edit(&mut self, transaction: EditorTransaction, cx: &App) -> anyhow::Result<()> {
        anyhow::ensure!(!self.is_board(), "this note is a board, not text");
        let snapshot = self.snapshot(cx);
        transaction.apply(&snapshot.text, snapshot.selection)?;
        self.pending_edits.push(transaction);
        Ok(())
    }

    /// Applies the queued edits in order, each as one undo step.
    pub fn apply_edits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_edits.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.pending_edits);
        let mut edited = false;
        self.editor.update(cx, |editor, cx| {
            for transaction in pending {
                let text = editor.value();
                let applied = transaction
                    .apply(&text, selection_of(editor))
                    .and_then(|(_, selection)| Ok((transaction.merged(&text)?, selection)));
                let (change, selection) = match applied {
                    Ok(applied) => applied,
                    // The note changed under the edit, as when it is reloaded from disk.
                    Err(error) => {
                        log::warn!("dropped a plugin's edit of {}: {error:#}", self.path);
                        continue;
                    }
                };
                let change = change.filter(|change| text[change.range.clone()] != change.text);
                if let Some(change) = change {
                    editor.set_selected_range(change.range, cx);
                    editor.replace(change.text, window, cx);
                    edited = true;
                }
                editor.set_selected_range_with_direction(
                    selection.range(),
                    selection.is_reversed(),
                    cx,
                );
            }
        });
        if edited {
            self.mark_edited(cx);
        }
    }

    /// Runs a plugin's editor command once the queued edits are applied.
    pub fn run_command(
        &mut self,
        command: EditorCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_board() {
            // A board has no text to format or undo through the editor.
            self.focus(window, cx);
            return;
        }
        self.apply_edits(window, cx);
        let from_preview = self.mode == ViewMode::Reading;
        if from_preview {
            self.set_mode(ViewMode::Editing, window, cx);
        } else {
            self.focus(window, cx);
        }
        let action: Box<dyn Action> = match command {
            EditorCommand::Focus => return,
            EditorCommand::Undo => Box::new(Undo),
            EditorCommand::Redo => Box::new(Redo),
        };
        let target = self.editor.read(cx).focus_handle(cx);
        if from_preview {
            // The editor takes actions once it has been drawn. Next-frame
            // callbacks run before that frame is drawn, so wait for two.
            window.on_next_frame(move |window, _| {
                window.on_next_frame(move |window, cx| {
                    target.dispatch_action(action.as_ref(), window, cx)
                })
            });
        } else {
            target.dispatch_action(action.as_ref(), window, cx);
        }
    }

    /// Writes unsaved edits to disk now.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        if let Some(canvas) = &self.canvas {
            canvas.update(cx, |canvas, cx| canvas.save(cx));
            return;
        }
        if let Some(kanban) = &self.kanban {
            kanban.update(cx, |kanban, cx| kanban.save(cx));
            return;
        }
        self.save_task = None;
        if !self.dirty {
            return;
        }
        let text = self.text(cx);
        let path = self.path.clone();
        match Notia::vault(cx).update(cx, |vault, cx| vault.save_note(&path, &text, cx)) {
            Ok(()) => {
                self.dirty = false;
                self.error = None;
            }
            Err(error) => self.error = Some(format!("Could not save this note: {error}").into()),
        }
        cx.notify();
    }

    fn mark_edited(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        if self.mode.shows_preview() {
            self.refresh_reading(cx);
        } else {
            self.reading_stale = true;
        }
        let delay = Duration::from_millis(AppSettings::global(cx).editor.autosave_delay_ms);
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            _ = this.update(cx, |this, cx| this.save(cx));
        }));
        cx.emit(NoteEditorEvent::Edited);
    }

    fn refresh_reading(&mut self, cx: &mut Context<Self>) {
        self.reading_markdown = to_reading_markdown(&self.text(cx));
        let text = self.reading_markdown.text.clone();
        self.reading
            .update(cx, |reading, cx| reading.set_text(&text, cx));
        self.reading_stale = false;
    }

    fn on_editor_event(
        &mut self,
        _: &Entity<EditorState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, InputEvent::Change) {
            self.mark_edited(cx);
        }
    }

    fn on_title_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::PressEnter { .. } => {
                self.commit_title(window, cx);
                self.focus(window, cx);
            }
            InputEvent::Blur => self.commit_title(window, cx),
            _ => {}
        }
    }

    /// Renames the note after its title, or restores the title when the
    /// rename is not possible.
    fn commit_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.title.read(cx).value().trim().to_string();
        if name.is_empty() || name == self.path.file_stem() {
            self.reset_title(window, cx);
            return;
        }
        self.save(cx);
        let path = self.path.clone();
        if let Err(error) = Notia::vault(cx).update(cx, |vault, cx| vault.rename(&path, &name, cx))
        {
            window.push_notification(format!("Could not rename the note: {error}"), cx);
            self.reset_title(window, cx);
        }
    }

    fn reset_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let stem = self.path.file_stem().to_string();
        self.title
            .update(cx, |title, cx| title.set_value(stem, window, cx));
    }

    fn on_vault_event(
        &mut self,
        vault: &Entity<notia_plugin::VaultModel>,
        event: &VaultEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            VaultEvent::Renamed { from, to } => {
                if let Some(path) = self.path.rebase(from, to) {
                    self.path = path.clone();
                    if let Some(canvas) = &self.canvas {
                        canvas.update(cx, |canvas, cx| canvas.set_path(path.clone(), cx));
                    }
                    if let Some(kanban) = &self.kanban {
                        kanban.update(cx, |kanban, cx| kanban.set_path(path, cx));
                    }
                    self.reset_title(window, cx);
                    cx.emit(NoteEditorEvent::Moved);
                    cx.notify();
                }
            }
            VaultEvent::NotesChanged(paths)
                if !self.dirty && !self.is_board() && paths.contains(&self.path) =>
            {
                // Changed outside Notia: show the new text unless the user
                // has edits of their own.
                let Some(note) = vault.read(cx).index().get(&self.path) else {
                    return;
                };
                let text = note.text.clone();
                if *text != *self.text(cx) {
                    // They were made to the text being replaced.
                    self.pending_edits.clear();
                    self.editor.update(cx, |editor, cx| {
                        editor.set_value(text.to_string(), window, cx)
                    });
                    self.reading_stale = true;
                    if self.mode.shows_preview() {
                        self.refresh_reading(cx);
                    }
                    cx.emit(NoteEditorEvent::Edited);
                }
            }
            _ => {}
        }
    }

    fn apply_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = AppSettings::global(cx).editor.clone();
        self.editor.update(cx, |editor, cx| {
            editor.set_line_number(settings.line_numbers, window, cx);
        });
    }

    /// The mode button and the note menu beside it. One click moves to the
    /// next of edit, half edit, and preview.
    fn render_mode_switch(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (icon, tooltip) = match self.mode.next() {
            ViewMode::Half => (Lucide::Columns2, "Half edit"),
            ViewMode::Reading => (Lucide::BookOpen, "Preview the rendered note"),
            ViewMode::Editing => (Lucide::Pencil, "Edit the Markdown"),
        };
        let path = self.path.clone();
        h_flex()
            .gap_1()
            .items_center()
            .child(
                Button::new("note-mode")
                    .icon(Icon::new(icon))
                    .outline()
                    .small()
                    .selected(self.mode != ViewMode::Editing)
                    .tooltip_with_action(tooltip, &ToggleReadingView, Some(WORKSPACE_CONTEXT))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_mode(window, cx))),
            )
            .child(
                Button::new("note-more")
                    .icon(Icon::new(Lucide::Ellipsis))
                    .ghost()
                    .small()
                    .tooltip("More options")
                    .dropdown_menu_with_anchor(Anchor::BottomRight, move |menu, _, _| {
                        let (open, copy, show) = (path.clone(), path.clone(), path.clone());
                        menu.item(PopupMenuItem::new("Open in new tab").on_click(
                            move |_, window, cx| {
                                workspace::open_note(
                                    open.clone(),
                                    OpenOptions::new().in_new_tab(),
                                    window,
                                    cx,
                                );
                            },
                        ))
                        .item(PopupMenuItem::new("Reveal in navigation").on_click(
                            move |_, window, cx| {
                                workspace::show_panel("file-explorer:files", window, cx);
                            },
                        ))
                        .separator()
                        .item(
                            PopupMenuItem::new("Copy path")
                                .icon(IconName::Copy)
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copy.to_string(),
                                    ));
                                }),
                        )
                        .item(
                            PopupMenuItem::new(if cfg!(target_os = "macos") {
                                "Reveal in Finder"
                            } else {
                                "Show in system explorer"
                            })
                            .on_click(move |_, _, cx| {
                                let path = Notia::vault(cx).read(cx).vault().abs_path(&show);
                                cx.reveal_path(&path);
                            }),
                        )
                    }),
            )
    }

    fn render_source(&self, appearance: &EditorAppearance, half: bool) -> impl IntoElement {
        let editor = self.editor.clone();
        Editor::new(&self.editor)
            .appearance(false)
            .h(relative(1.))
            .font_family(appearance.font_family.clone())
            .text_size(appearance.font_size)
            .line_height(relative(appearance.line_height))
            .when(half, |editor| editor.px_4())
            .when(!half, |editor| editor.px_8())
            .on_paste(move |item, window, cx| paste_image(&editor, item, window, cx))
    }

    fn render_reading(
        &self,
        appearance: &EditorAppearance,
        root: std::path::PathBuf,
        half: bool,
    ) -> impl IntoElement {
        let from = self.path.clone();
        TextView::new(&self.reading)
            .markdown_extensions(self.extensions.clone())
            .scrollable(true)
            .selectable(true)
            .image_source(move |uri| image_in_vault(root.clone(), uri))
            .on_link_click(move |url, event, window, cx| match parse_note_url(url) {
                Some(NoteUrl::Note(target)) => {
                    let new_tab = event.modifiers().secondary(cx);
                    workspace::open_link(&target, Some(from.clone()), new_tab, window, cx);
                }
                Some(NoteUrl::Tag(tag)) => {
                    let query = format!("tag:#{tag}");
                    Notia::workspace(cx)
                        .update(cx, |model, cx| model.request_search(query.into(), cx));
                }
                None => cx.open_url(url),
            })
            .font_family(appearance.font_family.clone())
            .text_size(appearance.font_size)
            .line_height(relative(appearance.line_height))
            .size_full()
            .when(half, |view| view.px_4().pb_8())
            .when(!half, |view| view.px_8().pb_16())
    }
}

impl Focusable for NoteEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for NoteEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let appearance = EditorAppearance::global(cx).clone();
        let theme = cx.theme();
        let (danger, muted) = (theme.danger, theme.muted_foreground);
        // The text column plus the `px_8` inset on both sides.
        let inset = rems(4.).to_pixels(window.rem_size());

        let title_size = appearance.font_size * 1.9;
        let title = Input::new(&self.title)
            .appearance(false)
            .font_family(appearance.font_family.clone())
            .text_size(title_size)
            .line_height(relative(1.3))
            .h(title_size * 1.6)
            .font_weight(FontWeight::BOLD)
            .px_8();

        let root = Notia::vault(cx).read(cx).vault().root().to_path_buf();
        let body = if let Some(board) = self.canvas.clone() {
            board.into_any_element()
        } else if let Some(board) = self.kanban.clone() {
            board.into_any_element()
        } else {
            match self.mode {
                ViewMode::Editing => self.render_source(&appearance, false).into_any_element(),
                ViewMode::Half => {
                    let border = theme.border;
                    h_flex()
                        .size_full()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .overflow_hidden()
                                .child(self.render_source(&appearance, true)),
                        )
                        .child(div().w_px().h_full().flex_none().bg(border))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .overflow_hidden()
                                .child(self.render_reading(&appearance, root.clone(), true)),
                        )
                        .into_any_element()
                }
                ViewMode::Reading => self
                    .render_reading(&appearance, root, false)
                    .into_any_element(),
            }
        };

        v_flex()
            .id("note-editor")
            .key_context("NoteEditor")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &Paste, window, cx| {
                // The editor handles paste while it is on screen. In preview
                // the note itself is focused, so the image is saved here.
                if this.mode != ViewMode::Reading || this.is_board() {
                    return;
                }
                let Some(item) = cx.read_from_clipboard() else {
                    return;
                };
                let editor = this.editor.clone();
                paste_image(&editor, &item, window, cx);
            }))
            .size_full()
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    h_flex()
                        .gap_2()
                        .px_4()
                        .py_1()
                        .text_sm()
                        .text_color(danger)
                        .child(Icon::new(IconName::TriangleAlert).small())
                        .child(error),
                )
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .justify_center()
                    .items_stretch()
                    .child(
                        v_flex()
                            .h_full()
                            .w_full()
                            .pt_6()
                            .when(!self.is_board(), |this| {
                                this.when_some(appearance.max_width, |this, width| {
                                    this.max_w(width + inset)
                                })
                            })
                            .children(self.toolbars.iter().map(|toolbar| {
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .flex_none()
                                    .px_8()
                                    .pb_2()
                                    .child(toolbar.clone())
                            }))
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_center()
                                    .pr_8()
                                    .child(div().flex_1().min_w_0().child(title))
                                    .when(!self.is_board(), |row| {
                                        row.child(self.render_mode_switch(cx))
                                    }),
                            )
                            .child(
                                div()
                                    .px_8()
                                    .pb_2()
                                    .truncate()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(
                                        self.path
                                            .parent()
                                            .filter(|parent| !parent.is_root())
                                            .map(|parent| parent.to_string())
                                            .unwrap_or_default(),
                                    ),
                            )
                            .child(div().flex_1().min_h_0().child(body)),
                    ),
            )
    }
}

/// Saves clipboard images into the vault and inserts a Markdown image.
/// Text pastes return false so the editor inserts them itself.
fn paste_image(
    editor: &Entity<EditorState>,
    item: &gpui_kit::ClipboardItem,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let vault = Notia::vault(cx);
    let saved = vault.update(cx, |model, cx| {
        let folder = AppSettings::global(cx).files.attachment_folder.clone();
        let saved = attachments::pasted_images(model.vault(), &folder, item);
        if matches!(saved, Ok(Some(_))) {
            model.rescan(cx);
        }
        saved
    });
    match saved {
        Ok(Some(markdown)) => {
            editor.update(cx, |editor, cx| editor.insert(markdown, window, cx));
            true
        }
        Ok(None) => false,
        Err(error) => {
            log::error!("could not save a pasted image: {error}");
            window.push_notification(
                Notification::error(format!("Could not save the pasted image: {error}")),
                cx,
            );
            true
        }
    }
}

fn selection_of(editor: &EditorState) -> EditorSelection {
    let range = editor.selected_range();
    if editor.is_selection_reversed() {
        EditorSelection::new(range.end, range.start)
    } else {
        EditorSelection::new(range.start, range.end)
    }
}

/// Draws `> [!type]` blockquotes as Obsidian-style callouts.
fn callout_extensions() -> MarkdownExtensions {
    CALLOUTS
        .iter()
        .fold(MarkdownExtensions::default(), |extensions, &kind| {
            extensions.block_surface(kind, move |_, cx| {
                let (color, icon, label) = callout_style(kind, cx);
                v_flex()
                    .gap_1()
                    .px_4()
                    .py_3()
                    .rounded(cx.theme().radius_lg)
                    .border_l_2()
                    .border_color(color)
                    .bg(color.opacity(0.08))
                    .child(
                        h_flex()
                            .gap_2()
                            .text_color(color)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(Icon::new(icon).small())
                            .child(label),
                    )
            })
        })
}

fn callout_style(kind: &str, cx: &App) -> (Hsla, Icon, &'static str) {
    use gpui_kit::assets::IconName as Lucide;

    let theme = cx.theme();
    match kind {
        "abstract" | "summary" | "tldr" => {
            (theme.cyan, Icon::new(Lucide::ClipboardList), "Abstract")
        }
        "info" => (theme.blue, Icon::new(IconName::Info), "Info"),
        "todo" => (theme.blue, Icon::new(IconName::CircleCheck), "Todo"),
        "tip" | "hint" | "important" => (theme.cyan, Icon::new(Lucide::Flame), "Tip"),
        "success" | "check" | "done" => (theme.green, Icon::new(IconName::Check), "Success"),
        "question" | "help" | "faq" => (theme.yellow, Icon::new(IconName::CircleAlert), "Question"),
        "warning" | "caution" | "attention" => {
            (theme.yellow, Icon::new(IconName::TriangleAlert), "Warning")
        }
        "failure" | "fail" | "missing" => (theme.red, Icon::new(IconName::CircleX), "Failure"),
        "danger" | "error" => (theme.red, Icon::new(Lucide::Zap), "Danger"),
        "bug" => (theme.red, Icon::new(Lucide::Bug), "Bug"),
        "example" => (theme.magenta, Icon::new(Lucide::List), "Example"),
        "quote" | "cite" => (theme.muted_foreground, Icon::new(Lucide::Quote), "Quote"),
        _ => (theme.blue, Icon::new(Lucide::Pencil), "Note"),
    }
}
