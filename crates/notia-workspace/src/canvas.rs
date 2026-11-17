//! A Canvas note: boxes joined by connectors on an endless board.
//!
//! The board is JSON in the shape Obsidian's Canvas plugin uses, so a board
//! survives a trip through either app. Groups, images, and file nodes stay
//! other apps' business; Notia reads the boxes and the connectors.

use std::{cell::Cell, rc::Rc, time::Duration};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use serde::{Deserialize, Serialize};

use notia_plugin::{AppSettings, Notia, OpenOptions, core::VaultPath, workspace};

/// The grid step, in board units.
const GRID: f32 = 24.;
const NODE_W: f32 = 190.;
const NODE_H: f32 = 80.;
const MIN_SCALE: f32 = 0.3;
const MAX_SCALE: f32 = 3.;
/// Screen radius of a side dot.
const HANDLE_RADIUS: f32 = 8.;
/// How close the pointer has to land on a side dot to pull a connector.
const HANDLE_HIT: f32 = 16.;
const UNDO_LIMIT: usize = 60;

/// The boxes and connectors of one canvas.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Board {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Node {
    pub id: SharedString,
    /// `text` for words typed on the board, `file` for a note in the vault.
    #[serde(rename = "type", default = "text_node")]
    pub kind: SharedString,
    pub x: f32,
    pub y: f32,
    #[serde(rename = "width")]
    pub w: f32,
    #[serde(rename = "height")]
    pub h: f32,
    pub text: String,
    /// Vault path of a note, when `kind` is `file`. Empty for a text box.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub id: SharedString,
    #[serde(rename = "fromNode")]
    pub from: SharedString,
    #[serde(rename = "toNode")]
    pub to: SharedString,
}

fn text_node() -> SharedString {
    "text".into()
}

fn is_note(node: &Node) -> bool {
    node.kind.as_ref() == "file"
}

/// Markdown notes whose title contains `query`, excluding this canvas.
fn matching_notes(cx: &App, canvas: &VaultPath, query: &str) -> Vec<(VaultPath, String)> {
    let query = query.trim().to_lowercase();
    let mut notes: Vec<(VaultPath, String)> = Notia::vault(cx)
        .read(cx)
        .index()
        .notes()
        .filter(|note| note.path.is_markdown() && !note.path.is_canvas() && &note.path != canvas)
        .map(|note| (note.path.clone(), note.title().to_string()))
        .filter(|(_, title)| query.is_empty() || title.to_lowercase().contains(&query))
        .collect();
    notes.sort_by(|left, right| left.1.to_lowercase().cmp(&right.1.to_lowercase()));
    notes.truncate(8);
    notes
}

/// What a box shows when it is not being typed in.
fn node_label(node: &Node) -> String {
    if is_note(node) {
        let path = VaultPath::new(&node.file);
        let name = path.file_stem();
        if name.is_empty() {
            "Note".to_string()
        } else {
            name.to_string()
        }
    } else if node.text.is_empty() {
        "Empty box".to_string()
    } else {
        node.text.clone()
    }
}

impl Board {
    fn parse(text: &str) -> Board {
        serde_json::from_str(text).unwrap_or_default()
    }

    fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// The next id: one above the highest number already in use.
    fn next_id(&self) -> SharedString {
        self.nodes
            .iter()
            .map(|node| node.id.as_ref())
            .chain(self.edges.iter().map(|edge| edge.id.as_ref()))
            .filter_map(|id| id.parse::<u64>().ok())
            .max()
            .map_or(1, |highest| highest + 1)
            .to_string()
            .into()
    }

    fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id.as_ref() == id)
    }

    /// The smallest rectangle holding every box, in board units.
    fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let first = self.nodes.first()?;
        let (mut x0, mut y0) = (first.x, first.y);
        let (mut x1, mut y1) = (first.x + first.w, first.y + first.h);
        for node in &self.nodes {
            x0 = x0.min(node.x);
            y0 = y0.min(node.y);
            x1 = x1.max(node.x + node.w);
            y1 = y1.max(node.y + node.h);
        }
        Some((x0, y0, x1, y1))
    }

    fn remove(&mut self, id: &str) {
        self.nodes.retain(|node| node.id.as_ref() != id);
        self.edges
            .retain(|edge| edge.from.as_ref() != id && edge.to.as_ref() != id);
    }

    fn connect(&mut self, from: &str, to: &str) {
        let linked = self.edges.iter().any(|edge| {
            (edge.from.as_ref(), edge.to.as_ref()) == (from, to)
                || (edge.from.as_ref(), edge.to.as_ref()) == (to, from)
        });
        if from == to || linked || self.node(from).is_none() || self.node(to).is_none() {
            return;
        }
        let id = self.next_id();
        self.edges.push(Edge {
            id,
            from: from.into(),
            to: to.into(),
        });
    }

    /// Adds a copy of `id` shifted aside, and returns its id.
    fn duplicate(&mut self, id: &str) -> Option<SharedString> {
        let mut node = self.node(id)?.clone();
        node.id = self.next_id();
        node.x += 24.;
        node.y += 24.;
        let copy = node.id.clone();
        self.nodes.push(node);
        Some(copy)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

const SIDES: [Side; 4] = [Side::Top, Side::Right, Side::Bottom, Side::Left];

/// Where a connector leaves a box.
fn anchor(node: &Node, side: Side) -> (f32, f32) {
    let (cx, cy) = (node.x + node.w / 2., node.y + node.h / 2.);
    match side {
        Side::Top => (cx, node.y),
        Side::Bottom => (cx, node.y + node.h),
        Side::Left => (node.x, cy),
        Side::Right => (node.x + node.w, cy),
    }
}

fn normal(side: Side) -> (f32, f32) {
    match side {
        Side::Top => (0., -1.),
        Side::Bottom => (0., 1.),
        Side::Left => (-1., 0.),
        Side::Right => (1., 0.),
    }
}

/// The sides two boxes join on, facing each other along the longer gap.
fn sides(from: &Node, to: &Node) -> (Side, Side) {
    let (dx, dy) = (
        to.x + to.w / 2. - (from.x + from.w / 2.),
        to.y + to.h / 2. - (from.y + from.h / 2.),
    );
    match (dx.abs() > dy.abs(), dx > 0., dy > 0.) {
        (true, true, _) => (Side::Right, Side::Left),
        (true, false, _) => (Side::Left, Side::Right),
        (false, _, true) => (Side::Bottom, Side::Top),
        (false, _, false) => (Side::Top, Side::Bottom),
    }
}

/// A connector in window pixels, with the way it leaves each end.
struct Segment {
    from: Point<Pixels>,
    from_normal: (f32, f32),
    to: Point<Pixels>,
    to_normal: (f32, f32),
}

#[derive(Clone)]
enum Drag {
    Pan {
        start: Point<Pixels>,
        pan: (f32, f32),
    },
    Node {
        id: SharedString,
        /// Where the pointer grabbed the box, in board units.
        grab: (f32, f32),
        /// Where the press started, in window pixels.
        origin: Point<Pixels>,
    },
}

/// A double-click on empty board, before a box exists.
struct Placer {
    x: f32,
    y: f32,
    /// The note list is open, rather than the text-or-note choice.
    notes: bool,
}

/// Tells the note the board was written, so the published text stays the file.
pub(crate) struct CanvasEvent;

/// One open canvas: its board, the part of it on screen, and the drag in
/// progress.
pub struct CanvasView {
    focus_handle: FocusHandle,
    path: VaultPath,
    board: Board,
    /// The board as last written, to tell saved work from unsaved work.
    saved: Board,
    undo: Vec<Board>,
    redo: Vec<Board>,
    /// Where the board's origin sits, in window pixels.
    pan: (f32, f32),
    scale: f32,
    selected: Option<SharedString>,
    hover: Option<SharedString>,
    /// The box being typed in, with the field showing its text.
    editing: Option<(SharedString, Entity<InputState>)>,
    edit_subscription: Option<Subscription>,
    drag: Option<Drag>,
    /// The board as it was when a drag began, kept so an unchanged drag adds
    /// no undo step.
    drag_start: Option<Board>,
    /// A connector being pulled: its box and the side it left.
    link: Option<(SharedString, Side)>,
    /// Where the pointer is, in window pixels.
    pointer: Point<Pixels>,
    /// The bounds of the layer holding the grid and the connectors. Its paint
    /// pass records them for the next frame, since a view cannot otherwise
    /// learn where it was placed.
    layer: Rc<Cell<Bounds<Pixels>>>,
    /// The zoom rail and the box toolbar. A press here belongs to those
    /// buttons, not to the board underneath.
    rail_bounds: Rc<Cell<Bounds<Pixels>>>,
    toolbar_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// A double-click on empty board, waiting for text box or note.
    placer: Option<Placer>,
    placer_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Filters the note list. Observing it redraws the list as the user types.
    note_query: Option<Entity<InputState>>,
    _note_query: Option<Subscription>,
    fitted: bool,
    error: Option<SharedString>,
    save_task: Option<Task<()>>,
}

impl EventEmitter<CanvasEvent> for CanvasView {}

impl CanvasView {
    pub fn new(path: VaultPath, text: &str, cx: &mut Context<Self>) -> Self {
        let board = Board::parse(text);
        Self {
            focus_handle: cx.focus_handle(),
            path,
            saved: board.clone(),
            board,
            undo: Vec::new(),
            redo: Vec::new(),
            pan: (0., 0.),
            scale: 1.,
            selected: None,
            hover: None,
            editing: None,
            edit_subscription: None,
            drag: None,
            drag_start: None,
            link: None,
            pointer: point(px(0.), px(0.)),
            layer: Rc::new(Cell::new(Bounds::default())),
            rail_bounds: Rc::new(Cell::new(Bounds::default())),
            toolbar_bounds: Rc::new(Cell::new(Bounds::default())),
            placer: None,
            placer_bounds: Rc::new(Cell::new(Bounds::default())),
            note_query: None,
            _note_query: None,
            fitted: false,
            error: None,
            save_task: None,
        }
    }

    fn board_at(&self, at: Point<Pixels>) -> (f32, f32) {
        (
            (at.x.as_f32() - self.pan.0) / self.scale,
            (at.y.as_f32() - self.pan.1) / self.scale,
        )
    }

    fn on_screen(&self, x: f32, y: f32) -> Point<Pixels> {
        point(
            px(x * self.scale + self.pan.0),
            px(y * self.scale + self.pan.1),
        )
    }

    /// A point in window pixels, moved into the view's own space.
    fn local(&self, at: Point<Pixels>) -> Point<Pixels> {
        at - self.layer.get().origin
    }

    fn holds(&self, node: &Node, at: Point<Pixels>) -> bool {
        let (x, y) = self.board_at(at);
        node.x <= x && x <= node.x + node.w && node.y <= y && y <= node.y + node.h
    }

    fn node_at(&self, at: Point<Pixels>) -> Option<SharedString> {
        self.board
            .nodes
            .iter()
            .rev()
            .find(|node| self.holds(node, at))
            .map(|node| node.id.clone())
    }

    /// The box a connector should join when released at `at`. The side dot
    /// sits on the border and sticks out, so a release just outside still counts.
    fn drop_at(&self, at: Point<Pixels>, from: &str) -> Option<SharedString> {
        if let Some(id) = self.node_at(at)
            && id.as_ref() != from
        {
            return Some(id);
        }
        let (x, y) = self.board_at(at);
        let pad = HANDLE_HIT / self.scale.max(0.01);
        self.board
            .nodes
            .iter()
            .rev()
            .filter(|node| node.id.as_ref() != from)
            .find(|node| {
                node.x - pad <= x
                    && x <= node.x + node.w + pad
                    && node.y - pad <= y
                    && y <= node.y + node.h + pad
            })
            .map(|node| node.id.clone())
    }

    /// The side dot of the hovered or selected box under the pointer.
    fn handle_at(&self, at: Point<Pixels>) -> Option<(SharedString, Side)> {
        let id = self.hover.as_ref().or(self.selected.as_ref())?.clone();
        let node = self.board.node(&id)?;
        SIDES
            .into_iter()
            .map(|side| {
                let (x, y) = anchor(node, side);
                (side, self.on_screen(x, y))
            })
            .find(|(_, point)| {
                let dx = point.x.as_f32() - at.x.as_f32();
                let dy = point.y.as_f32() - at.y.as_f32();
                dx * dx + dy * dy <= HANDLE_HIT * HANDLE_HIT
            })
            .map(|(side, _)| (id, side))
    }

    /// Every connector, plus the one being pulled.
    fn segments(&self) -> Vec<Segment> {
        let mut segments = Vec::new();
        for edge in &self.board.edges {
            let (from, to) = match (self.board.node(&edge.from), self.board.node(&edge.to)) {
                (Some(from), Some(to)) => (from, to),
                _ => continue,
            };
            let (a, b) = sides(from, to);
            let (fx, fy) = anchor(from, a);
            let (tx, ty) = anchor(to, b);
            segments.push(Segment {
                from: self.on_screen(fx, fy),
                from_normal: normal(a),
                to: self.on_screen(tx, ty),
                to_normal: normal(b),
            });
        }
        if let Some((id, side)) = &self.link
            && let Some(node) = self.board.node(id)
        {
            let (x, y) = anchor(node, *side);
            // Snap onto the box under the pointer, so a release there joins it.
            let (to, to_normal) = if let Some(target_id) = self.drop_at(self.pointer, id)
                && let Some(target) = self.board.node(&target_id)
            {
                let (_, end) = sides(node, target);
                let (tx, ty) = anchor(target, end);
                (self.on_screen(tx, ty), normal(end))
            } else {
                (self.pointer, (0., 0.))
            };
            segments.push(Segment {
                from: self.on_screen(x, y),
                from_normal: normal(*side),
                to,
                to_normal,
            });
        }
        segments
    }

    /// The dots to pull a connector from, in window pixels.
    fn handles(&self) -> Vec<Point<Pixels>> {
        let id = match self.hover.as_ref().or(self.selected.as_ref()) {
            Some(id) => id,
            None => return Vec::new(),
        };
        let Some(node) = self.board.node(id) else {
            return Vec::new();
        };
        SIDES
            .into_iter()
            .map(|side| {
                let (x, y) = anchor(node, side);
                self.on_screen(x, y)
            })
            .collect()
    }

    /// One undoable change.
    fn change(&mut self, edit: impl FnOnce(&mut Board), cx: &mut Context<Self>) {
        self.push_undo();
        self.redo.clear();
        edit(&mut self.board);
        self.mark_edited(cx);
    }

    fn push_undo(&mut self) {
        let board = self.board.clone();
        self.push_state(board);
    }

    fn push_state(&mut self, board: Board) {
        self.undo.push(board);
        while self.undo.len() > UNDO_LIMIT {
            self.undo.remove(0);
        }
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        if let Some(previous) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.board, previous));
            self.after_jump(cx);
        }
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.board, next));
            self.after_jump(cx);
        }
    }

    fn after_jump(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.editing = None;
        self.mark_edited(cx);
    }

    fn add_node(&mut self, x: f32, y: f32, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.board.next_id();
        let new = id.clone();
        self.change(
            move |board| {
                board.nodes.push(Node {
                    id,
                    kind: text_node(),
                    x: x - NODE_W / 2.,
                    y: y - NODE_H / 2.,
                    w: NODE_W,
                    h: NODE_H,
                    text: String::new(),
                    file: String::new(),
                });
            },
            cx,
        );
        self.selected = Some(new.clone());
        self.close_placer();
        self.begin_edit(&new, window, cx);
    }

    fn add_note(&mut self, x: f32, y: f32, path: VaultPath, cx: &mut Context<Self>) {
        let id = self.board.next_id();
        let new = id.clone();
        let file = path.as_str().to_string();
        self.change(
            move |board| {
                board.nodes.push(Node {
                    id,
                    kind: "file".into(),
                    x: x - NODE_W / 2.,
                    y: y - NODE_H / 2.,
                    w: NODE_W,
                    h: NODE_H,
                    text: String::new(),
                    file,
                });
            },
            cx,
        );
        self.selected = Some(new);
        self.close_placer();
    }

    fn open_placer(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        self.close_placer();
        self.placer = Some(Placer { x, y, notes: false });
        cx.notify();
    }

    fn close_placer(&mut self) {
        self.placer = None;
        self.note_query = None;
        self._note_query = None;
        self.placer_bounds.set(Bounds::default());
    }

    fn show_note_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(place) = self.placer.as_mut() else {
            return;
        };
        place.notes = true;
        let field = cx.new(|cx| InputState::new(window, cx).placeholder("Find a note"));
        self._note_query = Some(cx.observe(&field, |_, _, cx| cx.notify()));
        self.note_query = Some(field.clone());
        field.update(cx, |field, cx| field.focus(window, cx));
        cx.notify();
    }

    fn delete_selected(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        self.selected = None;
        self.editing = None;
        self.change(move |board| board.remove(&id), cx);
    }

    fn duplicate_selected(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        if let Some(copy) = self.board.duplicate(&id) {
            self.push_undo();
            self.redo.clear();
            self.selected = Some(copy);
            self.mark_edited(cx);
        }
    }

    fn connect(&mut self, from: SharedString, to: SharedString, cx: &mut Context<Self>) {
        self.change(move |board| board.connect(&from, &to), cx);
    }

    /// The board as it would be written.
    pub(crate) fn document(&self) -> SharedString {
        self.board.to_json().into()
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
    pub fn save(&mut self, cx: &mut Context<Self>) {
        self.save_task = None;
        self.flush_edit(cx);
        let json = self.board.to_json();
        let path = self.path.clone();
        if self.board == self.saved
            && Notia::vault(cx)
                .read(cx)
                .read_note(&path)
                .is_ok_and(|disk| disk == json)
        {
            return;
        }
        match Notia::vault(cx).update(cx, |vault, cx| vault.save_note(&path, &json, cx)) {
            Ok(()) => {
                self.saved = self.board.clone();
                self.error = None;
                cx.emit(CanvasEvent);
            }
            Err(error) => {
                self.error = Some(format!("Could not save this canvas: {error}").into());
            }
        }
        cx.notify();
    }

    /// Copies text still sitting in the field onto its box.
    fn flush_edit(&mut self, cx: &mut Context<Self>) {
        let Some((id, field)) = self.editing.clone() else {
            return;
        };
        let text = field.read(cx).value().trim().to_string();
        if self.board.node(&id).is_some_and(|node| node.text == text) {
            return;
        }
        self.change(
            move |board| {
                if let Some(node) = board.nodes.iter_mut().find(|node| node.id == id) {
                    node.text = text;
                }
            },
            cx,
        );
    }

    fn mark_edited(&mut self, cx: &mut Context<Self>) {
        let delay = Duration::from_millis(AppSettings::global(cx).editor.autosave_delay_ms);
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            _ = this.update(cx, |this, cx| this.save(cx));
        }));
        cx.notify();
    }

    fn zoom_at(&mut self, at: Point<Pixels>, factor: f32) {
        let scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        let ratio = scale / self.scale;
        self.pan = (
            at.x.as_f32() - (at.x.as_f32() - self.pan.0) * ratio,
            at.y.as_f32() - (at.y.as_f32() - self.pan.1) * ratio,
        );
        self.scale = scale;
    }

    fn center(&self) -> Point<Pixels> {
        let layer = self.layer.get();
        point(
            px(layer.origin.x.as_f32() + layer.size.width.as_f32() / 2.),
            px(layer.origin.y.as_f32() + layer.size.height.as_f32() / 2.),
        )
    }

    /// Shows the whole board, centred.
    fn fit(&mut self) {
        let layer = self.layer.get();
        let pad = 40.;
        let (width, height) = (
            (layer.size.width.as_f32() - pad * 2.).max(1.),
            (layer.size.height.as_f32() - pad * 2.).max(1.),
        );
        let Some((x0, y0, x1, y1)) = self.board.bounds() else {
            self.scale = 1.;
            self.pan = (layer.origin.x.as_f32(), layer.origin.y.as_f32());
            return;
        };
        let (bw, bh) = ((x1 - x0).max(1.), (y1 - y0).max(1.));
        let scale = (width / bw).min(height / bh).clamp(MIN_SCALE, MAX_SCALE);
        self.scale = scale;
        self.pan = (
            layer.origin.x.as_f32() + (width - bw * scale) / 2. - x0 * scale,
            layer.origin.y.as_f32() + (height - bh * scale) / 2. - y0 * scale,
        );
    }

    fn begin_edit(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(node) = self.board.node(id) else {
            return;
        };
        let (id, text) = (node.id.clone(), node.text.clone());
        let field = cx.new(|cx| InputState::new(window, cx).default_value(text));
        self.edit_subscription =
            Some(
                cx.subscribe_in(&field, window, |this, field, event, _, cx| match event {
                    InputEvent::PressEnter { .. } | InputEvent::Blur => this.commit_edit(field, cx),
                    _ => {}
                }),
            );
        self.editing = Some((id, field.clone()));
        field.update(cx, |field, cx| {
            field.select_all(window, cx);
            field.focus(window, cx);
        });
    }

    /// Moves the field's text into its box, once. A dropped field blurs, so
    /// the same commit runs again; the entity check stops it.
    fn commit_edit(&mut self, field: &Entity<InputState>, cx: &mut Context<Self>) {
        let Some((id, current)) = self.editing.take() else {
            return;
        };
        self.edit_subscription = None;
        if current != *field {
            return;
        }
        let text = field.read(cx).value().trim().to_string();
        if self.board.node(&id).is_some_and(|node| node.text == text) {
            return;
        }
        self.change(
            move |board| {
                if let Some(node) = board.nodes.iter_mut().find(|node| node.id == id) {
                    node.text = text;
                }
            },
            cx,
        );
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pointer = event.position;
        if hits_controls(self, event.position) {
            return;
        }
        let inside = self.node_at(event.position);
        // The dot overlaps the box edge. A press on it starts a connector
        // even when that point is also inside the box.
        if let Some(link) = self.handle_at(event.position) {
            self.link = Some(link);
            cx.notify();
            return;
        }
        match inside {
            Some(id) => {
                self.selected = Some(id.clone());
                if event.click_count > 1 && !self.board.node(&id).is_some_and(is_note) {
                    self.begin_edit(&id, window, cx);
                    // The board is also focusable. Leave the text field focused.
                    window.prevent_default();
                } else if let Some((nx, ny)) = self.board.node(&id).map(|node| (node.x, node.y)) {
                    // A drag starts from a single click, which ends typing.
                    // A note that is released without moving opens that note.
                    self.focus_handle.focus(window, cx);
                    let (x, y) = self.board_at(event.position);
                    self.drag_start = Some(self.board.clone());
                    self.drag = Some(Drag::Node {
                        id,
                        grab: (x - nx, y - ny),
                        origin: event.position,
                    });
                }
            }
            None => {
                self.selected = None;
                if event.click_count > 1 {
                    let (x, y) = self.board_at(event.position);
                    self.open_placer(x, y, cx);
                } else {
                    self.close_placer();
                    self.drag = Some(Drag::Pan {
                        start: event.position,
                        pan: self.pan,
                    });
                }
            }
        }
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.pointer = event.position;
        match self.drag.clone() {
            Some(Drag::Pan { start, pan }) => {
                self.pan = (
                    pan.0 + event.position.x.as_f32() - start.x.as_f32(),
                    pan.1 + event.position.y.as_f32() - start.y.as_f32(),
                );
            }
            Some(Drag::Node { id, grab, origin }) => {
                let (dx, dy) = (
                    event.position.x.as_f32() - origin.x.as_f32(),
                    event.position.y.as_f32() - origin.y.as_f32(),
                );
                // A small slip is still a click, so a note can be opened.
                if dx.hypot(dy) < 4. {
                    return;
                }
                let (x, y) = self.board_at(event.position);
                if let Some(node) = self.board.nodes.iter_mut().find(|node| node.id == id) {
                    node.x = x - grab.0;
                    node.y = y - grab.1;
                }
                self.mark_edited(cx);
            }
            None => {
                self.hover = self.node_at(event.position);
            }
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.pointer = event.position;
        let link = self.link.take();
        let open = self.note_click(event.position);
        self.drag = None;
        // A drag that moved a box is one undo step; one that only selected
        // it is not.
        if let Some(start) = self.drag_start.take()
            && start != self.board
        {
            self.push_state(start);
            self.redo.clear();
        }
        if let Some((from, _)) = link
            && let Some(to) = self.drop_at(event.position, &from)
        {
            self.connect(from, to, cx);
        }
        if let Some(path) = open {
            workspace::open_note(path, OpenOptions::new(), window, cx);
        }
        cx.notify();
    }

    /// A note box that was pressed and released without being dragged.
    fn note_click(&self, at: Point<Pixels>) -> Option<VaultPath> {
        let Drag::Node { id, origin, .. } = self.drag.as_ref()? else {
            return None;
        };
        let (dx, dy) = (
            at.x.as_f32() - origin.x.as_f32(),
            at.y.as_f32() - origin.y.as_f32(),
        );
        if dx.hypot(dy) >= 4. {
            return None;
        }
        let node = self.board.node(id)?;
        if !is_note(node) || node.file.is_empty() {
            return None;
        }
        Some(VaultPath::new(&node.file))
    }

    fn on_scroll(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.pointer = event.position;
        let (dx, dy) = match event.delta {
            ScrollDelta::Pixels(delta) => (delta.x.as_f32(), delta.y.as_f32()),
            ScrollDelta::Lines(delta) => (delta.x * 30., delta.y * 30.),
        };
        // A trackpad pinch is a separate gesture. Two-finger movement pans the
        // board; holding Command or Control zooms toward the pointer.
        if event.modifiers.platform || event.modifiers.control {
            self.zoom_at(event.position, (-dy * 0.002).exp());
        } else {
            self.pan = (self.pan.0 - dx, self.pan.1 - dy);
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Trackpad pinch. `delta` is a fraction, so 0.1 grows the board by a tenth.
    fn on_pinch(&mut self, event: &PinchEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.delta == 0. {
            return;
        }
        self.zoom_at(event.position, (1. + event.delta).clamp(0.5, 2.));
        cx.stop_propagation();
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        let key = event.keystroke.key.as_str();
        let modifiers = &event.keystroke.modifiers;
        match key {
            "backspace" | "delete" => self.delete_selected(cx),
            "escape" => {
                self.selected = None;
                self.link = None;
            }
            "z" if modifiers.platform && modifiers.shift => self.redo(cx),
            "z" if modifiers.platform => self.undo(cx),
            "y" if modifiers.control => self.undo(cx),
            _ => return,
        }
        cx.notify();
    }
}

impl Focusable for CanvasView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for CanvasView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The layer's bounds only arrive with the first paint, so the board
        // is centred on the second frame.
        let layer = self.layer.get();
        if !self.fitted && layer.size.width > px(0.) {
            self.fit();
            self.fitted = true;
        }

        let (accent, border, surface, muted, danger) = (
            cx.theme().accent,
            cx.theme().border,
            cx.theme().background,
            cx.theme().muted_foreground,
            cx.theme().danger,
        );
        let (scale, view, empty) = (self.scale, self.pan, self.board.nodes.is_empty());
        let (segments, handles, layer_bounds) =
            (self.segments(), self.handles(), self.layer.clone());
        let (boxes, toolbar, rail) = (self.boxes(cx), self.toolbar(cx), self.rail(cx));
        let placer = self.placer_menu(cx);
        if toolbar.is_none() {
            self.toolbar_bounds.set(Bounds::default());
        }
        if placer.is_none() {
            self.placer_bounds.set(Bounds::default());
        }
        let error = self.error.clone();
        // While a drag runs, the pointer is followed at the window level, so
        // it keeps moving when it leaves the board.
        let dragging = self.drag.is_some() || self.link.is_some();
        let weak = cx.weak_entity();

        div()
            .id("canvas")
            .key_context("Canvas")
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(surface)
            .track_focus(&self.focus_handle)
            // Capture runs before a box's text field, which would otherwise
            // take the press and leave the box unable to move.
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, window, cx| {
                if event.button != MouseButton::Left || hits_controls(this, event.position) {
                    return;
                }
                let edit = event.click_count > 1 && this.node_at(event.position).is_some();
                this.on_mouse_down(event, window, cx);
                if !edit {
                    cx.stop_propagation();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll(cx.listener(Self::on_scroll))
            .on_pinch(cx.listener(Self::on_pinch))
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                canvas(
                    move |bounds, _, _| layer_bounds.set(bounds),
                    move |bounds, _, window, _| {
                        paint_grid(window, bounds, view, scale, muted);
                        for segment in &segments {
                            paint_segment(window, segment, border);
                        }
                    },
                )
                .absolute()
                .inset_0(),
            )
            .children(boxes)
            .child(
                canvas(
                    move |_, _, _| {},
                    move |_, _, window, _| {
                        for at in &handles {
                            paint_handle(window, *at, accent, surface);
                        }
                        if !dragging {
                            return;
                        }
                        let weak_move = weak.clone();
                        window.on_mouse_event::<MouseMoveEvent>(move |event, phase, window, cx| {
                            if phase == DispatchPhase::Capture {
                                _ = weak_move
                                    .update(cx, |board, cx| board.on_mouse_move(event, window, cx));
                            }
                        });
                        let weak_up = weak.clone();
                        window.on_mouse_event::<MouseUpEvent>(move |event, phase, window, cx| {
                            if phase == DispatchPhase::Capture {
                                _ = weak_up
                                    .update(cx, |board, cx| board.on_mouse_up(event, window, cx));
                            }
                        });
                    },
                )
                .absolute()
                .inset_0(),
            )
            .children(toolbar)
            .child(rail)
            .children(placer)
            .when(empty, |this| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child("Double-click to add a text box or a note"),
                        ),
                )
            })
            .when_some(error, |this, error| {
                this.child(
                    h_flex()
                        .absolute()
                        .top_2()
                        .left_2()
                        .gap_2()
                        .text_sm()
                        .text_color(danger)
                        .child(Icon::new(IconName::TriangleAlert).small())
                        .child(error),
                )
            })
    }
}

