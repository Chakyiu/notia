//! A Kanban note: columns of cards, saved as JSON.
//!
//! A new board starts with To do, Doing, and Done. A card is Markdown: Enter
//! finishes it, and Shift+Enter starts another line. Cards move by dragging,
//! and the board writes itself shortly after a change, the way a canvas does.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    scroll::ScrollableElement as _,
    slider::{Slider, SliderEvent, SliderState},
    text::{TextView, TextViewStyle},
    v_flex,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use serde::{Deserialize, Serialize};

use notia_plugin::{AppSettings, Notia, core::VaultPath, workspace};

use crate::attachments::image_in_vault;
use crate::reading::{NoteUrl, parse_note_url, to_reading_markdown};

const UNDO_LIMIT: usize = 60;
const COLUMN_WIDTH_RANGE: (f32, f32) = (160., 420.);
const CARD_LINES_RANGE: (f32, f32) = (3., 16.);
const COLUMN_GAP_RANGE: (f32, f32) = (4., 32.);

/// The columns of one board.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Board {
    columns: Vec<Column>,
    layout: Layout,
}

/// How this board is drawn. Saved with the board, so each kanban keeps its own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Layout {
    column_width: f32,
    card_lines: f32,
    gap: f32,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            column_width: 224.,
            card_lines: 7.,
            gap: 12.,
        }
    }
}

impl Layout {
    fn clamped(self) -> Self {
        Self {
            column_width: self
                .column_width
                .clamp(COLUMN_WIDTH_RANGE.0, COLUMN_WIDTH_RANGE.1),
            card_lines: self
                .card_lines
                .clamp(CARD_LINES_RANGE.0, CARD_LINES_RANGE.1),
            gap: self.gap.clamp(COLUMN_GAP_RANGE.0, COLUMN_GAP_RANGE.1),
        }
    }

    fn column_width(&self) -> f32 {
        self.column_width
            .clamp(COLUMN_WIDTH_RANGE.0, COLUMN_WIDTH_RANGE.1)
    }

    fn card_lines(&self) -> f32 {
        self.card_lines
            .clamp(CARD_LINES_RANGE.0, CARD_LINES_RANGE.1)
    }

    fn gap(&self) -> f32 {
        self.gap.clamp(COLUMN_GAP_RANGE.0, COLUMN_GAP_RANGE.1)
    }
}

#[derive(Clone, Copy)]
enum LayoutField {
    Width,
    Lines,
    Gap,
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
            layout: Layout::default(),
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
        board.layout = board.layout.clamped();
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
        field: Entity<TextareaState>,
    },
}

impl Editing {
    fn column_field(&self) -> Option<Entity<InputState>> {
        match self {
            Self::Column { field, .. } => Some(field.clone()),
            Self::Card { .. } => None,
        }
    }

