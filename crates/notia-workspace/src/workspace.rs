use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    time::Duration,
};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, ThemeMode, TitleBar, WindowExt as _,
    button::{Button, ButtonVariants as _},
    command::CommandState,
    h_flex,
    kbd::Kbd,
    menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem},
    resizable::{HandleEdge, resize_handle},
    status_bar::StatusBar,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use notia_plugin::{
    ActiveNote, AppSettings, EditorCommand, EditorId, Notia, OpenOptions, Panel, PanelPlacement,
    PluginHost, RibbonItem, VaultEvent, VaultModel, ViewMode, WorkspaceModel,
    actions::{NewNote, OpenQuickSwitcher, OpenSettings, WORKSPACE_CONTEXT},
    core::VaultPath,
    note_tooltip,
    theme::{ColorScheme, ThemeCatalog},
};
use serde::{Deserialize, Serialize};

use crate::note_editor::{NoteEditor, NoteEditorEvent};

#[path = "phone.rs"]
mod phone;

#[path = "nav.rs"]
mod nav;

use phone::PhoneScreen;

/// The strip of plugin buttons along the left edge.
const RIBBON_WIDTH: Pixels = px(44.);
/// Room for the macOS window buttons drawn over the title row.
const TRAFFIC_LIGHTS_WIDTH: Pixels = px(76.);
const SIDEBAR_WIDTHS: (Pixels, Pixels) = (px(180.), px(560.));

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct SidebarLayout {
    open: bool,
    width: f32,
    active: Option<String>,
    #[serde(default)]
    order: Vec<String>,
    #[serde(default)]
    hidden: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Layout {
    left: SidebarLayout,
    right: SidebarLayout,
    #[serde(default)]
    ribbon: nav::RibbonLayout,
    tabs: Vec<VaultPath>,
    active_tab: Option<usize>,
    /// A second group of tabs beside or below the first. Absent when the
    /// editor is a single pane.
    #[serde(default)]
    split: Option<SavedSplit>,
    recent: Vec<VaultPath>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SplitAxis {
    Right,
    Down,
}

#[derive(Serialize, Deserialize)]
struct SavedSplit {
    axis: SplitAxis,
    ratio: f32,
    tabs: Vec<VaultPath>,
    active_tab: Option<usize>,
}

impl Default for SidebarLayout {
    fn default() -> Self {
        Self {
            open: true,
            width: 260.,
            active: None,
            order: Vec::new(),
            hidden: Vec::new(),
        }
    }
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            left: SidebarLayout::default(),
            right: SidebarLayout {
                width: 280.,
                ..SidebarLayout::default()
            },
            ribbon: nav::RibbonLayout::default(),
            tabs: Vec::new(),
            active_tab: None,
            split: None,
            recent: Vec::new(),
        }
    }
}

/// Which group of tabs a note lives in. `Main` is the original pane. A
/// horizontal split keeps both rows in the title bar; a vertical split keeps
/// the second row on the lower pane.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EditorPane {
    Main,
    Split,
}

struct SplitGroup {
    axis: SplitAxis,
    /// Share of the editor area given to the main pane, between 0.2 and 0.8.
    ratio: f32,
    tabs: Vec<OpenTab>,
    active: Option<usize>,
    scroll: ScrollHandle,
}

pub(crate) struct Sidebar {
    pub(crate) open: bool,
    width: Pixels,
    pub(crate) active: Option<SharedString>,
    /// The panel the user picked last. It may not be registered yet, since
    /// community plugins load after the window opens, or no longer, after
    /// its plugin was disabled; the sidebar shows it whenever it exists.
    chosen: Option<SharedString>,
    order: Vec<String>,
    hidden: Vec<String>,
}

impl Sidebar {
    fn from_layout(layout: &SidebarLayout) -> Self {
        let active: Option<SharedString> = layout.active.clone().map(Into::into);
        Self {
            open: layout.open,
            width: clamp_width(px(layout.width)),
            chosen: active.clone(),
            active,
            order: layout.order.clone(),
            hidden: layout.hidden.clone(),
        }
    }

    fn to_layout(&self) -> SidebarLayout {
        SidebarLayout {
            open: self.open,
            width: self.width.as_f32(),
            active: self
                .chosen
                .as_ref()
                .or(self.active.as_ref())
                .map(ToString::to_string),
            order: self.order.clone(),
            hidden: self.hidden.clone(),
        }
    }
}

#[derive(Clone)]
struct RibbonEntry {
    id: SharedString,
    label: SharedString,
    tooltip: SharedString,
    icon: Icon,
    enabled: bool,
    kind: RibbonKind,
}

#[derive(Clone)]
enum RibbonKind {
    Plugin(RibbonItem),
    Theme,
    Settings,
}

/// The drag payload of a sidebar's resize handle.
#[derive(Clone)]
struct SidebarResize(PanelPlacement);

impl Render for SidebarResize {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

struct OpenTab {
    editor: Entity<NoteEditor>,
    _subscription: Subscription,
}

struct OpenMenu {
    menu: Option<Entity<PopupMenu>>,
    position: Point<Pixels>,
    _dismiss: Subscription,
}

/// The drag payload of the divider between two note panes.
#[derive(Clone)]
struct EditorSplitDrag;

/// A note tab being dragged to a new place in its row, or into the other pane.
#[derive(Clone)]
struct TabDrag {
    pane: EditorPane,
    index: usize,
    title: SharedString,
}

/// Where a dragged tab would land. `index` is a position in the pane's
/// current list: before that tab, or just past the last one.
#[derive(Clone, Copy, PartialEq, Eq)]
struct TabSlot {
    pane: EditorPane,
    index: usize,
}

/// The tab drawn under the pointer while one is dragged.
struct TabGhost {
    title: SharedString,
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
fn same_pane_landing(from: usize, to: usize) -> Option<usize> {
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

/// The window's content: the ribbon, two sidebars of plugin panels, the
/// tabs of open notes and the status bar.
pub struct Workspace {
    focus_handle: FocusHandle,
    pub(crate) vault: Entity<VaultModel>,
    model: Entity<WorkspaceModel>,
    tabs: Vec<OpenTab>,
    active_tab: Option<usize>,
    tab_scroll: ScrollHandle,
    /// The pane that receives a new note, close-tab, and the view mode,
    /// when focus is not inside one of the editors.
    focus: EditorPane,
    split: Option<SplitGroup>,
    tab_menu: Option<OpenMenu>,
    /// Recently opened notes, most recent first.
    pub(crate) recent: Vec<VaultPath>,
    left: Sidebar,
    right: Sidebar,
    panels: Vec<Panel>,
    /// The view of each panel shown so far, with the panel instance it was
    /// built from.
    panel_views: HashMap<SharedString, (u64, AnyView)>,
    status_views: Vec<(SharedString, u64, AnyView)>,
    tab_bar_views: Vec<(SharedString, u64, AnyView)>,
    pub(crate) palette: Entity<CommandState>,
    pub(crate) palette_open: bool,
    pub(crate) switcher: Entity<CommandState>,
    pub(crate) switcher_open: bool,
    pub(crate) switcher_matches: Vec<crate::palette::SwitcherMatch>,
    layout_path: PathBuf,
    save_layout: Option<Task<()>>,
    ribbon: nav::RibbonLayout,
    /// The icon under the pointer during a nav drag, and which icon is held.
    nav_drop: Option<nav::NavDrop>,
    nav_dragging: Option<SharedString>,
    /// The gap shown while a note tab is dragged, and which tab is held.
    tab_drop: Option<TabSlot>,
    tab_dragging: Option<(EditorPane, usize)>,
    /// Which full-screen page a phone shows. A wide window ignores it.
    phone: PhoneScreen,
    _subscriptions: Vec<Subscription>,
}

enum NewNoteKind {
    Markdown,
    Canvas,
    Kanban,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let vault = Notia::vault(cx);
        let model = Notia::workspace(cx);
        let layout_path = vault.read(cx).vault().config_dir().join("workspace.json");
        let layout: Layout = std::fs::read_to_string(&layout_path)
            .ok()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();

        let subscriptions = vec![
            cx.observe_global_in::<PluginHost>(window, |this, window, cx| {
                this.sync_contributions(window, cx);
                cx.notify();
            }),
            cx.observe_window_appearance(window, |_, _, cx| {
                if ThemeCatalog::global(cx).appearance().scheme == ColorScheme::System {
                    ThemeCatalog::apply(cx);
                }
            }),
            cx.subscribe_in(&vault, window, Self::on_vault_event),
            cx.on_app_quit(|this, cx| {
                this.save_all(cx);
                this.save_layout_now(cx);
                async {}
            }),
        ];

        let mut workspace = Self {
            focus_handle: cx.focus_handle(),
            vault,
            model,
            tabs: Vec::new(),
            active_tab: None,
            tab_scroll: ScrollHandle::new(),
            focus: EditorPane::Main,
            split: None,
            tab_menu: None,
            recent: layout.recent,
            left: Sidebar::from_layout(&layout.left),
            right: Sidebar::from_layout(&layout.right),
            panels: Vec::new(),
            panel_views: HashMap::new(),
            status_views: Vec::new(),
            tab_bar_views: Vec::new(),
            palette: cx.new(|cx| CommandState::new(window, cx)),
            palette_open: false,
            switcher: cx.new(|cx| CommandState::new(window, cx)),
            switcher_open: false,
            switcher_matches: Vec::new(),
            layout_path,
            save_layout: None,
            ribbon: layout.ribbon,
            nav_drop: None,
            nav_dragging: None,
            tab_drop: None,
            tab_dragging: None,
            phone: PhoneScreen::Browse,
            _subscriptions: subscriptions,
        };
        workspace.sync_contributions(window, cx);

        let existing: Vec<VaultPath> = layout
            .tabs
            .into_iter()
            .filter(|path| workspace.vault.read(cx).vault().exists(path))
            .collect();
        for path in existing {
            let editor = cx.new(|cx| NoteEditor::new(path, window, cx));
            let tab = workspace.make_tab(editor, window, cx);
            workspace.tabs.push(tab);
        }
        workspace.restore_split(layout.split, window, cx);
        match layout.active_tab.filter(|ix| *ix < workspace.tabs.len()) {
            Some(ix) => workspace.activate_tab(ix, window, cx),
            None if !workspace.tabs.is_empty() => workspace.activate_tab(0, window, cx),
            None => {
                let focus_handle = workspace.focus_handle.clone();
                window.defer(cx, move |window, cx| focus_handle.focus(window, cx));
            }
        }
        if gpui_kit::is_mobile() && workspace.active_tab.is_some() {
            workspace.phone = PhoneScreen::Note;
        }
        workspace
    }

    /// Builds the views of newly enabled panels and status items, and drops
    /// the ones whose plugin was disabled.
    fn sync_contributions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let host = PluginHost::global(cx);
        let panels = host.panels();
        let status_items = host.status_items();
        let tab_bar_items = host.tab_bar_items();

        // A reloaded plugin contributes new instances under the same ids.
        let panel_keys: HashSet<(SharedString, u64)> = panels
            .iter()
            .map(|panel| (panel.id().clone(), panel.instance()))
            .collect();
        self.panel_views
            .retain(|id, (instance, _)| panel_keys.contains(&(id.clone(), *instance)));
        for panel in &panels {
            if !self.panel_views.contains_key(panel.id()) {
                let view = panel.build(window, cx);
                self.panel_views
                    .insert(panel.id().clone(), (panel.instance(), view));
            }
        }

        let mut previous: HashMap<(SharedString, u64), AnyView> = self
            .status_views
            .drain(..)
            .map(|(id, instance, view)| ((id, instance), view))
            .collect();
        self.status_views = status_items
            .iter()
            .map(|item| {
                let view = previous
                    .remove(&(item.id().clone(), item.instance()))
                    .unwrap_or_else(|| item.build(window, cx));
                (item.id().clone(), item.instance(), view)
            })
            .collect();

        let mut previous_tabs: HashMap<(SharedString, u64), AnyView> = self
            .tab_bar_views
            .drain(..)
            .map(|(id, instance, view)| ((id, instance), view))
            .collect();
        self.tab_bar_views = tab_bar_items
            .iter()
            .map(|item| {
                let view = previous_tabs
                    .remove(&(item.id().clone(), item.instance()))
                    .unwrap_or_else(|| item.build(window, cx));
                (item.id().clone(), item.instance(), view)
            })
            .collect();

        self.panels = panels;
        self.refresh_panel(PanelPlacement::Left);
        self.refresh_panel(PanelPlacement::Right);
        self.attach_toolbars(cx);
    }