impl CanvasView {
    /// The boxes, as views the text of a box can live in.
    fn boxes(&self, cx: &mut Context<Self>) -> Vec<Div> {
        let theme = cx.theme();
        let (accent, border, surface, foreground, muted, link) = (
            theme.accent,
            theme.border,
            theme.background,
            theme.foreground,
            theme.muted_foreground,
            theme.link,
        );
        let scale = self.scale;
        let (font, corner) = (px(14. * scale), px(8. * scale));
        let editing = self
            .editing
            .as_ref()
            .map(|(id, field)| (id.clone(), field.clone()));
        self.board
            .nodes
            .iter()
            .map(|node| {
                let at = self.local(self.on_screen(node.x, node.y));
                let selected = self.selected.as_ref().is_some_and(|id| *id == node.id);
                let field = editing
                    .as_ref()
                    .filter(|(id, _)| *id == node.id)
                    .map(|(_, field)| field.clone());
                let note = is_note(node);
                let (text, empty) = (node_label(node), node.text.is_empty() && !note);
                let inner = match field {
                    Some(field) => v_flex()
                        .size_full()
                        .justify_center()
                        .px(px(8. * scale))
                        .child(
                            Input::new(&field)
                                .appearance(false)
                                .text_size(font)
                                .text_center()
                                .w_full(),
                        ),
                    None => v_flex()
                        .size_full()
                        .justify_center()
                        .items_center()
                        .px(px(8. * scale))
                        .text_size(font)
                        .text_center()
                        .overflow_hidden()
                        .text_color(if note {
                            link
                        } else if empty {
                            muted
                        } else {
                            foreground
                        })
                        .child(text),
                };
                div()
                    .absolute()
                    .left(px(at.x.as_f32()))
                    .top(px(at.y.as_f32()))
                    .w(px(node.w * scale))
                    .h(px(node.h * scale))
                    .rounded(corner)
                    .border_2()
                    .border_color(if selected { accent } else { border })
                    .bg(surface)
                    .overflow_hidden()
                    .child(inner)
            })
            .collect()
    }

