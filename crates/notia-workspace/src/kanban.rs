//! A Kanban note: columns of cards, saved as JSON.
//!
//! A new board starts with To do, Doing, and Done. Cards move by dragging,
//! and the board writes itself shortly after a change, the way a canvas does.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement as _,
    v_flex,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use serde::{Deserialize, Serialize};

use notia_plugin::{AppSettings, Notia, core::VaultPath};

const COLUMN_W: f32 = 224.;
const UNDO_LIMIT: usize = 60;

/// The columns of one board.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Board {
    columns: Vec<Column>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Column {
    id: String,
    title: String,
    cards: Vec<Card>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Card {
    id: String,
    text: String,
}

/// JSON for a board with To do, Doing, and Done, and no cards yet.
pub(crate) fn starter_json() -> String {
    Board::starter().to_json()
}

impl Board {
    fn starter() -> Self {
        Self {
            columns: vec![
                Column {
                    id: "1".into(),
                    title: "To do".into(),
                    cards: Vec::new(),
                },
                Column {
                    id: "2".into(),
                    title: "Doing".into(),
                    cards: Vec::new(),
                },
                Column {
                    id: "3".into(),
                    title: "Done".into(),
                    cards: Vec::new(),
                },
            ],
        }
    }

    /// An empty file becomes the starter board. JSON this parser does not
    /// understand becomes an empty board, so a later edit does not invent
    /// columns the file never had.
    fn parse(text: &str) -> Self {
        let trimmed = text.trim();
        let mut board = if trimmed.is_empty() {
            Self::starter()
        } else {
            serde_json::from_str(trimmed).unwrap_or_default()
        };
        board.normalize();
        board
    }

    fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    fn normalize(&mut self) {
        let mut next = self
            .columns
            .iter()
            .flat_map(|column| {
                std::iter::once(column.id.as_str())
                    .chain(column.cards.iter().map(|card| card.id.as_str()))
            })
            .filter_map(|id| id.parse::<u64>().ok())
            .max()
            .map_or(1, |highest| highest + 1);
        for column in &mut self.columns {
            if column.id.is_empty() {
                column.id = next.to_string();
                next += 1;
            }
            if column.title.trim().is_empty() {
                column.title = "Untitled".to_string();
            }
            for card in &mut column.cards {
                if card.id.is_empty() {
                    card.id = next.to_string();
                    next += 1;
                }
            }
        }
    }

    fn next_id(&self) -> String {
        self.columns
            .iter()
            .flat_map(|column| {
                std::iter::once(column.id.as_str())
                    .chain(column.cards.iter().map(|card| card.id.as_str()))
            })
            .filter_map(|id| id.parse::<u64>().ok())
            .max()
            .map_or(1, |highest| highest + 1)
            .to_string()
    }

    fn column(&self, id: &str) -> Option<&Column> {
        self.columns.iter().find(|column| column.id == id)
    }

    fn card(&self, id: &str) -> Option<&Card> {
        self.columns
            .iter()
            .find_map(|column| column.cards.iter().find(|card| card.id == id))
    }

    fn locate(&self, id: &str) -> Option<(usize, usize)> {
        self.columns
            .iter()
            .enumerate()
            .find_map(|(column_ix, column)| {
                column
                    .cards
                    .iter()
                    .position(|card| card.id == id)
                    .map(|card_ix| (column_ix, card_ix))
            })
    }

    fn rename_column(&mut self, id: &str, title: String) {
        if let Some(column) = self.columns.iter_mut().find(|column| column.id == id) {
            column.title = title;
        }
    }

    fn remove_column(&mut self, id: &str) {
        self.columns.retain(|column| column.id != id);
    }

    fn set_card_text(&mut self, id: &str, text: String) {
        if let Some(card) = self
            .columns
            .iter_mut()
            .find_map(|column| column.cards.iter_mut().find(|card| card.id == id))
        {
            card.text = text;
        }
    }

    fn remove_card(&mut self, id: &str) {
        if let Some((column_ix, card_ix)) = self.locate(id) {
            self.columns[column_ix].cards.remove(card_ix);
        }
    }

    /// `index` is where the card would sit in the column as it is now, before
    /// the card leaves its current place. Dropping a card on itself, or in
    /// the gap just after it, changes nothing.
    fn accepts_move(&self, id: &str, column_id: &str, index: usize) -> bool {
        let Some((from_col, from_ix)) = self.locate(id) else {
            return false;
        };
        let Some(to_col) = self
            .columns
            .iter()
            .position(|column| column.id == column_id)
        else {
            return false;
        };
        !(from_col == to_col && (from_ix == index || from_ix + 1 == index))
    }

    fn move_card(&mut self, id: &str, column_id: &str, mut index: usize) -> bool {
        if !self.accepts_move(id, column_id, index) {
            return false;
        }
        let (from_col, from_ix) = self.locate(id).expect("accepts_move found the card");
        let to_col = self
            .columns
            .iter()
            .position(|column| column.id == column_id)
            .expect("accepts_move found the column");
        let card = self.columns[from_col].cards.remove(from_ix);
        if from_col == to_col && from_ix < index {
            index -= 1;
        }
        let cards = &mut self.columns[to_col].cards;
        let index = index.min(cards.len());
        cards.insert(index, card);
        true
    }
}

/// The card being dragged, and the words the ghost shows.
#[derive(Clone)]
struct CardDrag {
    id: String,
    text: SharedString,
}

struct CardGhost {
    text: SharedString,
}

impl Render for CardGhost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let empty = self.text.is_empty();
        div()
            .w(px(240.))
            .px_3()
            .py_2()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.accent)
            .bg(theme.background)
            .text_sm()
            .whitespace_normal()
            .line_clamp(4)
            .text_color(if empty {
                theme.muted_foreground
            } else {
                theme.foreground
            })
            .child(if empty {
                SharedString::from("Empty card")
            } else {
                self.text.clone()
            })
    }
}