    fn editor_toolbar_views(&self) -> Vec<AnyView> {
        self.panels
            .iter()
            .filter(|panel| panel.placement() == PanelPlacement::Editor)
            .filter_map(|panel| {
                self.panel_views
                    .get(panel.id())
                    .map(|(_, view)| view.clone())
            })
            .collect()
    }

    /// Puts editor toolbars on the focused note and takes them off the others.
    fn attach_toolbars(&mut self, cx: &mut Context<Self>) {
        let toolbars = self.editor_toolbar_views();
        let pane = self.focused_pane();
        let active = self.active_of(pane);
        for editor in self.all_editors() {
            editor.update(cx, |editor, cx| editor.set_toolbars(Vec::new(), cx));
        }
        if let Some(ix) = active
            && let Some(tab) = self.pane_tabs(pane).get(ix)
        {
            let editor = tab.editor.clone();
            editor.update(cx, |editor, cx| editor.set_toolbars(toolbars, cx));
        }
    }

    fn sidebar(&self, placement: PanelPlacement) -> &Sidebar {
        match placement {
            PanelPlacement::Left => &self.left,
            PanelPlacement::Right => &self.right,
            PanelPlacement::Editor => panic!("an editor toolbar is not a sidebar"),
        }
    }

    fn sidebar_mut(&mut self, placement: PanelPlacement) -> &mut Sidebar {
        match placement {
            PanelPlacement::Left => &mut self.left,
            PanelPlacement::Right => &mut self.right,
            PanelPlacement::Editor => panic!("an editor toolbar is not a sidebar"),
        }
    }

    pub(crate) fn active_editor(&self) -> Option<&Entity<NoteEditor>> {
        let pane = if gpui_kit::is_mobile() {
            EditorPane::Main
        } else {
            self.focused_pane()
        };
        self.active_of(pane)
            .and_then(|ix| self.pane_tabs(pane).get(ix))
            .map(|tab| &tab.editor)
    }

    fn focused_pane(&self) -> EditorPane {
        if self.focus == EditorPane::Split && self.split.is_some() {
            EditorPane::Split
        } else {
            EditorPane::Main
        }
    }

    /// The pane whose editor holds focus, or the last one that did.
    fn note_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> EditorPane {
        self.note_focus(window, cx);
        self.focused_pane()
    }

    fn note_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if gpui_kit::is_mobile() {
            self.focus = EditorPane::Main;
            return;
        }
        let detected = if self.split.as_ref().is_some_and(|split| {
            split
                .tabs
                .iter()
                .any(|tab| tab.editor.read(cx).contains_focus(window, cx))
        }) {
            EditorPane::Split
        } else if self
            .tabs
            .iter()
            .any(|tab| tab.editor.read(cx).contains_focus(window, cx))
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

    fn pane_tabs(&self, pane: EditorPane) -> &[OpenTab] {
        match pane {
            EditorPane::Main => &self.tabs,
            EditorPane::Split => self
                .split
                .as_ref()
                .map(|split| split.tabs.as_slice())
                .unwrap_or(&[]),
        }
    }

    fn active_of(&self, pane: EditorPane) -> Option<usize> {
        match pane {
            EditorPane::Main => self.active_tab,
            EditorPane::Split => self.split.as_ref().and_then(|split| split.active),
        }
    }

    fn set_active(&mut self, pane: EditorPane, active: Option<usize>) {
        match pane {
            EditorPane::Main => self.active_tab = active,
            EditorPane::Split => {
                if let Some(split) = &mut self.split {
                    split.active = active;
                }
            }
        }
    }

    fn all_editors(&self) -> Vec<Entity<NoteEditor>> {
        self.tabs
            .iter()
            .chain(self.split.iter().flat_map(|split| split.tabs.iter()))
            .map(|tab| tab.editor.clone())
            .collect()
    }

    fn find_tab(&self, pane: EditorPane, path: &VaultPath, cx: &App) -> Option<usize> {
        self.pane_tabs(pane)
            .iter()
            .position(|tab| tab.editor.read(cx).path() == path)
    }

    fn restore_split(
        &mut self,
        saved: Option<SavedSplit>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(saved) = saved else {
            return;
        };
        let mut tabs = Vec::new();
        for path in saved.tabs {
            if !self.vault.read(cx).vault().exists(&path) {
                continue;
            }
            let editor = cx.new(|cx| NoteEditor::new(path, window, cx));
            tabs.push(self.make_tab(editor, window, cx));
        }
        if tabs.is_empty() {
            return;
        }
        let active = saved.active_tab.filter(|ix| *ix < tabs.len()).or(Some(0));
        self.split = Some(SplitGroup {
            axis: saved.axis,
            ratio: saved.ratio.clamp(0.2, 0.8),
            tabs,
            active,
            scroll: ScrollHandle::new(),
        });
    }

