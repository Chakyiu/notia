use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    time::Duration,
};

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, ThemeMode, TitleBar, WindowExt as _,
    button::{Button, ButtonVariants as _},
    command::CommandState,
    h_flex,
    kbd::Kbd,
    menu::ContextMenuExt as _,
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
    recent: Vec<VaultPath>,
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
            recent: Vec::new(),
        }
    }
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

/// The window's content: the ribbon, two sidebars of plugin panels, the
/// tabs of open notes and the status bar.
pub struct Workspace {
    focus_handle: FocusHandle,
    pub(crate) vault: Entity<VaultModel>,
    model: Entity<WorkspaceModel>,
    tabs: Vec<OpenTab>,
    active_tab: Option<usize>,
    tab_scroll: ScrollHandle,
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

    /// Puts editor toolbars on the open note and takes them off the others.
    fn attach_toolbars(&mut self, cx: &mut Context<Self>) {
        let toolbars = self.editor_toolbar_views();
        let active = self.active_tab;
        for (ix, tab) in self.tabs.iter().enumerate() {
            if Some(ix) != active {
                tab.editor
                    .update(cx, |editor, cx| editor.set_toolbars(Vec::new(), cx));
            }
        }
        if let Some(ix) = active
            && let Some(tab) = self.tabs.get(ix)
        {
            tab.editor
                .update(cx, |editor, cx| editor.set_toolbars(toolbars, cx));
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
        self.active_tab
            .and_then(|ix| self.tabs.get(ix))
            .map(|tab| &tab.editor)
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

    /// Opens a note, reusing its tab when it is already open. Without
    /// `in_new_tab`, the note replaces the active tab's note.
    pub fn open_note(
        &mut self,
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
        let existing = self
            .tabs
            .iter()
            .position(|tab| tab.editor.read(cx).path() == &path);
        let ix = match existing {
            Some(ix) => ix,
            None => {
                let editor = cx.new(|cx| NoteEditor::new(path.clone(), window, cx));
                let tab = self.make_tab(editor, window, cx);
                match self.active_tab {
                    Some(active) if !options.is_new_tab() => {
                        self.tabs[active].editor.update(cx, |editor, cx| {
                            editor.apply_edits(window, cx);
                            editor.save(cx);
                        });
                        self.tabs[active] = tab;
                        active
                    }
                    active => {
                        let ix = active.map_or(self.tabs.len(), |active| active + 1);
                        self.tabs.insert(ix, tab);
                        ix
                    }
                }
            }
        };
        self.activate_tab(ix, window, cx);
        if let Some(line) = options.line() {
            self.tabs[ix]
                .editor
                .update(cx, |editor, cx| editor.reveal_line(line, window, cx));
        }
        self.recent.retain(|recent| recent != &path);
        self.recent.insert(0, path);
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
        self.new_named(folder, NewNoteKind::Markdown, window, cx);
    }

    /// Creates an empty Canvas note in `folder`, or the configured folder.
    pub fn new_canvas(
        &mut self,
        folder: Option<VaultPath>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_named(folder, NewNoteKind::Canvas, window, cx);
    }

    /// Creates a Kanban note in `folder`, or the configured folder.
    pub fn new_kanban(
        &mut self,
        folder: Option<VaultPath>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_named(folder, NewNoteKind::Kanban, window, cx);
    }

    fn new_named(
        &mut self,
        folder: Option<VaultPath>,
        kind: NewNoteKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
                self.open_note(path, OpenOptions::new().in_new_tab(), window, cx);
                if let Some(editor) = self.active_editor().cloned() {
                    editor.update(cx, |editor, cx| editor.focus_title(window, cx));
                }
            }
            Err(error) => window.push_notification(format!("Could not create a note: {error}"), cx),
        }
    }