    /// The buttons over the selected box.
    fn toolbar(&self, cx: &mut Context<Self>) -> Option<Div> {
        let (panel, border, radius, danger) = (
            cx.theme().sidebar,
            cx.theme().border,
            cx.theme().radius,
            cx.theme().danger,
        );
        let scale = self.scale;
        let node = self.selected.as_ref().and_then(|id| self.board.node(id))?;
        let delete_id = node.id.clone();
        let at = self.local(self.on_screen(node.x, node.y));
        let left = at.x.as_f32() + node.w * scale / 2. - 48.;
        Some(
            h_flex()
                .absolute()
                .left(px(left.max(0.)))
                .top(px(at.y.as_f32() - 34.))
                .gap_1()
                .p_1()
                .rounded(radius)
                .bg(panel)
                .border_1()
                .border_color(border)
                .child(bounds_probe(self.toolbar_bounds.clone()))
                .when(!is_note(node), |bar| {
                    bar.child(
                        Button::new("canvas-rename")
                            .icon(Icon::new(Lucide::Pencil))
                            .ghost()
                            .xsmall()
                            .tooltip("Rename")
                            .on_click(cx.listener(|this, _, window, cx| {
                                if let Some(id) = this.selected.clone() {
                                    this.begin_edit(&id, window, cx);
                                }
                                cx.notify();
                            })),
                    )
                })
                .child(
                    Button::new("canvas-duplicate")
                        .icon(Icon::new(Lucide::CopyPlus))
                        .ghost()
                        .xsmall()
                        .tooltip("Duplicate")
                        .on_click(cx.listener(|this, _, _, cx| this.duplicate_selected(cx))),
                )
                .child(
                    Button::new("canvas-delete")
                        .icon(Icon::new(Lucide::Trash))
                        .ghost()
                        .xsmall()
                        .text_color(danger)
                        .tooltip("Delete")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.selected = Some(delete_id.clone());
                            this.delete_selected(cx);
                        })),
                ),
        )
    }

    /// The choice shown where the board was double-clicked.
    fn placer_menu(&self, cx: &mut Context<Self>) -> Option<Div> {
        let place = self.placer.as_ref()?;
        let (x, y) = (place.x, place.y);
        let notes = place.notes;
        let at = self.local(self.on_screen(x, y));
        let theme = cx.theme();
        let (panel, border, radius, muted) = (
            theme.sidebar,
            theme.border,
            theme.radius,
            theme.muted_foreground,
        );
        let query = self
            .note_query
            .as_ref()
            .map(|field| field.read(cx).value().to_string())
            .unwrap_or_default();
        let matches = matching_notes(cx, &self.path, &query);
        let menu = v_flex()
            .absolute()
            .left(px(at.x.as_f32().max(8.)))
            .top(px(at.y.as_f32().max(8.)))
            .w(px(220.))
            .p_1()
            .gap_1()
            .rounded(radius)
            .bg(panel)
            .border_1()
            .border_color(border)
            .child(bounds_probe(self.placer_bounds.clone()));
        let menu = if notes {
            let field = self.note_query.clone()?;
            menu.child(Input::new(&field).appearance(true).small().w_full())
                .children(if matches.is_empty() {
                    vec![
                        div()
                            .px_2()
                            .py_1()
                            .text_xs()
                            .text_color(muted)
                            .child("No matching notes")
                            .into_any_element(),
                    ]
                } else {
                    matches
                        .into_iter()
                        .map(|(path, title)| {
                            let open = path.clone();
                            Button::new(SharedString::from(format!("canvas-note-{path}")))
                                .ghost()
                                .small()
                                .label(title)
                                .w_full()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.add_note(x, y, open.clone(), cx);
                                }))
                                .into_any_element()
                        })
                        .collect()
                })
        } else {
            menu.child(
                Button::new("canvas-place-text")
                    .ghost()
                    .small()
                    .icon(Icon::new(Lucide::Type))
                    .label("Text box")
                    .w_full()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.add_node(x, y, window, cx);
                    })),
            )
            .child(
                Button::new("canvas-place-note")
                    .ghost()
                    .small()
                    .icon(Icon::new(Lucide::FileText))
                    .label("Note")
                    .w_full()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.show_note_list(window, cx);
                    })),
            )
        };
        Some(menu)
    }

    /// The zoom, undo, and redo buttons along the right edge.
    fn rail(&self, cx: &mut Context<Self>) -> Div {
        let (panel, border, radius) = (cx.theme().sidebar, cx.theme().border, cx.theme().radius);
        v_flex()
            .absolute()
            .top_2()
            .right_2()
            .gap_1()
            .p_1()
            .rounded(radius)
            .bg(panel)
            .border_1()
            .border_color(border)
            .child(bounds_probe(self.rail_bounds.clone()))
            .child(rail_button(
                cx,
                "canvas-zoom-in",
                Lucide::Plus,
                "Zoom in",
                |this, _, _, cx| {
                    let at = this.center();
                    this.zoom_at(at, 1.25);
                    cx.notify();
                },
            ))
            .child(rail_button(
                cx,
                "canvas-zoom-out",
                Lucide::Minus,
                "Zoom out",
                |this, _, _, cx| {
                    let at = this.center();
                    this.zoom_at(at, 0.8);
                    cx.notify();
                },
            ))
            .child(rail_button(
                cx,
                "canvas-fit",
                Lucide::Maximize,
                "Fit the board",
                |this, _, _, cx| {
                    this.fit();
                    cx.notify();
                },
            ))
            .child(rail_button(
                cx,
                "canvas-undo",
                Lucide::Undo2,
                "Undo",
                |this, _, _, cx| {
                    this.undo(cx);
                    cx.notify();
                },
            ))
            .child(rail_button(
                cx,
                "canvas-redo",
                Lucide::Redo2,
                "Redo",
                |this, _, _, cx| {
                    this.redo(cx);
                    cx.notify();
                },
            ))
    }
}