    fn make_tab(
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
            editor,
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

    fn open_in_pane(
        &mut self,
        pane: EditorPane,
        path: VaultPath,
        options: OpenOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !path.is_note() {
            let path = self.vault.read(cx).vault().abs_path(&path);
            cx.open_with_system(&path);
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

    fn insert_note(
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
            let previous = self.pane_tabs(pane)[active].editor.clone();
            previous.update(cx, |editor, cx| {
                editor.apply_edits(window, cx);
                editor.save(cx);
            });
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
    fn take_tab(&mut self, pane: EditorPane, ix: usize) -> Option<OpenTab> {
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

    fn insert_tab(&mut self, pane: EditorPane, ix: usize, tab: OpenTab) -> usize {
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
    fn move_tab(
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
        let path = self.pane_tabs(from)[from_ix].editor.read(cx).path().clone();
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

    fn show_tab_slot(
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
    fn hover_tab_end(
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
    fn hover_tab_edge(
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
    fn hover_pane_body(
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

    fn drop_tab(&mut self, drag: &TabDrag, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_dragging = None;
        let Some(slot) = self.tab_drop.take() else {
            cx.notify();
            return;
        };
        self.move_tab(drag.pane, drag.index, slot.pane, slot.index, window, cx);
    }

    fn finish_open(
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
            let editor = tab.editor.clone();
            editor.update(cx, |editor, cx| editor.reveal_line(line, window, cx));
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

    fn new_note_in(&mut self, pane: EditorPane, window: &mut Window, cx: &mut Context<Self>) {
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

    fn activate(
        &mut self,
        pane: EditorPane,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.pane_tabs(pane).get(ix).map(|tab| tab.editor.clone()) else {
            return;
        };
        self.focus = pane;
        self.set_active(pane, Some(ix));
        editor.update(cx, |editor, cx| editor.focus(window, cx));
        self.attach_toolbars(cx);
        self.publish_active_note(cx);
        self.schedule_layout_save(cx);
        if gpui_kit::is_mobile() {
            self.phone = PhoneScreen::Note;
        }
        cx.notify();
    }

    fn close_in(
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
        tab.editor.update(cx, |editor, cx| {
            editor.apply_edits(window, cx);
            editor.save(cx);
        });
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
    fn focus_after_empty(&mut self, pane: EditorPane, window: &mut Window, cx: &mut Context<Self>) {
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

    fn close_others(
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

    fn close_to_the_right(
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

    fn close_all_in(&mut self, pane: EditorPane, window: &mut Window, cx: &mut Context<Self>) {
        while !self.pane_tabs(pane).is_empty() {
            let last = self.pane_tabs(pane).len() - 1;
            self.close_in(pane, last, window, cx);
        }
    }

    fn close_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_all_in(EditorPane::Split, window, cx);
    }

    fn ensure_split(&mut self, axis: SplitAxis) {
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
    fn split_tab(
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
            .map(|tab| tab.editor.read(cx).path().clone())
        else {
            return;
        };
        if let Some(tab) = self.pane_tabs(pane).get(ix) {
            let editor = tab.editor.clone();
            editor.update(cx, |editor, cx| {
                editor.apply_edits(window, cx);
                editor.save(cx);
            });
        }
        self.ensure_split(axis);
        if let Some(existing) = self.find_tab(EditorPane::Split, &path, cx) {
            self.activate(EditorPane::Split, existing, window, cx);
            return;
        }
        let index = self.insert_note(EditorPane::Split, path, true, window, cx);
        self.activate(EditorPane::Split, index, window, cx);
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

    pub(crate) fn persist(&mut self, cx: &mut Context<Self>) {
        self.save_all(cx);
        // Drop the delayed write. It would otherwise land after the vault
        // has changed and record this window against the new folder.
        self.save_layout.take();
        self.save_layout_now(cx);
    }

    fn save_all(&mut self, cx: &mut Context<Self>) {
        for editor in self.all_editors() {
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
                        .map(|tab| &tab.editor)
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

    pub(crate) fn toggle_sidebar(&mut self, placement: PanelPlacement, cx: &mut Context<Self>) {
        if gpui_kit::is_mobile() {
            self.phone = match (placement, self.phone) {
                (PanelPlacement::Left, PhoneScreen::Browse) => PhoneScreen::Note,
                (PanelPlacement::Left, _) => PhoneScreen::Browse,
                (PanelPlacement::Right, PhoneScreen::Inspect) => PhoneScreen::Note,
                (PanelPlacement::Right, _) => PhoneScreen::Inspect,
                (PanelPlacement::Editor, _) => self.phone,
            };
            cx.notify();
            return;
        }
        let sidebar = self.sidebar_mut(placement);
        sidebar.open = !sidebar.open;
        self.schedule_layout_save(cx);
        cx.notify();
    }

    fn select_panel(
        &mut self,
        placement: PanelPlacement,
        id: SharedString,
        cx: &mut Context<Self>,
    ) {
        let sidebar = self.sidebar_mut(placement);
        sidebar.open = true;
        sidebar.chosen = Some(id.clone());
        sidebar.active = Some(id);
        if gpui_kit::is_mobile() {
            self.phone = match placement {
                PanelPlacement::Left => PhoneScreen::Browse,
                PanelPlacement::Right => PhoneScreen::Inspect,
                PanelPlacement::Editor => self.phone,
            };
        }
        self.schedule_layout_save(cx);
        cx.notify();
    }

    pub(crate) fn show_panel(&mut self, id: &str, cx: &mut Context<Self>) {
        let found = self
            .panels
            .iter()
            .find(|panel| panel.id().as_ref() == id)
            .map(|panel| (panel.placement(), panel.id().clone()));
        let Some((placement, id)) = found else {
            return;
        };
        // Editor toolbars are already above the note.
        if placement == PanelPlacement::Editor {
            return;
        }
        // A command that opens a panel the user had hidden puts the icon back.
        nav::unhide(&mut self.sidebar_mut(placement).hidden, id.as_ref());
        self.select_panel(placement, id, cx);
    }

    fn publish_active_note(&mut self, cx: &mut Context<Self>) {
        let note = self.active_editor().map(|editor| {
            let editor = editor.read(cx);
            ActiveNote::new(editor.path().clone(), editor.text(cx), editor.mode())
                .with_cursor_line(editor.cursor_line(cx))
        });
        self.model
            .update(cx, |model, cx| model.set_active_note(note, cx));
    }

    fn on_vault_event(
        &mut self,
        _: &Entity<VaultModel>,
        event: &VaultEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            VaultEvent::Trashed(path) => {
                for pane in [EditorPane::Split, EditorPane::Main] {
                    loop {
                        let Some(ix) = self
                            .pane_tabs(pane)
                            .iter()
                            .position(|tab| tab.editor.read(cx).path().starts_with(path))
                        else {
                            break;
                        };
                        self.close_in(pane, ix, window, cx);
                    }
                }
                self.recent.retain(|recent| !recent.starts_with(path));
            }
            VaultEvent::Renamed { from, to } => {
                for recent in &mut self.recent {
                    if let Some(moved) = recent.rebase(from, to) {
                        *recent = moved;
                    }
                }
                self.schedule_layout_save(cx);
            }
            _ => {}
        }
    }

    fn schedule_layout_save(&mut self, cx: &mut Context<Self>) {
        self.save_layout = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            _ = this.update(cx, |this, cx| this.save_layout_now(cx));
        }));
    }

    fn save_layout_now(&self, cx: &App) {
        let layout = Layout {
            left: self.left.to_layout(),
            right: self.right.to_layout(),
            ribbon: self.ribbon.clone(),
            tabs: self
                .tabs
                .iter()
                .map(|tab| tab.editor.read(cx).path().clone())
                .collect(),
            active_tab: self.active_tab,
            split: self.split.as_ref().map(|split| SavedSplit {
                axis: split.axis,
                ratio: split.ratio,
                tabs: split
                    .tabs
                    .iter()
                    .map(|tab| tab.editor.read(cx).path().clone())
                    .collect(),
                active_tab: split.active,
            }),
            recent: self.recent.clone(),
        };
        let saved = serde_json::to_string_pretty(&layout)
            .map_err(std::io::Error::other)
            .and_then(|json| std::fs::write(&self.layout_path, json));
        if let Err(error) = saved {
            log::error!("could not save {}: {error}", self.layout_path.display());
        }
    }

    fn render_title_row(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (sidebar_bg, sidebar_border, tab_bar_bg) =
            (theme.sidebar, theme.sidebar_border, theme.tab_bar);
        let traffic_lights = cfg!(target_os = "macos") && !window.is_fullscreen();
        // The ribbon is below this row. A sidebar's header has to be exactly
        // as wide as the sidebar, or its icons drift away from the column.
        let left_width = RIBBON_WIDTH
            + if self.left.open {
                self.left.width
            } else {
                px(0.)
            };

        let left = h_flex()
            .flex_none()
            .h_full()
            .w(left_width)
            .items_center()
            .when(self.left.open, |this| {
                this.bg(sidebar_bg)
                    .border_r_1()
                    .border_color(sidebar_border)
                    // The window buttons cover the ribbon and the start of
                    // the sidebar; the collapse button is the first thing
                    // clear of them.
                    .when(traffic_lights, |this| this.pl(TRAFFIC_LIGHTS_WIDTH))
                    .when(!traffic_lights, |this| this.pl_1())
                    .child(title_control(self.sidebar_toggle(PanelPlacement::Left, cx)))
                    .child(
                        title_control(self.render_panel_switcher(PanelPlacement::Left, cx))
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden(),
                    )
            });

        let right = h_flex()
            .flex_none()
            .h_full()
            .items_center()
            .when(self.right.open, |this| {
                this.w(self.right.width)
                    .pl_2()
                    .pr_2()
                    .bg(sidebar_bg)
                    .border_l_1()
                    .border_color(sidebar_border)
                    .child(
                        title_control(self.render_panel_switcher(PanelPlacement::Right, cx))
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden(),
                    )
                    // The collapse button is the last thing in the sidebar.
                    .child(title_control(
                        self.sidebar_toggle(PanelPlacement::Right, cx),
                    ))
            });

        TitleBar::new()
            .pl_0()
            .bg(tab_bar_bg)
            .border_color(sidebar_border)
            .child(
                h_flex()
                    .size_full()
                    .items_stretch()
                    .child(left)
                    .child(self.render_tab_strip(traffic_lights, cx))
                    .child(right),
            )
    }

    fn sidebar_toggle(
        &self,
        placement: PanelPlacement,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (id, icon, tooltip) = match placement {
            PanelPlacement::Left => (
                "toggle-left-sidebar",
                if self.left.open {
                    IconName::PanelLeftClose
                } else {
                    IconName::PanelLeftOpen
                },
                "Toggle left sidebar",
            ),
            PanelPlacement::Right => (
                "toggle-right-sidebar",
                if self.right.open {
                    IconName::PanelRightClose
                } else {
                    IconName::PanelRightOpen
                },
                "Toggle right sidebar",
            ),
            PanelPlacement::Editor => panic!("an editor toolbar is not a sidebar"),
        };
        Button::new(id)
            .icon(icon)
            .ghost()
            .small()
            .tooltip(tooltip)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_sidebar(placement, cx)))
    }

    fn refresh_panel(&mut self, placement: PanelPlacement) {
        let known: Vec<SharedString> = self
            .panels
            .iter()
            .filter(|panel| panel.placement() == placement)
            .map(|panel| panel.id().clone())
            .collect();
        let ids: Vec<String> = known.iter().map(ToString::to_string).collect();
        let order = self.sidebar(placement).order.clone();
        let hidden = self.sidebar(placement).hidden.clone();
        let visible = nav::visible_ids(&ids, &order, &hidden);
        let is_visible = |id: &SharedString| visible.iter().any(|item| item == id.as_ref());
        let exists = |id: &SharedString| known.iter().any(|item| item == id);
        let sidebar = self.sidebar_mut(placement);
        // A hidden panel was turned off on purpose. A missing one may still
        // be loading with its plugin, so that choice is kept.
        if sidebar
            .chosen
            .as_ref()
            .is_some_and(|id| exists(id) && !is_visible(id))
        {
            sidebar.chosen = visible.first().map(|id| SharedString::from(id.as_str()));
        }
        sidebar.active = sidebar
            .chosen
            .clone()
            .filter(|id| is_visible(id))
            .or_else(|| sidebar.active.clone().filter(|id| is_visible(id)))
            .or_else(|| visible.first().map(|id| SharedString::from(id.as_str())));
    }

    fn panel_ids(&self, placement: PanelPlacement) -> Vec<String> {
        self.panels
            .iter()
            .filter(|panel| panel.placement() == placement)
            .map(|panel| panel.id().to_string())
            .collect()
    }

    fn move_panel(&mut self, placement: PanelPlacement, id: &str, slot: &nav::Slot) -> bool {
        let available = self.panel_ids(placement);
        let hidden = self.sidebar(placement).hidden.clone();
        nav::move_linear(
            &mut self.sidebar_mut(placement).order,
            &available,
            &hidden,
            id,
            slot,
        )
    }

    fn apply_nav_move(&mut self, id: &str, target: &nav::NavDrop, cx: &App) -> bool {
        match target.bar {
            nav::NavBar::RibbonTop | nav::NavBar::RibbonBottom => nav::move_ribbon(
                &mut self.ribbon,
                &ribbon_available(cx),
                id,
                target.bar,
                &target.slot,
            ),
            nav::NavBar::Left => self.move_panel(PanelPlacement::Left, id, &target.slot),
            nav::NavBar::Right => self.move_panel(PanelPlacement::Right, id, &target.slot),
        }
    }

    fn hover_nav(
        &mut self,
        event: &DragMoveEvent<nav::NavDrag>,
        bar: nav::NavBar,
        target: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        if !event.bounds.contains(&event.event.position) {
            return;
        }
        let drag = event.drag(cx).clone();
        if !nav::same_menu(drag.bar, bar) {
            return;
        }
        if target.as_ref().is_some_and(|id| id == &drag.id) {
            self.clear_nav_drop(cx);
            return;
        }
        let after = if bar.horizontal() {
            let mid = event.bounds.left().as_f32() + event.bounds.size.width.as_f32() * 0.5;
            event.event.position.x.as_f32() > mid
        } else {
            let mid = event.bounds.top().as_f32() + event.bounds.size.height.as_f32() * 0.5;
            event.event.position.y.as_f32() > mid
        };
        let slot = match target {
            Some(id) if after => nav::Slot::After(id.to_string()),
            Some(id) => nav::Slot::Before(id.to_string()),
            None => nav::Slot::End,
        };
        let next = nav::NavDrop { bar, slot };
        if self.nav_drop.as_ref() != Some(&next) {
            self.nav_drop = Some(next);
            cx.notify();
        }
    }

    fn clear_nav_drop(&mut self, cx: &mut Context<Self>) {
        if self.nav_drop.take().is_some() {
            cx.notify();
        }
    }

    fn drop_nav(&mut self, drag: &nav::NavDrag, expected: nav::NavBar, cx: &mut Context<Self>) {
        let Some(target) = self.nav_drop.take() else {
            cx.notify();
            return;
        };
        if target.bar == expected && nav::same_menu(drag.bar, target.bar) {
            let id = drag.id.to_string();
            if self.apply_nav_move(&id, &target, cx) {
                self.schedule_layout_save(cx);
            }
        }
        cx.notify();
    }

    fn toggle_ribbon(&mut self, id: &str, cx: &mut Context<Self>) {
        nav::toggle_hidden(&mut self.ribbon.hidden, id);
        self.schedule_layout_save(cx);
        cx.notify();
    }

    fn toggle_panel(&mut self, placement: PanelPlacement, id: &str, cx: &mut Context<Self>) {
        nav::toggle_hidden(&mut self.sidebar_mut(placement).hidden, id);
        self.refresh_panel(placement);
        self.schedule_layout_save(cx);
        cx.notify();
    }

    fn nav_button(
        &self,
        bar: nav::NavBar,
        id: SharedString,
        icon: Icon,
        tooltip: SharedString,
        selected: bool,
        disabled: bool,
        compact: bool,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mark_after = self.nav_drop.as_ref().and_then(|drop| {
            if drop.bar != bar {
                return None;
            }
            match &drop.slot {
                nav::Slot::Before(target) if target.as_str() == id.as_ref() => Some(false),
                nav::Slot::After(target) if target.as_str() == id.as_ref() => Some(true),
                _ => None,
            }
        });
        let dimmed = self.nav_dragging.as_ref() == Some(&id);
        let drag = nav::NavDrag {
            bar,
            id: id.clone(),
            icon: icon.clone(),
        };
        let workspace = cx.weak_entity();
        let target = id.clone();
        let element_id = SharedString::from(format!("nav-{}-{id}", bar.tag()));
        let on_move = cx.listener(move |this, event: &DragMoveEvent<nav::NavDrag>, _, cx| {
            this.hover_nav(event, bar, Some(target.clone()), cx);
        });
        let on_drop = cx.listener(move |this, drag: &nav::NavDrag, _, cx| {
            this.drop_nav(drag, bar, cx);
        });
        nav::icon_button(
            nav::IconSpec {
                element_id: element_id.into(),
                icon,
                tooltip,
                selected,
                disabled,
                compact,
                dimmed,
                horizontal: bar.horizontal(),
                mark_after,
                drag,
            },
            on_click,
            move |drag, cx| {
                let Some(workspace) = workspace.upgrade() else {
                    return;
                };
                let dragging = drag.id.clone();
                // The drag is installed after this returns, and the window
                // redraws then. Notifying here would render before that and
                // clear the held icon.
                workspace.update(cx, |this, _| {
                    this.nav_dragging = Some(dragging);
                });
            },
            on_move,
            on_drop,
            cx,
        )
    }

    fn panel_rows(&self, placement: PanelPlacement) -> Vec<nav::MenuRow> {
        let panels: Vec<&Panel> = self
            .panels
            .iter()
            .filter(|panel| panel.placement() == placement)
            .collect();
        let ids: Vec<String> = panels.iter().map(|panel| panel.id().to_string()).collect();
        let sidebar = self.sidebar(placement);
        let visible = nav::visible_ids(&ids, &sidebar.order, &sidebar.hidden);
        let mut rows = Vec::new();
        for id in &visible {
            let Some(panel) = panels
                .iter()
                .find(|panel| panel.id().as_ref() == id.as_str())
            else {
                continue;
            };
            rows.push(nav::MenuRow {
                id: panel.id().clone(),
                label: panel.title().clone(),
                icon: panel.icon().clone(),
                visible: true,
            });
        }
        for panel in &panels {
            if visible.iter().any(|id| id.as_str() == panel.id().as_ref()) {
                continue;
            }
            rows.push(nav::MenuRow {
                id: panel.id().clone(),
                label: panel.title().clone(),
                icon: panel.icon().clone(),
                visible: false,
            });
        }
        rows
    }

    fn panel_placeholder(&self, placement: PanelPlacement, cx: &App) -> Div {
        let any = self
            .panels
            .iter()
            .any(|panel| panel.placement() == placement);
        let message = if any {
            "Right-click the header to choose a panel."
        } else {
            "No panel is turned on. Enable one in Settings."
        };
        div()
            .p_4()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(message)
    }

    fn render_panel_switcher(
        &self,
        placement: PanelPlacement,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let bar = match placement {
            PanelPlacement::Left => nav::NavBar::Left,
            PanelPlacement::Right => nav::NavBar::Right,
            PanelPlacement::Editor => panic!("an editor toolbar is not a sidebar"),
        };
        let active = self.sidebar(placement).active.clone();
        let ids = {
            let sidebar = self.sidebar(placement);
            nav::visible_ids(&self.panel_ids(placement), &sidebar.order, &sidebar.hidden)
        };
        let panels: Vec<(SharedString, Icon, SharedString)> = self
            .panels
            .iter()
            .filter(|panel| panel.placement() == placement)
            .map(|panel| {
                (
                    panel.id().clone(),
                    panel.icon().clone(),
                    panel.title().clone(),
                )
            })
            .collect();
        let workspace = cx.entity();
        let mut icons = Vec::new();
        for id in ids {
            let Some((panel_id, icon, title)) = panels
                .iter()
                .find(|(panel_id, _, _)| panel_id.as_ref() == id.as_str())
                .map(|(panel_id, icon, title)| (panel_id.clone(), icon.clone(), title.clone()))
            else {
                continue;
            };
            let selected = active.as_ref() == Some(&panel_id);
            let owner = workspace.clone();
            let clicked = panel_id.clone();
            icons.push(
                self.nav_button(
                    bar,
                    panel_id,
                    icon,
                    title,
                    selected,
                    false,
                    true,
                    move |_, app| {
                        owner.update(app, |this, cx| {
                            this.select_panel(placement, clicked.clone(), cx);
                        });
                    },
                    cx,
                )
                .into_any_element(),
            );
        }
        let rows = self.panel_rows(placement);
        let toggle: Rc<dyn Fn(&str, &mut App)> = Rc::new(move |id, app| {
            let id = id.to_string();
            workspace.update(app, |this, cx| this.toggle_panel(placement, &id, cx));
        });
        h_flex()
            .id(("panel-switcher", placement as usize))
            .gap_0p5()
            .items_center()
            .min_h(px(28.))
            .children(icons)
            .context_menu(move |menu, _, _| {
                nav::visibility_menu(menu, rows.clone(), toggle.clone())
            })
    }

    fn note_tabs(&self, pane: EditorPane, cx: &mut Context<Self>) -> Vec<Tab> {
        let id_prefix = match pane {
            EditorPane::Main => "close-main",
            EditorPane::Split => "close-split",
        };
        let marker = cx.theme().drag_border;
        let workspace = cx.weak_entity();
        self.pane_tabs(pane)
            .iter()
            .enumerate()
            .map(|(ix, tab)| {
                let editor = tab.editor.read(cx);
                let path = editor.path().clone();
                let title = editor.title();
                let insert_before = self
                    .tab_drop
                    .is_some_and(|slot| slot.pane == pane && slot.index == ix);
                let dragging = self.tab_dragging == Some((pane, ix));
                let owner = workspace.clone();
                Tab::new()
                    .label(title.clone())
                    .tooltip(note_tooltip(path))
                    .when(insert_before, |tab| {
                        tab.border_l(px(2.)).border_color(marker)
                    })
                    .when(dragging, |tab| tab.opacity(0.45))
                    .cursor_grab()
                    .suffix(
                        Button::new((id_prefix, ix))
                            .icon(IconName::Close)
                            .ghost()
                            .xsmall()
                            .tooltip("Close")
                            // The press belongs to the button. Letting it
                            // reach the tab would drag the tab as it closes.
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_in(pane, ix, window, cx);
                            })),
                    )
                    // TabBar only accepts Tab children, so the menu is a
                    // right-click on the tab rather than a wrapper.
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.open_tab_menu(pane, ix, event.position, window, cx);
                        }),
                    )
                    // A middle click closes the tab, the same way a browser tab does.
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.close_in(pane, ix, window, cx);
                        }),
                    )
                    .on_drag(
                        TabDrag {
                            pane,
                            index: ix,
                            title,
                        },
                        move |drag, _, _, cx| {
                            if let Some(workspace) = owner.upgrade() {
                                let dragging = (drag.pane, drag.index);
                                // The drag is installed after this returns.
                                // Notifying here would redraw first and drop
                                // the held tab.
                                workspace.update(cx, |this, _| {
                                    this.tab_dragging = Some(dragging);
                                    this.focus = drag.pane;
                                    this.set_active(drag.pane, Some(drag.index));
                                });
                            }
                            let title = drag.title.clone();
                            cx.new(|_| TabGhost { title })
                        },
                    )
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<TabDrag>, _, cx| {
                            this.hover_tab_edge(pane, ix, event, cx);
                        },
                    ))
                    .on_drop(cx.listener(|this, drag: &TabDrag, window, cx| {
                        this.drop_tab(drag, window, cx);
                    }))
            })
            .collect()
    }

    /// A hairline at the end of a tab row when a drop would land there.
    fn tab_end_mark(&self, pane: EditorPane, cx: &App) -> Option<Div> {
        let slot = self.tab_drop?;
        if slot.pane != pane || slot.index != self.pane_tabs(pane).len() {
            return None;
        }
        Some(
            div()
                .flex_none()
                .w(px(2.))
                .h(px(16.))
                .rounded_full()
                .bg(cx.theme().drag_border),
        )
    }

    fn render_tab_strip(&self, traffic_lights: bool, cx: &mut Context<Self>) -> AnyElement {
        // A side-by-side split shares the title row. The two strips use the
        // same weights as the panes under them, so the tabs stay on one line
        // and the divider lines up. A split stacked downward keeps its own
        // tab row on the lower pane.
        let Some(ratio) = self
            .split
            .as_ref()
            .filter(|split| split.axis == SplitAxis::Right)
            .map(|split| split.ratio)
        else {
            return self
                .render_title_tabs(EditorPane::Main, traffic_lights, cx)
                .into_any_element();
        };
        h_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .items_stretch()
            .child(weighted_pane(
                ratio,
                true,
                self.render_title_tabs(EditorPane::Main, traffic_lights, cx),
            ))
            .child(weighted_pane(
                1. - ratio,
                true,
                self.render_title_tabs(EditorPane::Split, traffic_lights, cx),
            ))
            .into_any_element()
    }

    /// One pane's tabs, in the title bar. The original pane carries the
    /// left-edge controls; the rightmost pane carries the right-edge ones.
    fn render_title_tabs(
        &self,
        pane: EditorPane,
        traffic_lights: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tabs = self.note_tabs(pane, cx);
        let active = self.active_of(pane);
        let scroll = match pane {
            EditorPane::Main => self.tab_scroll.clone(),
            EditorPane::Split => self
                .split
                .as_ref()
                .map(|split| split.scroll.clone())
                .unwrap_or_else(ScrollHandle::new),
        };
        let bar_id = match pane {
            EditorPane::Main => "note-tabs",
            EditorPane::Split => "split-tabs",
        };
        let new_id = match pane {
            EditorPane::Main => "new-note-tab",
            EditorPane::Split => "split-new-note",
        };
        let rightmost = match pane {
            EditorPane::Main => !self
                .split
                .as_ref()
                .is_some_and(|split| split.axis == SplitAxis::Right),
            EditorPane::Split => true,
        };
        let border = cx.theme().border;

        // The tab title, the new-note button and a closed sidebar's toggle
        // share the vertical middle of the title bar.
        h_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .items_center()
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<TabDrag>, _, cx| {
                    this.hover_tab_end(pane, event, cx);
                }),
            )
            .on_drop(cx.listener(|this, drag: &TabDrag, window, cx| {
                this.drop_tab(drag, window, cx);
            }))
            .when(pane == EditorPane::Split, |this| {
                this.border_l_1().border_color(border).pl_1()
            })
            .when(
                pane == EditorPane::Main && !self.left.open && traffic_lights,
                |this| {
                    // The window buttons reach past the ribbon into this row.
                    this.pl(px(TRAFFIC_LIGHTS_WIDTH.as_f32() - RIBBON_WIDTH.as_f32()))
                },
            )
            .when(pane == EditorPane::Main && !self.left.open, |this| {
                this.child(title_control(self.sidebar_toggle(PanelPlacement::Left, cx)))
            })
            .child(
                // Grow only until the tabs fill the row, so the new-note
                // button stays next to the last tab.
                div()
                    .min_w_0()
                    .flex_grow(0.)
                    .flex_shrink(1.)
                    .overflow_hidden()
                    .child(stop_title_drag(
                        TabBar::new(bar_id)
                            .folder()
                            .max_width(px(220.))
                            .track_scroll(&scroll)
                            .when_some(active, |bar, ix| bar.selected_index(ix))
                            .on_click(cx.listener(move |this, ix: &usize, window, cx| {
                                this.activate(pane, *ix, window, cx);
                            }))
                            .children(tabs),
                    )),
            )
            .children(self.tab_end_mark(pane, cx))
            .child(title_control(
                Button::new(new_id)
                    .icon(IconName::Plus)
                    .ghost()
                    .xsmall()
                    .tooltip("New note")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.new_note_in(pane, window, cx);
                    })),
            ))
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<TabDrag>, _, cx| {
                            this.hover_tab_end(pane, event, cx);
                        },
                    ))
                    .on_drop(cx.listener(|this, drag: &TabDrag, window, cx| {
                        this.drop_tab(drag, window, cx);
                    })),
            )
            .when(pane == EditorPane::Split, |this| {
                this.child(title_control(
                    Button::new("close-split")
                        .icon(IconName::Close)
                        .ghost()
                        .xsmall()
                        .tooltip("Close split")
                        .on_click(cx.listener(|this, _, window, cx| this.close_split(window, cx))),
                ))
            })
            .when(rightmost, |this| {
                this.children(
                    self.tab_bar_views
                        .iter()
                        .map(|(_, _, view)| title_control(view.clone()).pr_2()),
                )
                .when(!self.right.open, |this| {
                    this.child(title_control(self.sidebar_toggle(PanelPlacement::Right, cx)).pr_2())
                })
            })
    }

    fn render_ribbon(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (sidebar_bg, sidebar_border, marker) = {
            let theme = cx.theme();
            (theme.sidebar, theme.sidebar_border, theme.drag_border)
        };
        let is_dark = cx.theme().is_dark();
        let entries = ribbon_entries(cx);
        let ids: Vec<String> = entries.iter().map(|entry| entry.id.to_string()).collect();
        let (top, bottom) = nav::ribbon_zones(&ids, &self.ribbon);
        let rows = menu_rows(&entries, &top, &bottom);
        let workspace = cx.entity();
        let toggle: Rc<dyn Fn(&str, &mut App)> = Rc::new(move |id, app| {
            let id = id.to_string();
            workspace.update(app, |this, cx| this.toggle_ribbon(&id, cx));
        });
        let gap_marked = self.nav_drop.as_ref().is_some_and(|drop| {
            drop.bar == nav::NavBar::RibbonTop && matches!(drop.slot, nav::Slot::End)
        });

        let mut above = Vec::new();
        for id in &top {
            if let Some(entry) = entries
                .iter()
                .find(|entry| entry.id.as_ref() == id.as_str())
            {
                above.push(self.ribbon_icon(nav::NavBar::RibbonTop, entry, is_dark, cx));
            }
        }
        let mut below = Vec::new();
        for id in &bottom {
            if let Some(entry) = entries
                .iter()
                .find(|entry| entry.id.as_ref() == id.as_str())
            {
                below.push(self.ribbon_icon(nav::NavBar::RibbonBottom, entry, is_dark, cx));
            }
        }

        v_flex()
            .id("ribbon")
            .flex_none()
            .w(RIBBON_WIDTH)
            .h_full()
            .items_center()
            .py_2()
            .gap_1()
            .bg(sidebar_bg)
            .border_r_1()
            .border_color(sidebar_border)
            .children(above)
            .child(self.ribbon_gap(gap_marked, marker, cx))
            .children(below)
            .context_menu(move |menu, _, _| {
                nav::visibility_menu(menu, rows.clone(), toggle.clone())
            })
    }

    fn ribbon_icon(
        &self,
        bar: nav::NavBar,
        entry: &RibbonEntry,
        is_dark: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kind = entry.kind.clone();
        self.nav_button(
            bar,
            entry.id.clone(),
            entry.icon.clone(),
            entry.tooltip.clone(),
            false,
            !entry.enabled,
            false,
            move |window, cx| ribbon_action(kind.clone(), is_dark, window, cx),
            cx,
        )
        .into_any_element()
    }

    /// The empty stretch between the top icons and the ones pinned to the
    /// bottom. Dropping here moves an icon to the end of the top group.
    fn ribbon_gap(&self, marked: bool, color: Hsla, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("ribbon-gap")
            .flex_1()
            .w_full()
            .relative()
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<nav::NavDrag>, _, cx| {
                    this.hover_nav(event, nav::NavBar::RibbonTop, None, cx);
                }),
            )
            .on_drop(cx.listener(|this, drag: &nav::NavDrag, _, cx| {
                this.drop_nav(drag, nav::NavBar::RibbonTop, cx);
            }))
            .when(marked, move |this| {
                this.child(
                    div()
                        .absolute()
                        .top(px(0.))
                        .left(px(10.))
                        .right(px(10.))
                        .h(px(2.))
                        .rounded(px(1.))
                        .bg(color),
                )
            })
    }

    fn render_sidebar(
        &self,
        placement: PanelPlacement,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let sidebar = self.sidebar(placement);
        let view = sidebar
            .active
            .as_ref()
            .and_then(|id| self.panel_views.get(id))
            .map(|(_, view)| view.clone());
        let missing = view.is_none();
        let edge = match placement {
            PanelPlacement::Left => HandleEdge::Trailing,
            PanelPlacement::Right => HandleEdge::Leading,
            PanelPlacement::Editor => panic!("an editor toolbar is not a sidebar"),
        };

        div()
            .id(("sidebar", placement as usize))
            .relative()
            .flex_none()
            .h_full()
            .w(sidebar.width)
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .border_color(theme.sidebar_border)
            .map(|this| match placement {
                PanelPlacement::Left => this.border_r_1(),
                PanelPlacement::Right => this.border_l_1(),
                PanelPlacement::Editor => this,
            })
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<SidebarResize>, _, cx| {
                    if event.drag(cx).0 != placement {
                        return;
                    }
                    let x = event.event.position.x;
                    let width = match placement {
                        PanelPlacement::Left => x - event.bounds.left(),
                        PanelPlacement::Right => event.bounds.right() - x,
                        PanelPlacement::Editor => return,
                    };
                    this.sidebar_mut(placement).width = clamp_width(width);
                    this.schedule_layout_save(cx);
                    cx.notify();
                }),
            )
            .child(
                v_flex()
                    .size_full()
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .children(view)
                            .when(missing, |body| {
                                body.child(self.panel_placeholder(placement, cx))
                            }),
                    )
                    .when(placement == PanelPlacement::Left, |column| {
                        column.child(crate::vaults::footer(cx))
                    }),
            )
            .child(
                resize_handle(("sidebar-resize", placement as usize), Axis::Horizontal)
                    .inside(edge)
                    .on_drag(SidebarResize(placement), |drag, _, _, cx| {
                        cx.new(|_| drag.clone())
                    }),
            )
    }

    fn render_empty_state(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let action_button =
            |id: &'static str, label: &'static str, action: Box<dyn Action>, window: &Window| {
                let hint =
                    Kbd::binding_for_action(action.as_ref(), Some(WORKSPACE_CONTEXT), window);
                h_flex()
                    .gap_2()
                    .child(
                        Button::new(id)
                            .ghost()
                            .label(label)
                            .on_click(move |_, window, cx| {
                                window.dispatch_action(action.boxed_clone(), cx)
                            }),
                    )
                    .children(hint)
            };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_1()
            .child(
                div()
                    .mb_3()
                    .text_xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .child("No file is open"),
            )
            .child(action_button(
                "empty-new-note",
                "Create new note",
                Box::new(NewNote),
                window,
            ))
            .child(action_button(
                "empty-go-to",
                "Go to file",
                Box::new(OpenQuickSwitcher),
                window,
            ))
            .child(action_button(
                "empty-settings",
                "Open settings",
                Box::new(OpenSettings),
                window,
            ))
    }
}