#[derive(Clone)]
enum Editing {
    Column {
        id: String,
        field: Entity<InputState>,
    },
    Card {
        id: String,
        field: Entity<InputState>,
    },
}

impl Editing {
    fn field(&self) -> Entity<InputState> {
        match self {
            Self::Column { field, .. } | Self::Card { field, .. } => field.clone(),
        }
    }

    fn column_id(&self) -> Option<&str> {
        match self {
            Self::Column { id, .. } => Some(id),
            Self::Card { .. } => None,
        }
    }

    fn card_id(&self) -> Option<&str> {
        match self {
            Self::Card { id, .. } => Some(id),
            Self::Column { .. } => None,
        }
    }
}

/// Tells the note to publish the board. The text rides on the event so the
/// note can store it without reading this view from inside its own update.
pub(crate) struct KanbanEvent {
    pub document: SharedString,
}

/// One open Kanban note.
pub(crate) struct KanbanView {
    focus_handle: FocusHandle,
    path: VaultPath,
    board: Board,
    /// The board as last written, so a save can tell a change from a no-op.
    saved: Board,
    undo: Vec<Board>,
    redo: Vec<Board>,
    selected: Option<String>,
    editing: Option<Editing>,
    edit_subscription: Option<Subscription>,
    /// Where a dragged card would land: the column, and the index in that
    /// column before the card is removed.
    drop_at: Option<(String, usize)>,
    /// Rank of the drop offered for the current pointer position. A card
    /// outranks the column around it.
    drop_rank: u8,
    /// The pointer position the rank belongs to, so a new move starts over.
    drop_pointer: Option<(f32, f32)>,
    dragging: Option<String>,
    error: Option<SharedString>,
    save_task: Option<Task<()>>,
}

impl EventEmitter<KanbanEvent> for KanbanView {}