/// A button of the board's tool rail.
fn hits_controls(view: &CanvasView, at: Point<Pixels>) -> bool {
    covers(view.rail_bounds.get(), at)
        || covers(view.toolbar_bounds.get(), at)
        || covers(view.placer_bounds.get(), at)
}

fn covers(bounds: Bounds<Pixels>, at: Point<Pixels>) -> bool {
    bounds.size.width > px(0.) && bounds.size.height > px(0.) && bounds.contains(&at)
}

/// Records where a floating control was laid out, without taking part in hit testing.
fn bounds_probe(store: Rc<Cell<Bounds<Pixels>>>) -> impl IntoElement {
    canvas(move |bounds, _, _| store.set(bounds), |_, _, _, _| {})
        .absolute()
        .inset_0()
}

fn rail_button(
    cx: &mut Context<CanvasView>,
    name: &'static str,
    icon: Lucide,
    tooltip: &'static str,
    handler: impl Fn(&mut CanvasView, &ClickEvent, &mut Window, &mut Context<CanvasView>) + 'static,
) -> Button {
    Button::new(name)
        .icon(Icon::new(icon))
        .ghost()
        .small()
        .tooltip(tooltip)
        .on_click(cx.listener(handler))
}

/// The grid of dots. Below a readable step the grid doubles, so a zoomed-out
/// board never paints a dot for every pixel.
fn paint_grid(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    pan: (f32, f32),
    scale: f32,
    color: Hsla,
) {
    let mut step = GRID * scale;
    while step < 24. {
        step *= 2.;
    }
    let dot = px(1.5);
    let (left, right) = (
        bounds.origin.x.as_f32(),
        bounds.origin.x.as_f32() + bounds.size.width.as_f32(),
    );
    let (top, bottom) = (
        bounds.origin.y.as_f32(),
        bounds.origin.y.as_f32() + bounds.size.height.as_f32(),
    );
    let mut y = top + (pan.1 - top).rem_euclid(step);
    while y <= bottom {
        let mut x = left + (pan.0 - left).rem_euclid(step);
        while x <= right {
            window.paint_quad(fill(
                Bounds {
                    origin: point(px(x), px(y)),
                    size: size(dot, dot),
                },
                color,
            ));
            x += step;
        }
        y += step;
    }
}