fn ribbon_available(cx: &App) -> Vec<String> {
    let mut ids: Vec<String> = PluginHost::global(cx)
        .ribbon_items()
        .iter()
        .map(|item| item.id().to_string())
        .collect();
    ids.push(nav::THEME_ID.to_string());
    ids.push(nav::SETTINGS_ID.to_string());
    ids
}

fn ribbon_entries(cx: &App) -> Vec<RibbonEntry> {
    let is_dark = cx.theme().is_dark();
    let switchable = ThemeCatalog::global(cx)
        .active_pack()
        .is_none_or(|pack| pack.has_mode(ThemeMode::Light) && pack.has_mode(ThemeMode::Dark));
    let mut entries: Vec<RibbonEntry> = PluginHost::global(cx)
        .ribbon_items()
        .into_iter()
        .map(|item| RibbonEntry {
            id: item.id().clone(),
            label: item.tooltip().clone(),
            tooltip: item.tooltip().clone(),
            icon: item.icon().clone(),
            enabled: true,
            kind: RibbonKind::Plugin(item),
        })
        .collect();
    let theme_tooltip = match (switchable, is_dark) {
        (false, _) => "This theme has a single appearance",
        (true, true) => "Use light mode",
        (true, false) => "Use dark mode",
    };
    entries.push(RibbonEntry {
        id: nav::THEME_ID.into(),
        label: "Theme".into(),
        tooltip: theme_tooltip.into(),
        icon: Icon::new(if is_dark {
            IconName::Sun
        } else {
            IconName::Moon
        }),
        enabled: switchable,
        kind: RibbonKind::Theme,
    });
    entries.push(RibbonEntry {
        id: nav::SETTINGS_ID.into(),
        label: "Settings".into(),
        tooltip: "Settings".into(),
        icon: Icon::new(IconName::Settings),
        enabled: true,
        kind: RibbonKind::Settings,
    });
    entries
}