impl KanbanView {
    pub(crate) fn new(path: VaultPath, text: &str, cx: &mut Context<Self>) -> Self {
        let board = Board::parse(text);
        Self {
            focus_handle: cx.focus_handle(),
            path,
            saved: board.clone(),
            board,
            undo: Vec::new(),
            redo: Vec::new(),
            selected: None,
            editing: None,
            edit_subscription: None,
            drop_at: None,
            drop_rank: 0,
            drop_pointer: None,
            dragging: None,
            error: None,
            save_task: None,
        }
    }

    pub(crate) fn document(&self) -> SharedString {
        self.board.to_json().into()
    }

    fn change(&mut self, mutate: impl FnOnce(&mut Board), cx: &mut Context<Self>) {
        let before = self.board.clone();
        mutate(&mut self.board);
        if self.board == before {
            return;
        }
        self.undo.push(before);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.mark_edited(cx);
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        let Some(previous) = self.undo.pop() else {
            return;
        };
        self.cancel_edit();
        self.redo.push(std::mem::replace(&mut self.board, previous));
        if self.redo.len() > UNDO_LIMIT {
            self.redo.remove(0);
        }
        self.mark_edited(cx);
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        let Some(next) = self.redo.pop() else {
            return;
        };
        self.cancel_edit();
        self.undo.push(std::mem::replace(&mut self.board, next));
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.mark_edited(cx);
    }

    fn cancel_edit(&mut self) {
        self.editing = None;
        self.edit_subscription = None;
    }