    fn card_field(&self) -> Option<Entity<TextareaState>> {
        match self {
            Self::Card { field, .. } => Some(field.clone()),
            Self::Column { .. } => None,
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
    /// The board before the current settings drag, so the drag is one undo step.
    layout_drag: Option<Board>,
    settings_open: bool,
    width_slider: Entity<SliderState>,
    lines_slider: Entity<SliderState>,
    gap_slider: Entity<SliderState>,
    _slider_subs: Vec<Subscription>,
    error: Option<SharedString>,
    save_task: Option<Task<()>>,
}

impl EventEmitter<KanbanEvent> for KanbanView {}

impl KanbanView {
    pub(crate) fn new(path: VaultPath, text: &str, cx: &mut Context<Self>) -> Self {
        let board = Board::parse(text);
        let width_slider =
            cx.new(|_| layout_slider(COLUMN_WIDTH_RANGE, 4., board.layout.column_width()));
        let lines_slider =
            cx.new(|_| layout_slider(CARD_LINES_RANGE, 1., board.layout.card_lines()));
        let gap_slider = cx.new(|_| layout_slider(COLUMN_GAP_RANGE, 2., board.layout.gap()));
        let slider_subs = vec![
            cx.subscribe(&width_slider, |this, _, event: &SliderEvent, cx| {
                this.on_layout_slider(LayoutField::Width, event, cx);
            }),
            cx.subscribe(&lines_slider, |this, _, event: &SliderEvent, cx| {
                this.on_layout_slider(LayoutField::Lines, event, cx);
            }),
            cx.subscribe(&gap_slider, |this, _, event: &SliderEvent, cx| {
                this.on_layout_slider(LayoutField::Gap, event, cx);
            }),
        ];
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
            layout_drag: None,
            settings_open: false,
            width_slider,
            lines_slider,
            gap_slider,
            _slider_subs: slider_subs,
            error: None,
            save_task: None,
        }
    }

    pub(crate) fn document(&self) -> SharedString {
        self.board.to_json().into()
    }

    fn on_layout_slider(
        &mut self,
        field: LayoutField,
        event: &SliderEvent,
        cx: &mut Context<Self>,
    ) {
        let value = match event {
            SliderEvent::Change(value) | SliderEvent::Release(value) => value.start(),
        };
        match event {
            SliderEvent::Change(_) => {
                if !self.layout_would_change(field, value) {
                    return;
                }
                if self.layout_drag.is_none() {
                    self.layout_drag = Some(self.board.clone());
                }
                self.write_layout(field, value);
                self.mark_edited(cx);
            }
            SliderEvent::Release(_) => self.finish_layout_drag(cx),
        }
    }

    fn layout_would_change(&self, field: LayoutField, value: f32) -> bool {
        let layout = &self.board.layout;
        match field {
            LayoutField::Width => (layout.column_width() - value).abs() >= 0.5,
            LayoutField::Lines => (layout.card_lines() - value).abs() >= 0.5,
            LayoutField::Gap => (layout.gap() - value).abs() >= 0.5,
        }
    }

    fn write_layout(&mut self, field: LayoutField, value: f32) {
        match field {
            LayoutField::Width => {
                self.board.layout.column_width =
                    value.clamp(COLUMN_WIDTH_RANGE.0, COLUMN_WIDTH_RANGE.1);
            }
            LayoutField::Lines => {
                self.board.layout.card_lines = value.clamp(CARD_LINES_RANGE.0, CARD_LINES_RANGE.1);
            }
            LayoutField::Gap => {
                self.board.layout.gap = value.clamp(COLUMN_GAP_RANGE.0, COLUMN_GAP_RANGE.1);
            }
        }
    }

    fn finish_layout_drag(&mut self, cx: &mut Context<Self>) {
        let Some(before) = self.layout_drag.take() else {
            return;
        };
        if before.layout == self.board.layout {
            return;
        }
        self.undo.push(before);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.mark_edited(cx);
    }

    fn reset_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.board.layout == Layout::default() {
            return;
        }
        let before = self.board.clone();
        self.board.layout = Layout::default();
        self.undo.push(before);
        if self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.sync_sliders(window, cx);
        self.mark_edited(cx);
    }