fn menu_row(entry: &RibbonEntry, visible: bool) -> nav::MenuRow {
    nav::MenuRow {
        id: entry.id.clone(),
        label: entry.label.clone(),
        icon: entry.icon.clone(),
        visible,
    }
}

fn menu_rows(entries: &[RibbonEntry], top: &[String], bottom: &[String]) -> Vec<nav::MenuRow> {
    let mut rows = Vec::new();
    for id in top.iter().chain(bottom) {
        if let Some(entry) = entries
            .iter()
            .find(|entry| entry.id.as_ref() == id.as_str())
        {
            rows.push(menu_row(entry, true));
        }
    }
    for entry in entries {
        let shown = top
            .iter()
            .chain(bottom)
            .any(|id| id.as_str() == entry.id.as_ref());
        if !shown {
            rows.push(menu_row(entry, false));
        }
    }
    rows
}

fn ribbon_action(kind: RibbonKind, is_dark: bool, window: &mut Window, cx: &mut App) {
    match kind {
        RibbonKind::Plugin(item) => item.run(window, cx),
        RibbonKind::Theme => {
            ThemeCatalog::update_appearance(cx, |appearance| {
                appearance.scheme = if is_dark {
                    ColorScheme::Light
                } else {
                    ColorScheme::Dark
                };
            });
        }
        RibbonKind::Settings => window.dispatch_action(Box::new(OpenSettings), cx),
    }
}