    fn mark_edited(&mut self, cx: &mut Context<Self>) {
        let delay = Duration::from_millis(AppSettings::global(cx).editor.autosave_delay_ms);
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            _ = this.update(cx, |this, cx| this.save(cx));
        }));
        cx.emit(KanbanEvent {
            document: self.document(),
        });
        cx.notify();
    }

    /// Follows a rename so later writes land on the file the note is showing.
    pub(crate) fn set_path(&mut self, path: VaultPath, cx: &mut Context<Self>) {
        if self.path == path {
            return;
        }
        self.path = path;
        self.save(cx);
    }

    /// Writes unsaved changes now.
    pub(crate) fn save(&mut self, cx: &mut Context<Self>) {
        self.save_task = None;
        if self.board == self.saved {
            return;
        }
        let json = self.board.to_json();
        let path = self.path.clone();
        match Notia::vault(cx).update(cx, |vault, cx| vault.save_note(&path, &json, cx)) {
            Ok(()) => {
                self.saved = self.board.clone();
                self.error = None;
            }
            Err(error) => {
                self.error = Some(format!("Could not save this kanban: {error}").into());
            }
        }
        cx.notify();
    }

    fn add_column(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.board.next_id();
        let column_id = id.clone();
        self.change(
            move |board| {
                board.columns.push(Column {
                    id,
                    title: "Untitled".into(),
                    cards: Vec::new(),
                });
            },
            cx,
        );
        self.begin_column_edit(&column_id, window, cx);
    }

    fn delete_column(&mut self, id: String, cx: &mut Context<Self>) {
        let editing_here = self.editing.as_ref().is_some_and(|edit| {
            edit.column_id() == Some(id.as_str())
                || edit.card_id().is_some_and(|card| {
                    self.board
                        .column(&id)
                        .is_some_and(|column| column.cards.iter().any(|item| item.id == card))
                })
        });
        if editing_here {
            self.cancel_edit();
        }
        if self.selected.as_ref().is_some_and(|selected| {
            self.board
                .column(&id)
                .is_some_and(|column| column.cards.iter().any(|card| card.id == *selected))
        }) {
            self.selected = None;
        }
        self.change(move |board| board.remove_column(&id), cx);
    }

    fn add_card(&mut self, column_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.board.next_id();
        let card_id = id.clone();
        let target = column_id.clone();
        self.change(
            move |board| {
                let Some(column) = board.columns.iter_mut().find(|column| column.id == target)
                else {
                    return;
                };
                column.cards.push(Card {
                    id,
                    text: String::new(),
                });
            },
            cx,
        );
        self.selected = Some(card_id.clone());
        self.begin_card_edit(&card_id, window, cx);
    }

    fn delete_selected(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        if self.board.card(&id).is_none() {
            return;
        }
        self.selected = None;
        if self
            .editing
            .as_ref()
            .is_some_and(|edit| edit.card_id() == Some(id.as_str()))
        {
            self.cancel_edit();
        }
        self.change(move |board| board.remove_card(&id), cx);
    }

    fn delete_card(&mut self, id: String, cx: &mut Context<Self>) {
        self.selected = Some(id);
        self.delete_selected(cx);
    }

    fn begin_column_edit(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(title) = self.board.column(id).map(|column| column.title.clone()) else {
            return;
        };
        if self
            .editing
            .as_ref()
            .is_some_and(|edit| edit.column_id() == Some(id))
        {
            return;
        }
        self.open_field(
            EditingKind::Column(id.to_string()),
            title,
            "Column",
            window,
            cx,
        );
    }

    fn begin_card_edit(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.board.card(id).map(|card| card.text.clone()) else {
            return;
        };
        if self
            .editing
            .as_ref()
            .is_some_and(|edit| edit.card_id() == Some(id))
        {
            return;
        }
        self.selected = Some(id.to_string());
        self.open_field(EditingKind::Card(id.to_string()), text, "Card", window, cx);
    }

    fn open_field(
        &mut self,
        kind: EditingKind,
        text: String,
        placeholder: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(edit) = self.editing.clone() {
            self.commit_edit(&edit.field(), cx);
        }
        let field = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(text)
        });
        self.edit_subscription =
            Some(
                cx.subscribe_in(&field, window, |this, field, event, _, cx| match event {
                    InputEvent::PressEnter { .. } | InputEvent::Blur => this.commit_edit(field, cx),
                    _ => {}
                }),
            );
        self.editing = Some(match kind {
            EditingKind::Column(id) => Editing::Column {
                id,
                field: field.clone(),
            },
            EditingKind::Card(id) => Editing::Card {
                id,
                field: field.clone(),
            },
        });
        field.update(cx, |field, cx| {
            field.select_all(window, cx);
            field.focus(window, cx);
        });
        cx.notify();
    }

    /// Moves the field's text onto the board, once. A dropped field blurs,
    /// so the same commit runs again; clearing the edit first stops it.
    fn commit_edit(&mut self, field: &Entity<InputState>, cx: &mut Context<Self>) {
        let Some(edit) = self.editing.clone() else {
            return;
        };
        if edit.field() != *field {
            return;
        }
        self.editing = None;
        self.edit_subscription = None;
        let text = field.read(cx).value().trim().to_string();
        match edit {
            Editing::Column { id, .. } => {
                if self
                    .board
                    .column(&id)
                    .is_some_and(|column| column.title == text)
                {
                    cx.notify();
                    return;
                }
                let title = if text.is_empty() {
                    "Untitled".to_string()
                } else {
                    text
                };
                self.change(move |board| board.rename_column(&id, title), cx);
            }
            Editing::Card { id, .. } => {
                if self.board.card(&id).is_some_and(|card| card.text == text) {
                    cx.notify();
                    return;
                }
                self.change(move |board| board.set_card_text(&id, text), cx);
            }
        }
    }

    fn drop_card(&mut self, drag: &CardDrag, cx: &mut Context<Self>) {
        let Some((column, index)) = self.drop_at.take() else {
            return;
        };
        let id = drag.id.clone();
        if !self.board.accepts_move(&id, &column, index) {
            return;
        }
        self.selected = Some(id.clone());
        self.change(
            move |board| {
                board.move_card(&id, &column, index);
            },
            cx,
        );
    }

    /// Starts a drag move at `at`. A new position forgets the previous offer
    /// so a pointer that has left every column does not keep a stale target.
    /// Callers can run in any order: the first one for a position clears, and
    /// a later one sees the same position and keeps what it offers.
    fn note_pointer(&mut self, at: Point<Pixels>, drag_id: &str, cx: &mut Context<Self>) {
        if self.dragging.as_deref() != Some(drag_id) {
            self.dragging = Some(drag_id.to_string());
            cx.notify();
        }
        let key = (at.x.as_f32(), at.y.as_f32());
        if self.drop_pointer != Some(key) {
            self.drop_pointer = Some(key);
            self.drop_rank = 0;
            if self.drop_at.take().is_some() {
                cx.notify();
            }
        }
    }

    /// Remembers a drop target. A higher rank replaces a lower one, so a card
    /// wins over the column it sits in.
    fn offer_drop(
        &mut self,
        at: Point<Pixels>,
        rank: u8,
        slot: Option<(String, usize)>,
        drag_id: &str,
        cx: &mut Context<Self>,
    ) {
        self.note_pointer(at, drag_id, cx);
        if rank < self.drop_rank {
            return;
        }
        self.drop_rank = rank;
        if self.drop_at != slot {
            self.drop_at = slot;
            cx.notify();
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        let key = event.keystroke.key.as_str();
        let modifiers = &event.keystroke.modifiers;
        match key {
            "backspace" | "delete" => self.delete_selected(cx),
            "escape" => self.selected = None,
            "enter" => {
                if let Some(id) = self.selected.clone() {
                    self.begin_card_edit(&id, window, cx);
                }
            }
            "z" if modifiers.platform && modifiers.shift => self.redo(cx),
            "z" if modifiers.platform => self.undo(cx),
            "y" if modifiers.platform => self.redo(cx),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

enum EditingKind {
    Column(String),
    Card(String),
}

impl Focusable for KanbanView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for KanbanView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !cx.has_active_drag() {
            self.drop_at = None;
            self.drop_rank = 0;
            self.drop_pointer = None;
            self.dragging = None;
        }

        let theme = cx.theme();
        let (accent, border, surface, well, foreground, muted, danger, radius) = (
            theme.accent,
            theme.border,
            theme.background,
            theme.secondary,
            theme.foreground,
            theme.muted_foreground,
            theme.danger,
            theme.radius,
        );
        let board_columns = self.board.columns.clone();
        let drop_at = self.drop_at.clone();
        let selected = self.selected.clone();
        let dragging = self.dragging.clone();
        let editing = self.editing.clone();
        let error = self.error.clone();
        let can_undo = !self.undo.is_empty();
        let can_redo = !self.redo.is_empty();

        let mut column_views = Vec::new();
        for column in &board_columns {
            column_views.push(self.column_view(
                column,
                drop_at.as_ref(),
                selected.as_deref(),
                dragging.as_deref(),
                editing.as_ref(),
                accent,
                border,
                surface,
                well,
                foreground,
                muted,
                danger,
                radius,
                cx,
            ));
        }

        v_flex()
            .id("kanban")
            .key_context("Kanban")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(surface)
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<CardDrag>, _, cx| {
                let drag_id = event.drag(cx).id.clone();
                this.note_pointer(event.event.position, &drag_id, cx);
            }))
            .child(
                h_flex()
                    .flex_none()
                    .justify_between()
                    .items_center()
                    .gap_1()
                    .px_4()
                    .pt_3()
                    .child(
                        Button::new("kanban-add-column")
                            .icon(Icon::new(IconName::Plus))
                            .label("Add column")
                            .outline()
                            .small()
                            .tooltip("Add column")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_column(window, cx)),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(history_button(
                                cx,
                                "kanban-undo",
                                IconName::Undo2,
                                "Undo",
                                !can_undo,
                                |this, _, _, cx| this.undo(cx),
                            ))
                            .child(history_button(
                                cx,
                                "kanban-redo",
                                IconName::Redo2,
                                "Redo",
                                !can_redo,
                                |this, _, _, cx| this.redo(cx),
                            )),
                    ),
            )
            .child(
                div()
                    .id("kanban-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_x_scrollbar()
                    .child(
                        h_flex()
                            .h_full()
                            .items_stretch()
                            .gap_3()
                            .px_4()
                            .pt_3()
                            .pb_4()
                            .when(column_views.is_empty(), |row| {
                                row.child(
                                    div()
                                        .self_center()
                                        .text_sm()
                                        .text_color(muted)
                                        .child("Add a column to start."),
                                )
                            })
                            .children(column_views),
                    ),
            )
            .when_some(error, |this, error| {
                this.child(
                    h_flex()
                        .absolute()
                        .top_2()
                        .left_4()
                        .gap_2()
                        .text_sm()
                        .text_color(danger)
                        .child(Icon::new(IconName::TriangleAlert).small())
                        .child(error),
                )
            })
    }
}