    /// Keeps the sliders on the saved layout after undo, redo, or a reset.
    fn sync_sliders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let layout = self.board.layout.clone();
        sync_slider(&self.width_slider, layout.column_width(), window, cx);
        sync_slider(&self.lines_slider, layout.card_lines(), window, cx);
        sync_slider(&self.gap_slider, layout.gap(), window, cx);
    }

    fn settings_panel(
        &self,
        muted: Hsla,
        well: Hsla,
        border: Hsla,
        radius: Pixels,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let layout = &self.board.layout;
        v_flex()
            .mx_4()
            .mt_3()
            .p_3()
            .gap_3()
            .rounded(radius)
            .border_1()
            .border_color(border)
            .bg(well)
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Board"),
                    )
                    .child(
                        Button::new(("kanban-layout-reset", cx.entity_id().as_u64()))
                            .label("Reset")
                            .ghost()
                            .xsmall()
                            .tooltip("Use the usual sizes")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.reset_layout(window, cx)),
                            ),
                    ),
            )
            .child(setting_row(
                "Column width",
                format!("{:.0} px", layout.column_width()),
                &self.width_slider,
                muted,
            ))
            .child(setting_row(
                "Card height",
                format!("{:.0} lines", layout.card_lines()),
                &self.lines_slider,
                muted,
            ))
            .child(setting_row(
                "Column gap",
                format!("{:.0} px", layout.gap()),
                &self.gap_slider,
                muted,
            ))
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
        self.open_column(id.to_string(), title, window, cx);
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
        self.open_card(id.to_string(), text, window, cx);
    }

    fn open_column(
        &mut self,
        id: String,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_current(cx);
        let field = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Column")
                .default_value(text)
        });
        self.edit_subscription = Some(cx.subscribe_in(
            &field,
            window,
            |this, field, event, _, cx| match event {
                InputEvent::PressEnter { .. } | InputEvent::Blur => this.commit_column(field, cx),
                _ => {}
            },
        ));
        self.editing = Some(Editing::Column {
            id,
            field: field.clone(),
        });
        field.update(cx, |field, cx| {
            field.select_all(window, cx);
            field.focus(window, cx);
        });
        cx.notify();
    }

    /// A card is a textarea: Enter writes it, and Shift+Enter inserts a line.
    fn open_card(&mut self, id: String, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_current(cx);
        let field = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Card")
                .default_value(text)
                .submit_on_enter(true)
                .auto_grow(1, 10_000)
        });
        self.edit_subscription =
            Some(
                cx.subscribe_in(&field, window, |this, field, event, _, cx| match event {
                    InputEvent::PressEnter { shift: false, .. } | InputEvent::Blur => {
                        this.commit_card(field, cx)
                    }
                    _ => {}
                }),
            );
        self.editing = Some(Editing::Card {
            id,
            field: field.clone(),
        });
        field.update(cx, |field, cx| {
            field.select_all(window, cx);
            field.focus(window, cx);
        });
        cx.notify();
    }

    fn commit_current(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.editing.clone() else {
            return;
        };
        match edit {
            Editing::Column { field, .. } => self.commit_column(&field, cx),
            Editing::Card { field, .. } => self.commit_card(&field, cx),
        }
    }

    /// Moves the field's text onto the board, once. A dropped field blurs,
    /// so the same commit runs again; clearing the edit first stops it.
    fn commit_column(&mut self, field: &Entity<InputState>, cx: &mut Context<Self>) {
        let Some(edit) = self.editing.clone() else {
            return;
        };
        let Some(current) = edit.column_field() else {
            return;
        };
        if current != *field {
            return;
        }
        let Editing::Column { id, .. } = edit else {
            return;
        };
        self.editing = None;
        self.edit_subscription = None;
        let text = field.read(cx).value().trim().to_string();
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

    fn commit_card(&mut self, field: &Entity<TextareaState>, cx: &mut Context<Self>) {
        let Some(edit) = self.editing.clone() else {
            return;
        };
        let Some(current) = edit.card_field() else {
            return;
        };
        if current != *field {
            return;
        }
        let Editing::Card { id, .. } = edit else {
            return;
        };
        self.editing = None;
        self.edit_subscription = None;
        let text = card_body(&field.read(cx).value());
        if self.board.card(&id).is_some_and(|card| card.text == text) {
            cx.notify();
            return;
        }
        self.change(move |board| board.set_card_text(&id, text), cx);
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
            "escape" if self.settings_open => self.settings_open = false,
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

impl Focusable for KanbanView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for KanbanView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let card_max = card_max_height(window, self.board.layout.card_lines());
        self.sync_sliders(window, cx);
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

        let settings = self
            .settings_open
            .then(|| self.settings_panel(muted, well, border, radius, cx));

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
                card_max,
                cx,
            ));
        }

        v_flex()
            .id(("kanban", cx.entity_id().as_u64()))
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
                        Button::new(("kanban-add-column", cx.entity_id().as_u64()))
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
                            .child(
                                Button::new(("kanban-settings", cx.entity_id().as_u64()))
                                    .icon(Icon::new(IconName::SlidersHorizontal))
                                    .ghost()
                                    .small()
                                    .selected(self.settings_open)
                                    .tooltip("Board settings")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.settings_open = !this.settings_open;
                                        cx.notify();
                                    })),
                            )
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
            .when_some(settings, |board, panel| board.child(panel))
            .child(
                div()
                    .id(("kanban-scroll", cx.entity_id().as_u64()))
                    .flex_1()
                    .min_h_0()
                    .overflow_x_scrollbar()
                    .child(
                        h_flex()
                            .h_full()
                            .items_stretch()
                            .gap(px(self.board.layout.gap()))
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
        card_max: Pixels,
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
                    card_max,
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
            .id(SharedString::from(format!(
                "kanban-column-{}-{}",
                cx.entity_id().as_u64(),
                column.id
            )))
            .w(px(self.board.layout.column_width()))
            .h_full()
            .flex_none()
            .rounded(radius)
            .bg(well)
            .border_1()
            .border_color(if targeted { accent } else { border })
            .child(
                h_flex()
                    .id(SharedString::from(format!(
                        "kanban-head-{}-{}",
                        cx.entity_id().as_u64(),
                        column.id
                    )))
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
                            .id(SharedString::from(format!(
                                "kanban-title-{}-{}",
                                cx.entity_id().as_u64(),
                                column.id
                            )))
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
                        Button::new(SharedString::from(format!(
                            "kanban-drop-col-{}-{}",
                            cx.entity_id().as_u64(),
                            column.id
                        )))
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
                    .id(SharedString::from(format!(
                        "kanban-cards-{}-{}",
                        cx.entity_id().as_u64(),
                        column.id
                    )))
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
                    Button::new(SharedString::from(format!(
                        "kanban-add-{}-{}",
                        cx.entity_id().as_u64(),
                        column.id
                    )))
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
        card_max: Pixels,
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
            .id(SharedString::from(format!(
                "kanban-card-{}-{id}",
                cx.entity_id().as_u64()
            )))
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
                Some(field) => card_scroll(
                    &format!("{}-{id}", cx.entity_id().as_u64()),
                    card_max,
                    Textarea::new(&field)
                        .appearance(false)
                        .small()
                        .text_sm()
                        .w_full(),
                )
                .into_any_element(),
                None if empty => div()
                    .w_full()
                    .text_sm()
                    .text_color(muted)
                    .child("Empty card")
                    .into_any_element(),
                None => card_scroll(
                    &format!("{}-{id}", cx.entity_id().as_u64()),
                    card_max,
                    div()
                        .w_full()
                        .pr(if selected { px(24.) } else { px(0.) })
                        .text_sm()
                        .text_color(foreground)
                        .child(self.card_markdown(card, cx)),
                )
                .into_any_element(),
            })
            .when(selected, |card| {
                card.child(
                    div().absolute().top(px(4.)).right(px(4.)).child(
                        Button::new(SharedString::from(format!(
                            "kanban-del-{}-{delete_id}",
                            cx.entity_id().as_u64()
                        )))
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

impl KanbanView {
    /// The card as the same Markdown a note uses, including wikilinks.
    fn card_markdown(&self, card: &Card, cx: &mut Context<Self>) -> impl IntoElement {
        let source = visible_line_breaks(&to_reading_markdown(&card.text).text);
        let from = self.path.clone();
        let root = Notia::vault(cx).read(cx).vault().root().to_path_buf();
        TextView::markdown(
            SharedString::from(format!("kanban-md-{}-{}", cx.entity_id().as_u64(), card.id)),
            source,
        )
        .selectable(false)
        .scrollable(false)
        .text_sm()
        .style(
            TextViewStyle::default()
                .paragraph_gap(rems(0.25))
                .heading_font_size(|level, _| px(13. + (6 - u32::from(level)).min(4) as f32)),
        )
        .image_source(move |uri| image_in_vault(root.clone(), uri))
        .on_link_click(move |url, event, window, cx| match parse_note_url(url) {
            Some(NoteUrl::Note(target)) => {
                let new_tab = event.modifiers().secondary(cx);
                workspace::open_link(&target, Some(from.clone()), new_tab, window, cx);
            }
            Some(NoteUrl::Tag(tag)) => {
                Notia::workspace(cx).update(cx, |model, cx| {
                    model.request_search(format!("tag:#{tag}").into(), cx);
                });
            }
            None => cx.open_url(url),
        })
    }
}

/// Seven lines of the card's text. The card uses the small size, so the cap
/// follows that size rather than the window's larger text.
fn card_max_height(window: &Window, lines: f32) -> Pixels {
    // Cards use the small text size, 0.875 of the window text.
    window.line_height() * 0.875 * lines
}

/// A Shift+Enter in a card is a real line. Markdown would otherwise draw a
/// single newline as a space, so those breaks become hard breaks. A blank
/// line, a list, a heading, or a code fence stays a block break.
fn visible_line_breaks(source: &str) -> String {
    let lines: Vec<&str> = source.split('\n').collect();
    let mut out = String::with_capacity(source.len());
    let mut fence: Option<(char, usize)> = None;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if let Some((marker, len)) = fence {
            let run = trimmed.chars().take_while(|&ch| ch == marker).count();
            if run >= len && trimmed[run..].trim().is_empty() {
                fence = None;
            }
        } else if let Some(open) = fence_open(trimmed) {
            fence = Some(open);
        }
        out.push_str(line);
        let Some(next) = lines.get(i + 1) else {
            break;
        };
        let block = fence.is_some()
            || line.is_empty()
            || next.is_empty()
            || block_line(next)
            || line.ends_with("  ")
            || line.ends_with('\\');
        if block {
            out.push('\n');
        } else {
            out.push_str("\\\n");
        }
    }
    out
}

fn fence_open(line: &str) -> Option<(char, usize)> {
    let marker = line.chars().next().filter(|ch| *ch == '`' || *ch == '~')?;
    let len = line.chars().take_while(|&ch| ch == marker).count();
    (len >= 3).then_some((marker, len))
}

fn block_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with('#')
        || trimmed.starts_with('>')
        || trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("+ ")
        || trimmed.starts_with("- [")
        || ordered_list(trimmed)
}

fn ordered_list(line: &str) -> bool {
    let Some((number, rest)) = line.split_once(['.', ')']) else {
        return false;
    };
    !number.is_empty()
        && number.chars().all(|ch| ch.is_ascii_digit())
        && rest.starts_with([' ', '\t'])
}

/// The card text to store. Blank lines at either end go; a leading indent
/// stays, because that is how a Markdown code block is written.
fn card_body(raw: &str) -> String {
    let text = raw.trim_matches(['\n', '\r']);
    if text.trim().is_empty() {
        String::new()
    } else {
        text.to_string()
    }
}

fn layout_slider(range: (f32, f32), step: f32, value: f32) -> SliderState {
    SliderState::new()
        .min(range.0)
        .max(range.1)
        .step(step)
        .default_value(value)
}

fn sync_slider(slider: &Entity<SliderState>, value: f32, window: &mut Window, cx: &mut App) {
    slider.update(cx, |state, cx| {
        if (state.value().start() - value).abs() > 0.01 {
            state.set_value(value, window, cx);
        }
    });
}

fn setting_row(
    label: &'static str,
    value: String,
    slider: &Entity<SliderState>,
    muted: Hsla,
) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .child(div().text_sm().child(label))
                .child(div().text_xs().text_color(muted).child(value)),
        )
        .child(Slider::new(slider).w_full())
}

/// The card body, at most seven lines tall. A longer card scrolls here.
fn card_scroll(id: &str, max_h: Pixels, child: impl IntoElement) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("kanban-card-scroll-{id}")))
        .w_full()
        .max_h(max_h)
        .overflow_y_scrollbar()
        .child(child)
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
    use super::{Board, Card, Layout, card_body, visible_line_breaks};

    #[test]
    fn a_board_without_layout_uses_the_usual_sizes() {
        let board = Board::parse(r#"{"columns":[{"id":"1","title":"To do","cards":[]}]}"#);
        assert_eq!(board.layout, Layout::default());
        let mut sized = Board::starter();
        sized.layout.column_width = 300.;
        sized.layout.card_lines = 5.;
        sized.layout.gap = 20.;
        let read = Board::parse(&sized.to_json());
        assert_eq!(read.layout.column_width(), 300.);
        assert_eq!(read.layout.card_lines(), 5.);
        assert_eq!(read.layout.gap(), 20.);
    }

    #[test]
    fn a_typed_line_break_stays_a_line_and_lists_stay_blocks() {
        assert_eq!(visible_line_breaks("one\ntwo"), "one\\\ntwo");
        assert_eq!(visible_line_breaks("one\n\ntwo"), "one\n\ntwo");
        assert_eq!(visible_line_breaks("- a\n- b"), "- a\n- b");
        assert_eq!(
            visible_line_breaks("```\n- not a list\n```"),
            "```\n- not a list\n```"
        );
    }

    #[test]
    fn card_text_keeps_markdown_lines() {
        assert_eq!(card_body("**bold**\n- item\n"), "**bold**\n- item");
        assert_eq!(card_body("    code"), "    code");
        assert_eq!(card_body("\n\n  \n"), "");
    }

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