fn clamp_width(width: Pixels) -> Pixels {
    px(width
        .as_f32()
        .clamp(SIDEBAR_WIDTHS.0.as_f32(), SIDEBAR_WIDTHS.1.as_f32()))
}

/// Keeps a press on a title-row control from starting a window drag.
fn stop_title_drag(element: impl IntoElement) -> Div {
    div()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(element)
}

/// A title-row control centered on the height of the title bar. The row
/// itself aligns tabs to the bottom, so a bare button would sit too low.
fn title_control(element: impl IntoElement) -> Div {
    stop_title_drag(element).h_full().flex().items_center()
}

fn weighted_pane(ratio: f32, horizontal: bool, child: impl IntoElement) -> Div {
    div()
        .relative()
        .min_w_0()
        .min_h_0()
        .flex_grow(ratio)
        .flex_shrink(1.)
        .flex_basis(px(0.))
        .when(horizontal, |this| this.h_full())
        .when(!horizontal, |this| this.w_full())
        .child(child)
}

fn tab_context_menu(
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
                            .map(|tab| tab.editor.read(cx).path().to_string())
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
                    .map(|tab| tab.editor.read(cx).path().clone())
            }) else {
                return;
            };
            let abs = Notia::vault(cx).read(cx).vault().abs_path(&path);
            cx.reveal_path(&abs);
        }))
}