impl KanbanView {
    fn column_view(
        &self,
        column: &Column,
        drop_at: Option<&(String, usize)>,
        selected: Option<&str>,
        dragging: Option<&str>,
        editing: Option<&Editing>,
        accent: Hsla,
        border: Hsla,
        surface: Hsla,
        well: Hsla,
        foreground: Hsla,
        muted: Hsla,
        danger: Hsla,
        radius: Pixels,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let drop_index = drop_at.and_then(|(id, index)| (id == &column.id).then_some(*index));
        let targeted = drop_index.is_some();
        let column_id = column.id.clone();
        let header_id = column_id.clone();
        let list_id = column_id.clone();
        let title_id = column_id.clone();
        let editing_title = editing.and_then(|edit| match edit {
            Editing::Column { id, field } if id == &column.id => Some(field.clone()),
            _ => None,
        });

        let mut cards: Vec<AnyElement> = Vec::new();
        for (ix, card) in column.cards.iter().enumerate() {
            if drop_index == Some(ix) {
                cards.push(drop_line(accent).into_any_element());
            }
            cards.push(
                self.card_view(
                    card,
                    &column.id,
                    ix,
                    selected == Some(card.id.as_str()),
                    dragging == Some(card.id.as_str()),
                    editing,
                    accent,
                    border,
                    surface,
                    foreground,
                    muted,
                    danger,
                    radius,
                    cx,
                )
                .into_any_element(),
            );
        }
        if drop_index == Some(column.cards.len()) {
            cards.push(drop_line(accent).into_any_element());
        }

        let count = column.cards.len();
        v_flex()
            .id(SharedString::from(format!("kanban-column-{}", column.id)))
            .w(px(COLUMN_W))
            .h_full()
            .flex_none()
            .rounded(radius)
            .bg(well)
            .border_1()
            .border_color(if targeted { accent } else { border })
            .child(
                h_flex()
                    .id(SharedString::from(format!("kanban-head-{}", column.id)))
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .pt_2()
                    .pb_1()
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<CardDrag>, _, cx| {
                            if !event.bounds.contains(&event.event.position) {
                                return;
                            }
                            let drag_id = event.drag(cx).id.clone();
                            let slot = this
                                .board
                                .accepts_move(&drag_id, &header_id, 0)
                                .then(|| (header_id.clone(), 0));
                            this.offer_drop(event.event.position, 1, slot, &drag_id, cx);
                        },
                    ))
                    .on_drop(cx.listener(|this, drag: &CardDrag, _, cx| this.drop_card(drag, cx)))
                    .child(match editing_title {
                        Some(field) => div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                Input::new(&field)
                                    .appearance(false)
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .w_full(),
                            )
                            .into_any_element(),
                        None => div()
                            .id(SharedString::from(format!("kanban-title-{}", column.id)))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.begin_column_edit(&title_id, window, cx);
                            }))
                            .child(column.title.clone())
                            .into_any_element(),
                    })
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(muted)
                            .child(count.to_string()),
                    )
                    .child(
                        Button::new(SharedString::from(format!("kanban-drop-col-{}", column.id)))
                            .icon(Icon::new(IconName::X))
                            .ghost()
                            .xsmall()
                            .text_color(muted)
                            .tooltip("Delete column")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.delete_column(column_id.clone(), cx);
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id(SharedString::from(format!("kanban-cards-{}", column.id)))
                    .flex_1()
                    .min_h_0()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .overflow_y_scrollbar()
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<CardDrag>, _, cx| {
                            if !event.bounds.contains(&event.event.position) {
                                return;
                            }
                            let drag_id = event.drag(cx).id.clone();
                            let index = this
                                .board
                                .column(&list_id)
                                .map(|column| column.cards.len())
                                .unwrap_or(0);
                            let slot = this
                                .board
                                .accepts_move(&drag_id, &list_id, index)
                                .then(|| (list_id.clone(), index));
                            this.offer_drop(event.event.position, 1, slot, &drag_id, cx);
                        },
                    ))
                    .on_drop(cx.listener(|this, drag: &CardDrag, _, cx| this.drop_card(drag, cx)))
                    .children(cards),
            )
            .child(
                div().flex_none().px_2().pb_2().child(
                    Button::new(SharedString::from(format!("kanban-add-{}", column.id)))
                        .icon(Icon::new(IconName::Plus))
                        .label("Add card")
                        .ghost()
                        .small()
                        .w_full()
                        .tooltip("Add card")
                        .on_click({
                            let column_id = column.id.clone();
                            cx.listener(move |this, _, window, cx| {
                                this.add_card(column_id.clone(), window, cx);
                            })
                        }),
                ),
            )
    }

    fn card_view(
        &self,
        card: &Card,
        column_id: &str,
        index: usize,
        selected: bool,
        dragging: bool,
        editing: Option<&Editing>,
        accent: Hsla,
        border: Hsla,
        surface: Hsla,
        foreground: Hsla,
        muted: Hsla,
        danger: Hsla,
        radius: Pixels,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let id = card.id.clone();
        let column_id = column_id.to_string();
        let editing_field = editing.and_then(|edit| match edit {
            Editing::Card {
                id: editing_id,
                field,
            } if editing_id == &card.id => Some(field.clone()),
            _ => None,
        });
        let empty = card.text.is_empty();
        let drag = CardDrag {
            id: card.id.clone(),
            text: card.text.clone().into(),
        };
        let select_id = id.clone();
        let edit_id = id.clone();
        let delete_id = id.clone();

        div()
            .id(SharedString::from(format!("kanban-card-{id}")))
            .relative()
            .w_full()
            .min_h(px(36.))
            .px_3()
            .py_2()
            .rounded(radius)
            .border_1()
            .border_color(if selected { accent } else { border })
            .bg(surface)
            .opacity(if dragging { 0.4 } else { 1. })
            .when(editing_field.is_none(), |card| {
                card.on_drag(drag, |drag, _, _, cx| {
                    let text = drag.text.clone();
                    cx.new(|_| CardGhost { text })
                })
                .on_click(cx.listener(
                    move |this, event: &ClickEvent, window, cx| {
                        if event.click_count() > 1 {
                            this.begin_card_edit(&edit_id, window, cx);
                        } else {
                            this.selected = Some(select_id.clone());
                            this.focus_handle.focus(window, cx);
                            cx.notify();
                        }
                    },
                ))
            })
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<CardDrag>, _, cx| {
                    if !event.bounds.contains(&event.event.position) {
                        return;
                    }
                    let drag_id = event.drag(cx).id.clone();
                    let top = event.bounds.origin.y.as_f32();
                    let height = event.bounds.size.height.as_f32();
                    let index = if event.event.position.y.as_f32() < top + height * 0.5 {
                        index
                    } else {
                        index + 1
                    };
                    let slot = this
                        .board
                        .accepts_move(&drag_id, &column_id, index)
                        .then(|| (column_id.clone(), index));
                    this.offer_drop(event.event.position, 2, slot, &drag_id, cx);
                }),
            )
            .on_drop(cx.listener(|this, drag: &CardDrag, _, cx| this.drop_card(drag, cx)))
            .child(match editing_field {
                Some(field) => div()
                    .w_full()
                    .pr(px(24.))
                    .child(Input::new(&field).appearance(false).text_sm().w_full()),
                None => div()
                    .w_full()
                    .pr(if selected { px(24.) } else { px(0.) })
                    .text_sm()
                    .whitespace_normal()
                    .line_clamp(8)
                    .text_color(if empty { muted } else { foreground })
                    .child(if empty {
                        "Empty card".to_string()
                    } else {
                        card.text.clone()
                    }),
            })
            .when(selected, |card| {
                card.child(
                    div().absolute().top(px(4.)).right(px(4.)).child(
                        Button::new(SharedString::from(format!("kanban-del-{delete_id}")))
                            .icon(Icon::new(IconName::Trash))
                            .ghost()
                            .xsmall()
                            .text_color(danger)
                            .tooltip("Delete card")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.delete_card(delete_id.clone(), cx);
                            })),
                    ),
                )
            })
    }
}

