use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use gpui_kit::component::command::CommandState;
use gpui_kit::*;
use notia_plugin::{
    ActiveNote, Notia, Panel, PanelPlacement, PluginHost, VaultEvent, VaultModel, WorkspaceModel,
    core::VaultPath,
    theme::{ColorScheme, ThemeCatalog},
};

use crate::note_editor::NoteEditor;

#[path = "phone.rs"]
mod phone;

#[path = "nav.rs"]
mod nav;

mod layout;
mod render;
mod ribbon;
mod tabs;

use layout::{Layout, Sidebar, SplitGroup};
use phone::PhoneScreen;
use tabs::{EditorPane, OpenMenu, OpenTab, TabSlot};

/// The strip of plugin buttons along the left edge.
const RIBBON_WIDTH: Pixels = px(44.);
/// Room for the macOS window buttons drawn over the title row.
const TRAFFIC_LIGHTS_WIDTH: Pixels = px(76.);

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
    pub(super) fn sync_contributions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn editor_toolbar_views(&self) -> Vec<AnyView> {
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
    pub(super) fn attach_toolbars(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn persist(&mut self, cx: &mut Context<Self>) {
        self.save_all(cx);
        // Drop the delayed write. It would otherwise land after the vault
        // has changed and record this window against the new folder.
        self.save_layout.take();
        self.save_layout_now(cx);
    }

    pub(super) fn save_all(&mut self, cx: &mut Context<Self>) {
        for editor in self.all_editors() {
            editor.update(cx, |editor, cx| editor.save(cx));
        }
    }

    pub(super) fn publish_active_note(&mut self, cx: &mut Context<Self>) {
        let note = self.active_editor().map(|editor| {
            let editor = editor.read(cx);
            ActiveNote::new(editor.path().clone(), editor.text(cx), editor.mode())
                .with_cursor_line(editor.cursor_line(cx))
        });
        self.model
            .update(cx, |model, cx| model.set_active_note(note, cx));
    }

    pub(super) fn on_vault_event(
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
}

#[cfg(test)]
mod layout_tests {
    use super::layout::{Layout, SplitAxis};
    use super::tabs::same_pane_landing;

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
        assert_eq!(split.axis, SplitAxis::Down);
        assert!((split.ratio - 0.4).abs() < f32::EPSILON);
        assert_eq!(split.tabs.len(), 1);
        assert_eq!(split.active_tab, Some(0));
    }

    #[test]
    fn dragging_a_tab_along_its_row_lands_where_the_gap_was() {
        // [A B C D], drag A to before C: the gap is index 2, and A lands at 1.
        assert_eq!(same_pane_landing(0, 2), Some(1));
        // Drag A to the end.
        assert_eq!(same_pane_landing(0, 4), Some(3));
        // Drag D to the front.
        assert_eq!(same_pane_landing(3, 0), Some(0));
        // Drag B to just before D.
        assert_eq!(same_pane_landing(1, 3), Some(2));
    }

    #[test]
    fn dropping_a_tab_on_itself_does_not_move_it() {
        assert_eq!(same_pane_landing(1, 1), None);
        assert_eq!(same_pane_landing(1, 2), None);
    }
}