#[cfg(test)]
mod layout_tests {
    use super::Layout;

    #[test]
    fn workspace_layout_without_nav_fields_still_loads() {
        let json = r#"{
            "left": {"open": true, "width": 260.0, "active": "file-explorer:files"},
            "right": {"open": false, "width": 280.0},
            "tabs": [],
            "recent": []
        }"#;
        let layout: Layout = serde_json::from_str(json).unwrap();
        assert!(layout.ribbon.leading.is_empty());
        assert!(layout.ribbon.hidden.is_empty());
        assert!(layout.left.order.is_empty());
        assert!(layout.left.hidden.is_empty());
        assert!(layout.right.hidden.is_empty());
        assert_eq!(layout.left.active.as_deref(), Some("file-explorer:files"));
        assert!(!layout.right.open);
        assert!(layout.split.is_none());
    }

    #[test]
    fn workspace_layout_restores_a_split() {
        let json = r#"{
            "tabs": ["Welcome.md"],
            "active_tab": 0,
            "split": {
                "axis": "down",
                "ratio": 0.4,
                "tabs": ["Welcome.md"],
                "active_tab": 0
            }
        }"#;
        let layout: Layout = serde_json::from_str(json).unwrap();
        let split = layout.split.as_ref().unwrap();
        assert_eq!(split.axis, super::SplitAxis::Down);
        assert!((split.ratio - 0.4).abs() < f32::EPSILON);
        assert_eq!(split.tabs.len(), 1);
        assert_eq!(split.active_tab, Some(0));
    }

    #[test]
    fn dragging_a_tab_along_its_row_lands_where_the_gap_was() {
        // [A B C D], drag A to before C: the gap is index 2, and A lands at 1.
        assert_eq!(super::same_pane_landing(0, 2), Some(1));
        // Drag A to the end.
        assert_eq!(super::same_pane_landing(0, 4), Some(3));
        // Drag D to the front.
        assert_eq!(super::same_pane_landing(3, 0), Some(0));
        // Drag B to just before D.
        assert_eq!(super::same_pane_landing(1, 3), Some(2));
    }

    #[test]
    fn dropping_a_tab_on_itself_does_not_move_it() {
        assert_eq!(super::same_pane_landing(1, 1), None);
        assert_eq!(super::same_pane_landing(1, 2), None);
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !cx.has_active_drag() {
            self.nav_drop = None;
            self.nav_dragging = None;
            self.tab_drop = None;
            self.tab_dragging = None;
        }
        if gpui_kit::is_mobile() {
            self.render_phone(window, cx).into_any_element()
        } else {
            self.render_desktop(window, cx).into_any_element()
        }
    }
}