/// A connector: two curves that leave their boxes squarely and meet in the
/// middle.
fn paint_segment(window: &mut Window, segment: &Segment, color: Hsla) {
    let (fx, fy) = (segment.from.x.as_f32(), segment.from.y.as_f32());
    let (tx, ty) = (segment.to.x.as_f32(), segment.to.y.as_f32());
    let bend = ((tx - fx).hypot(ty - fy) * 0.35).max(16.);
    let start = |normal: (f32, f32), x: f32, y: f32| {
        point(px(x + normal.0 * bend), px(y + normal.1 * bend))
    };
    let mut path = PathBuilder::stroke(px(1.5));
    path.move_to(segment.from);
    path.curve_to(
        point(px((fx + tx) / 2.), px((fy + ty) / 2.)),
        start(segment.from_normal, fx, fy),
    );
    path.curve_to(segment.to, start(segment.to_normal, tx, ty));
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

/// The dot on a box's side that a connector is pulled from.
fn paint_handle(window: &mut Window, at: Point<Pixels>, ring: Hsla, inside: Hsla) {
    let radius = px(HANDLE_RADIUS);
    let bounds = Bounds {
        origin: point(
            px(at.x.as_f32() - radius.as_f32()),
            px(at.y.as_f32() - radius.as_f32()),
        ),
        size: size(radius * 2., radius * 2.),
    };
    window.paint_quad(
        fill(bounds, inside)
            .corner_radii(radius.as_f32())
            .border_widths(px(1.5).as_f32())
            .border_color(ring),
    );
}

#[cfg(test)]
mod tests {
    // Named imports: globbing this module would bring in GPUI's own `test`
    // macro in place of Rust's.
    use super::{Board, Node};
    use gpui_kit::SharedString;

    #[test]
    fn boards_read_round_trip_and_hold_to_boxes_and_links() {
        let mut board = Board::default();
        let a: SharedString = "1".into();
        let b: SharedString = "2".into();
        for (id, x) in [(a.clone(), 0.), (b.clone(), 300.)] {
            board.nodes.push(Node {
                id,
                x,
                ..Node::default()
            });
        }
        board.connect(&a, &b);
        board.connect(&a, &b);
        board.connect(&a, &a);
        assert_eq!(board.edges.len(), 1);
        assert_eq!(Board::parse(&board.to_json()), board);

        // A board written by Obsidian's Canvas plugin reads in with its ids.
        let theirs = Board::parse(
            r#"{"nodes":[{"id":"a1","type":"text","x":10,"y":20,"width":120,"height":60,"text":"hi"}],
                 "edges":[{"id":"e1","fromNode":"a1","toNode":"b2","fromSide":"right","toSide":"left"}],
                 "unlisted":"kept elsewhere"}"#,
        );
        assert_eq!(theirs.nodes[0].w, 120.);
        assert_eq!(theirs.nodes[0].text, "hi");
        assert_eq!(theirs.edges[0].to, "b2");

        board.remove(&a);
        assert!(board.node(&a).is_none());
        assert!(board.edges.is_empty());
        let copy = board.duplicate(&b).unwrap();
        assert_ne!(copy, b);
        // The copy takes the next free id, so nothing collides.
        assert!(board.nodes.iter().all(|node| node.id != a));
    }
}