fn drop_line(color: Hsla) -> Div {
    div()
        .h(px(3.))
        .w_full()
        .flex_none()
        .rounded(px(2.))
        .bg(color)
}

fn history_button(
    cx: &mut Context<KanbanView>,
    name: &'static str,
    icon: IconName,
    tooltip: &'static str,
    disabled: bool,
    handler: impl Fn(&mut KanbanView, &ClickEvent, &mut Window, &mut Context<KanbanView>) + 'static,
) -> Button {
    Button::new(name)
        .icon(Icon::new(icon))
        .ghost()
        .small()
        .disabled(disabled)
        .tooltip(tooltip)
        .on_click(cx.listener(handler))
}

#[cfg(test)]
mod tests {
    use super::{Board, Card};

    fn texts<'a>(board: &'a Board, column: &str) -> Vec<&'a str> {
        board
            .column(column)
            .unwrap()
            .cards
            .iter()
            .map(|card| card.text.as_str())
            .collect()
    }

    #[test]
    fn starter_roundtrips_and_blank_files_use_it() {
        let starter = Board::starter();
        assert_eq!(
            starter
                .columns
                .iter()
                .map(|column| column.title.as_str())
                .collect::<Vec<_>>(),
            vec!["To do", "Doing", "Done"]
        );
        assert_eq!(Board::parse(&starter.to_json()), starter);
        assert_eq!(Board::parse(""), starter);
        assert_eq!(Board::parse("   "), starter);
        assert!(Board::parse("not json").columns.is_empty());
    }

    #[test]
    fn fills_missing_ids_and_titles() {
        let board = Board::parse(r#"{"columns":[{"title":"","cards":[{"text":"Ship it"}]}]}"#);
        assert_eq!(board.columns.len(), 1);
        assert_eq!(board.columns[0].title, "Untitled");
        assert_eq!(board.columns[0].id, "1");
        assert_eq!(board.columns[0].cards[0].text, "Ship it");
        assert_eq!(board.columns[0].cards[0].id, "2");
    }

    #[test]
    fn moves_a_card_within_and_across_columns() {
        let mut board = Board::starter();
        board.columns[0].cards.push(Card {
            id: "4".into(),
            text: "A".into(),
        });
        board.columns[0].cards.push(Card {
            id: "5".into(),
            text: "B".into(),
        });
        board.columns[2].cards.push(Card {
            id: "6".into(),
            text: "C".into(),
        });

        assert!(!board.move_card("4", "1", 0));
        assert!(!board.move_card("4", "1", 1));
        assert!(board.move_card("4", "1", 2));
        assert_eq!(texts(&board, "1"), vec!["B", "A"]);
        assert!(!board.move_card("5", "1", 1));

        assert!(board.move_card("4", "3", 0));
        assert_eq!(texts(&board, "1"), vec!["B"]);
        assert_eq!(texts(&board, "3"), vec!["A", "C"]);

        assert!(board.move_card("6", "3", 0));
        assert_eq!(texts(&board, "3"), vec!["C", "A"]);
    }
}