    pub(crate) fn activate_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(ix) else {
            return;
        };
        self.active_tab = Some(ix);
        let editor = tab.editor.clone();
        editor.update(cx, |editor, cx| editor.focus(window, cx));
        self.attach_toolbars(cx);
        self.publish_active_note(cx);
        self.schedule_layout_save(cx);
        cx.notify();
    }

    pub(crate) fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(ix);
        tab.editor.update(cx, |editor, cx| {
            editor.apply_edits(window, cx);
            editor.save(cx);
        });
        self.active_tab = match self.active_tab {
            _ if self.tabs.is_empty() => None,
            Some(active) if active > ix => Some(active - 1),
            Some(active) if active == ix => Some(ix.min(self.tabs.len() - 1)),
            other => other,
        };
        match self.active_tab {
            Some(active) => self.activate_tab(active, window, cx),
            None => {
                self.focus_handle.focus(window, cx);
                self.publish_active_note(cx);
                self.schedule_layout_save(cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn close_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.active_tab {
            self.close_tab(ix, window, cx);
        }
    }

    pub(crate) fn cycle_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.tabs.len();
        if let Some(active) = self.active_tab.filter(|_| count > 1) {
            let next = if forward {
                (active + 1) % count
            } else {
                (active + count - 1) % count
            };
            self.activate_tab(next, window, cx);
        }
    }

    pub(crate) fn set_view_mode(
        &mut self,
        mode: ViewMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.active_editor().cloned() {
            editor.update(cx, |editor, cx| editor.set_mode(mode, window, cx));
        }
    }

    pub(crate) fn toggle_reading_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor().cloned() {
            editor.update(cx, |editor, cx| editor.toggle_mode(window, cx));
        }
    }

    pub(crate) fn save_active(&mut self, cx: &mut Context<Self>) {
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
        for tab in &self.tabs {
            tab.editor.update(cx, |editor, cx| editor.save(cx));
        }
    }

    pub(crate) fn reveal_line(&mut self, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor().cloned() {
            editor.update(cx, |editor, cx| editor.reveal_line(line, window, cx));
        }
    }

    /// The editor with this id, or the active one for `None`.
    pub(crate) fn editor(&self, id: Option<EditorId>, cx: &App) -> Option<&Entity<NoteEditor>> {
        match id {
            Some(id) => self
                .tabs
                .iter()
                .map(|tab| &tab.editor)
                .find(|editor| editor.read(cx).id() == id),
            None => self.active_editor(),
        }
    }

    /// Applies the edits plugins queued on any open note.
    pub(crate) fn apply_edits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editors: Vec<_> = self.tabs.iter().map(|tab| tab.editor.clone()).collect();
        for editor in editors {
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
                for ix in (0..self.tabs.len()).rev() {
                    if self.tabs[ix].editor.read(cx).path().starts_with(path) {
                        self.close_tab(ix, window, cx);
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

    fn render_tab_strip(&self, traffic_lights: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs: Vec<Tab> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(ix, tab)| {
                Tab::new().label(tab.editor.read(cx).title()).suffix(
                    Button::new(("close-tab", ix))
                        .icon(IconName::Close)
                        .ghost()
                        .xsmall()
                        .tooltip("Close")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_tab(ix, window, cx);
                        })),
                )
            })
            .collect();

        // The tab title, the new-note button and a closed sidebar's toggle
        // share the vertical middle of the title bar.
        h_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .items_center()
            .when(!self.left.open && traffic_lights, |this| {
                // The window buttons reach past the ribbon into this row.
                this.pl(px(TRAFFIC_LIGHTS_WIDTH.as_f32() - RIBBON_WIDTH.as_f32()))
            })
            .when(!self.left.open, |this| {
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
                        TabBar::new("note-tabs")
                            .folder()
                            .max_width(px(220.))
                            .track_scroll(&self.tab_scroll)
                            .when_some(self.active_tab, |bar, ix| bar.selected_index(ix))
                            .on_click(cx.listener(|this, ix: &usize, window, cx| {
                                this.activate_tab(*ix, window, cx);
                            }))
                            .children(tabs),
                    )),
            )
            .child(title_control(
                Button::new("new-note-tab")
                    .icon(IconName::Plus)
                    .ghost()
                    .xsmall()
                    .tooltip("New note")
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(NewNote), cx)),
            ))
            .child(div().flex_1().h_full())
            .children(
                self.tab_bar_views
                    .iter()
                    .map(|(_, _, view)| title_control(view.clone()).pr_2()),
            )
            .when(!self.right.open, |this| {
                this.child(title_control(self.sidebar_toggle(PanelPlacement::Right, cx)).pr_2())
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
        }
        if gpui_kit::is_mobile() {
            self.render_phone(window, cx).into_any_element()
        } else {
            self.render_desktop(window, cx).into_any_element()
        }
    }
}

impl Workspace {
    fn render_desktop(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (background, foreground, font_family) = (
            theme.background,
            theme.foreground,
            theme.font_family.clone(),
        );
        let status_bar_bg = theme.sidebar;

        let note = match self.active_editor() {
            Some(editor) => editor.clone().into_any_element(),
            None => self.render_empty_state(window, cx).into_any_element(),
        };
        let main = div().flex_1().min_w_0().h_full().bg(background).child(note);

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
                    .child(main)
                    .when(self.right.open, |this| {
                        this.child(self.render_sidebar(PanelPlacement::Right, cx))
                    }),
            )
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