impl Workspace {
    fn open_tab_menu(
        &mut self,
        pane: EditorPane,
        ix: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pane_tabs(pane).get(ix).is_none() {
            return;
        }
        self.activate(pane, ix, window, cx);
        let workspace = cx.entity();
        // PopupMenu builds inside this update. Reading the workspace again
        // from that builder panics, so the menu gets a snapshot of the counts.
        let count = self.pane_tabs(pane).len();
        let split_open = self.split.is_some();
        // Escape and a click outside return to the note the menu covered.
        let return_focus = self
            .pane_tabs(pane)
            .get(ix)
            .map(|tab| tab.editor.focus_handle(cx));
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            let menu = tab_context_menu(menu, workspace.clone(), pane, ix, count, split_open);
            match return_focus.clone() {
                Some(handle) => menu.action_context(handle),
                None => menu,
            }
        });
        let dismiss = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            if let Some(open) = &mut this.tab_menu {
                open.menu = None;
            }
            cx.notify();
        });
        menu.focus_handle(cx).focus(window, cx);
        self.tab_menu = Some(OpenMenu {
            menu: Some(menu),
            position,
            _dismiss: dismiss,
        });
        cx.notify();
    }

    fn render_tab_menu(&self, window: &Window) -> Option<AnyElement> {
        let open = self.tab_menu.as_ref()?;
        let menu = open.menu.clone()?;
        let position = open.position;
        Some(
            deferred(
                anchored().child(
                    div()
                        .w(window.bounds().size.width)
                        .h(window.bounds().size.height)
                        .on_scroll(|_, _, cx| cx.stop_propagation())
                        .child(
                            anchored()
                                .position(position)
                                .snap_to_window_with_margin(px(8.))
                                .anchor(Anchor::TopLeft)
                                .child(menu),
                        ),
                ),
            )
            .with_priority(100)
            .into_any_element(),
        )
    }

    fn render_editor_area(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let background = cx.theme().background;
        let primary = self.render_pane(EditorPane::Main, window, cx);
        let Some(split) = self.split.as_ref() else {
            return div()
                .flex_1()
                .min_w_0()
                .h_full()
                .bg(background)
                .child(primary)
                .into_any_element();
        };
        let ratio = split.ratio;
        let horizontal = split.axis == SplitAxis::Right;
        let secondary = self.render_pane(EditorPane::Split, window, cx);
        // Trailing is the right edge of a horizontal drag and the bottom
        // edge of a vertical one, which is the side facing the other pane.
        let drag_axis = if horizontal {
            Axis::Horizontal
        } else {
            Axis::Vertical
        };
        let handle = resize_handle("editor-split-handle", drag_axis)
            .inside(HandleEdge::Trailing)
            .on_drag(EditorSplitDrag, |drag, _, _, cx| cx.new(|_| drag.clone()));
        let primary = weighted_pane(ratio, horizontal, primary).child(handle);
        let secondary = weighted_pane(1. - ratio, horizontal, secondary);
        let panes = if horizontal {
            h_flex()
                .size_full()
                .child(primary)
                .child(secondary)
                .into_any_element()
        } else {
            v_flex()
                .size_full()
                .child(primary)
                .child(secondary)
                .into_any_element()
        };
        div()
            .id("editor-area")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .bg(background)
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<EditorSplitDrag>, _, cx| {
                    let Some(axis) = this.split.as_ref().map(|split| split.axis) else {
                        return;
                    };
                    let bounds = event.bounds;
                    let ratio = match axis {
                        SplitAxis::Right => {
                            let width = bounds.size.width.as_f32();
                            if width < 1. {
                                return;
                            }
                            (event.event.position.x - bounds.left()).as_f32() / width
                        }
                        SplitAxis::Down => {
                            let height = bounds.size.height.as_f32();
                            if height < 1. {
                                return;
                            }
                            (event.event.position.y - bounds.top()).as_f32() / height
                        }
                    };
                    let ratio = ratio.clamp(0.2, 0.8);
                    if let Some(split) = &mut this.split
                        && (split.ratio - ratio).abs() > 0.004
                    {
                        split.ratio = ratio;
                        this.schedule_layout_save(cx);
                        cx.notify();
                    }
                }),
            )
            .child(panes)
            .into_any_element()
    }

    fn render_pane(
        &self,
        pane: EditorPane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let note = match self
            .active_of(pane)
            .and_then(|ix| self.pane_tabs(pane).get(ix))
        {
            Some(tab) => tab.editor.clone().into_any_element(),
            None => self.render_empty_state(window, cx).into_any_element(),
        };
        let drop_layer = match pane {
            EditorPane::Main => "tab-drop-main",
            EditorPane::Split => "tab-drop-split",
        };
        v_flex()
            .size_full()
            .min_h_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    if this.focus != pane {
                        this.focus = pane;
                        this.attach_toolbars(cx);
                        this.publish_active_note(cx);
                        cx.notify();
                    }
                }),
            )
            .when(
                // Side-by-side panes keep both tab rows in the title bar.
                // Only a downward split needs a tab row inside the pane.
                pane == EditorPane::Split
                    && self
                        .split
                        .as_ref()
                        .is_some_and(|split| split.axis == SplitAxis::Down),
                |this| this.child(self.render_split_tabs(cx)),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(note)
                    // The note's own hit targets sit in front of this pane, so
                    // a tab drop on the writing surface needs a layer above
                    // them. It exists only while a tab is held.
                    .when(self.tab_dragging.is_some(), |body| {
                        body.child(
                            div()
                                .id(drop_layer)
                                .absolute()
                                .inset_0()
                                .on_drag_move(cx.listener(
                                    move |this, event: &DragMoveEvent<TabDrag>, _, cx| {
                                        this.hover_pane_body(pane, event, cx);
                                    },
                                ))
                                .on_drop(cx.listener(|this, drag: &TabDrag, window, cx| {
                                    this.drop_tab(drag, window, cx);
                                })),
                        )
                    }),
            )
    }

    fn render_split_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (scroll, active) = self
            .split
            .as_ref()
            .map(|split| (split.scroll.clone(), split.active))
            .unwrap_or_else(|| (ScrollHandle::new(), None));
        let tabs = self.note_tabs(EditorPane::Split, cx);
        let (bg, border) = {
            let theme = cx.theme();
            (theme.tab_bar, theme.border)
        };
        h_flex()
            .flex_none()
            .w_full()
            .items_end()
            .px_1()
            .bg(bg)
            .border_b_1()
            .border_color(border)
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<TabDrag>, _, cx| {
                this.hover_tab_end(EditorPane::Split, event, cx);
            }))
            .on_drop(cx.listener(|this, drag: &TabDrag, window, cx| {
                this.drop_tab(drag, window, cx);
            }))
            .child(
                h_flex()
                    .min_w_0()
                    .flex_1()
                    .items_end()
                    .overflow_hidden()
                    .on_drag_move(cx.listener(|this, event: &DragMoveEvent<TabDrag>, _, cx| {
                        this.hover_tab_end(EditorPane::Split, event, cx);
                    }))
                    .on_drop(cx.listener(|this, drag: &TabDrag, window, cx| {
                        this.drop_tab(drag, window, cx);
                    }))
                    .child(stop_title_drag(
                        TabBar::new("split-tabs")
                            .folder()
                            .max_width(px(220.))
                            .track_scroll(&scroll)
                            .when_some(active, |bar, ix| bar.selected_index(ix))
                            .on_click(cx.listener(|this, ix: &usize, window, cx| {
                                this.activate(EditorPane::Split, *ix, window, cx);
                            }))
                            .children(tabs),
                    ))
                    .children(self.tab_end_mark(EditorPane::Split, cx)),
            )
            .child(title_control(
                Button::new("split-new-note")
                    .icon(IconName::Plus)
                    .ghost()
                    .xsmall()
                    .tooltip("New note")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.new_note_in(EditorPane::Split, window, cx);
                    })),
            ))
            .child(title_control(
                Button::new("close-split")
                    .icon(IconName::Close)
                    .ghost()
                    .xsmall()
                    .tooltip("Close split")
                    .on_click(cx.listener(|this, _, window, cx| this.close_split(window, cx))),
            ))
    }

    fn render_desktop(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.note_focus(window, cx);
        let theme = cx.theme();
        let (background, foreground, font_family) = (
            theme.background,
            theme.foreground,
            theme.font_family.clone(),
        );
        let status_bar_bg = theme.sidebar;
        let menu = self.render_tab_menu(window);

        v_flex()
            .id("workspace")
            .key_context(WORKSPACE_CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(background)
            .text_color(foreground)
            .font_family(font_family)
            .child(self.render_title_row(window, cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .items_stretch()
                    .child(self.render_ribbon(cx))
                    .when(self.left.open, |this| {
                        this.child(self.render_sidebar(PanelPlacement::Left, cx))
                    })
                    .child(self.render_editor_area(window, cx))
                    .when(self.right.open, |this| {
                        this.child(self.render_sidebar(PanelPlacement::Right, cx))
                    }),
            )
            .children(menu)
            .child(
                StatusBar::new()
                    .bg(status_bar_bg)
                    .min_h_6()
                    .text_xs()
                    .map(|bar| {
                        self.status_views
                            .iter()
                            .fold(bar, |bar, (_, _, view)| bar.right(view.clone()))
                    }),
            )
    }
}
