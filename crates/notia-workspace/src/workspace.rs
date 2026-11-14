use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::Duration,
};

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _, ThemeMode,
    TitleBar, WindowExt as _,
    button::{Button, ButtonVariants as _},
    command::CommandState,
    h_flex,
    kbd::Kbd,
    resizable::{HandleEdge, resize_handle},
    status_bar::StatusBar,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use notia_plugin::{
    ActiveNote, AppSettings, Notia, OpenOptions, Panel, PanelPlacement, PluginHost, VaultEvent,
    VaultModel, ViewMode, WorkspaceModel,
    actions::{NewNote, OpenQuickSwitcher, OpenSettings, WORKSPACE_CONTEXT},
    core::VaultPath,
    theme::{ColorScheme, ThemeCatalog},
};
use serde::{Deserialize, Serialize};

use crate::note_editor::{NoteEditor, NoteEditorEvent};

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
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct Layout {
    left: SidebarLayout,
    right: SidebarLayout,
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
}

impl Sidebar {
    fn from_layout(layout: &SidebarLayout) -> Self {
        let active: Option<SharedString> = layout.active.clone().map(Into::into);
        Self {
            open: layout.open,
            width: clamp_width(px(layout.width)),
            chosen: active.clone(),
            active,
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
        }
    }
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
    pub(crate) palette: Entity<CommandState>,
    pub(crate) palette_open: bool,
    pub(crate) switcher: Entity<CommandState>,
    pub(crate) switcher_open: bool,
    pub(crate) switcher_matches: Vec<crate::palette::SwitcherMatch>,
    layout_path: PathBuf,
    save_layout: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
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
            palette: cx.new(|cx| CommandState::new(window, cx)),
            palette_open: false,
            switcher: cx.new(|cx| CommandState::new(window, cx)),
            switcher_open: false,
            switcher_matches: Vec::new(),
            layout_path,
            save_layout: None,
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
        workspace
    }

    /// Builds the views of newly enabled panels and status items, and drops
    /// the ones whose plugin was disabled.
    fn sync_contributions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let host = PluginHost::global(cx);
        let panels = host.panels();
        let status_items = host.status_items();

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

        for placement in [PanelPlacement::Left, PanelPlacement::Right] {
            let exists = |id: &SharedString| {
                panels
                    .iter()
                    .any(|panel| panel.id() == id && panel.placement() == placement)
            };
            let first = panels
                .iter()
                .find(|panel| panel.placement() == placement)
                .map(|panel| panel.id().clone());
            let sidebar = self.sidebar_mut(placement);
            sidebar.active = sidebar
                .chosen
                .clone()
                .filter(exists)
                .or_else(|| sidebar.active.clone().filter(exists))
                .or(first);
        }
        self.panels = panels;
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
        if !path.is_markdown() {
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
                        self.tabs[active]
                            .editor
                            .update(cx, |editor, cx| editor.save(cx));
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
        let folder = folder
            .unwrap_or_else(|| VaultPath::new(&AppSettings::global(cx).files.new_note_folder));
        match self.vault.update(cx, |vault, cx| {
            vault.create_note(&folder, "Untitled", "", cx)
        }) {
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
        self.publish_active_note(cx);
        self.schedule_layout_save(cx);
        cx.notify();
    }

    pub(crate) fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(ix);
        tab.editor.update(cx, |editor, cx| editor.save(cx));
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

    pub(crate) fn insert_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor().cloned() {
            editor.update(cx, |editor, cx| editor.insert_text(text, window, cx));
        }
    }

    pub(crate) fn toggle_sidebar(&mut self, placement: PanelPlacement, cx: &mut Context<Self>) {
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
        self.schedule_layout_save(cx);
        cx.notify();
    }

    pub(crate) fn show_panel(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(panel) = self.panels.iter().find(|panel| panel.id().as_ref() == id) else {
            return;
        };
        // Editor toolbars are already above the note.
        if panel.placement() == PanelPlacement::Editor {
            return;
        }
        let (placement, id) = (panel.placement(), panel.id().clone());
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

    fn render_panel_switcher(
        &self,
        placement: PanelPlacement,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.sidebar(placement).active.clone();
        h_flex().gap_0p5().items_center().children(
            self.panels
                .iter()
                .filter(|panel| panel.placement() == placement)
                .map(|panel| {
                    let id = panel.id().clone();
                    Button::new(SharedString::from(format!("panel-{id}")))
                        .icon(panel.icon().clone())
                        .ghost()
                        .small()
                        .selected(active.as_ref() == Some(&id))
                        .tooltip(panel.title().clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_panel(placement, id.clone(), cx);
                        }))
                }),
        )
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
            .when(!self.right.open, |this| {
                this.child(title_control(self.sidebar_toggle(PanelPlacement::Right, cx)).pr_2())
            })
    }

    fn render_ribbon(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let is_dark = theme.is_dark();
        let (sidebar_bg, sidebar_border) = (theme.sidebar, theme.sidebar_border);
        let items = PluginHost::global(cx).ribbon_items();
        let switchable = ThemeCatalog::global(cx)
            .active_pack()
            .is_none_or(|pack| pack.has_mode(ThemeMode::Light) && pack.has_mode(ThemeMode::Dark));
        let scheme_tooltip = match (switchable, is_dark) {
            (false, _) => "This theme has a single appearance",
            (true, true) => "Use light mode",
            (true, false) => "Use dark mode",
        };

        v_flex()
            .flex_none()
            .w(RIBBON_WIDTH)
            .h_full()
            .items_center()
            .py_2()
            .gap_1()
            .bg(sidebar_bg)
            .border_r_1()
            .border_color(sidebar_border)
            .children(items.into_iter().map(|item| {
                Button::new(SharedString::from(format!("ribbon-{}", item.id())))
                    .icon(item.icon().clone())
                    .ghost()
                    .tooltip(item.tooltip().clone())
                    .on_click(move |_, window, cx| item.run(window, cx))
            }))
            .child(div().flex_1())
            .child(
                Button::new("ribbon-scheme")
                    .icon(if is_dark {
                        IconName::Sun
                    } else {
                        IconName::Moon
                    })
                    .ghost()
                    .disabled(!switchable)
                    .tooltip(scheme_tooltip)
                    .on_click(move |_, _, cx| {
                        ThemeCatalog::update_appearance(cx, |appearance| {
                            appearance.scheme = if is_dark {
                                ColorScheme::Light
                            } else {
                                ColorScheme::Dark
                            };
                        });
                    }),
            )
            .child(
                Button::new("ribbon-settings")
                    .icon(IconName::Settings)
                    .ghost()
                    .tooltip("Settings")
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenSettings), cx)),
            )
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
            .child(v_flex().size_full().overflow_hidden().children(view))
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

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (background, foreground, font_family) = (
            theme.background,
            theme.foreground,
            theme.font_family.clone(),
        );
        let (status_bar_bg, border) = (theme.sidebar, theme.border);

        let editor_bars: Vec<_> = self
            .panels
            .iter()
            .filter(|panel| panel.placement() == PanelPlacement::Editor)
            .filter_map(|panel| self.panel_views.get(panel.id()).map(|(_, view)| view.clone()))
            .collect();
        let note = match self.active_editor() {
            Some(editor) => editor.clone().into_any_element(),
            None => self.render_empty_state(window, cx).into_any_element(),
        };
        let main = v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .bg(background)
            .children(editor_bars.into_iter().map(|view| {
                div()
                    .w_full()
                    .flex_none()
                    .border_b_1()
                    .border_color(border)
                    .child(view)
            }))
            .child(div().flex_1().min_h_0().w_full().child(note));

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
