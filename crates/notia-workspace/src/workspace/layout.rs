//! Saved window layout, sidebar state, and the widths persisted with them.

use std::time::Duration;

use gpui_kit::{prelude::FluentBuilder as _, *};
use notia_plugin::{PanelPlacement, core::VaultPath};
use serde::{Deserialize, Serialize};

use super::Workspace;
use super::nav;
use super::phone::PhoneScreen;
use super::tabs::OpenTab;

const SIDEBAR_WIDTHS: (Pixels, Pixels) = (px(180.), px(560.));

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct SidebarLayout {
    pub(super) open: bool,
    pub(super) width: f32,
    pub(super) active: Option<String>,
    #[serde(default)]
    pub(super) order: Vec<String>,
    #[serde(default)]
    pub(super) hidden: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Layout {
    pub(super) left: SidebarLayout,
    pub(super) right: SidebarLayout,
    #[serde(default)]
    pub(super) ribbon: nav::RibbonLayout,
    pub(super) tabs: Vec<VaultPath>,
    pub(super) active_tab: Option<usize>,
    /// A second group of tabs beside or below the first. Absent when the
    /// editor is a single pane.
    #[serde(default)]
    pub(super) split: Option<SavedSplit>,
    pub(super) recent: Vec<VaultPath>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SplitAxis {
    Right,
    Down,
}

#[derive(Serialize, Deserialize)]
pub(super) struct SavedSplit {
    pub(super) axis: SplitAxis,
    pub(super) ratio: f32,
    pub(super) tabs: Vec<VaultPath>,
    pub(super) active_tab: Option<usize>,
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

pub(super) struct SplitGroup {
    pub(super) axis: SplitAxis,
    /// Share of the editor area given to the main pane, between 0.2 and 0.8.
    pub(super) ratio: f32,
    pub(super) tabs: Vec<OpenTab>,
    pub(super) active: Option<usize>,
    pub(super) scroll: ScrollHandle,
}

pub(crate) struct Sidebar {
    pub(crate) open: bool,
    pub(super) width: Pixels,
    pub(crate) active: Option<SharedString>,
    /// The panel the user picked last. It may not be registered yet, since
    /// community plugins load after the window opens, or no longer, after
    /// its plugin was disabled; the sidebar shows it whenever it exists.
    chosen: Option<SharedString>,
    pub(super) order: Vec<String>,
    pub(super) hidden: Vec<String>,
}

impl Sidebar {
    pub(super) fn from_layout(layout: &SidebarLayout) -> Self {
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

/// The drag payload of a sidebar's resize handle.
#[derive(Clone)]
pub(super) struct SidebarResize(pub(super) PanelPlacement);

impl Render for SidebarResize {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl Workspace {
    pub(super) fn sidebar(&self, placement: PanelPlacement) -> &Sidebar {
        match placement {
            PanelPlacement::Left => &self.left,
            PanelPlacement::Right => &self.right,
            PanelPlacement::Editor => panic!("an editor toolbar is not a sidebar"),
        }
    }

    pub(super) fn sidebar_mut(&mut self, placement: PanelPlacement) -> &mut Sidebar {
        match placement {
            PanelPlacement::Left => &mut self.left,
            PanelPlacement::Right => &mut self.right,
            PanelPlacement::Editor => panic!("an editor toolbar is not a sidebar"),
        }
    }

    pub(super) fn restore_split(
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
            if let Some(tab) = self.tab_for_saved_path(path, window, cx) {
                tabs.push(tab);
            }
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

    pub(super) fn select_panel(
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

    pub(super) fn schedule_layout_save(&mut self, cx: &mut Context<Self>) {
        self.save_layout = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            _ = this.update(cx, |this, cx| this.save_layout_now(cx));
        }));
    }

    pub(super) fn save_layout_now(&self, cx: &App) {
        let layout = Layout {
            left: self.left.to_layout(),
            right: self.right.to_layout(),
            ribbon: self.ribbon.clone(),
            tabs: self
                .tabs
                .iter()
                .map(|tab| tab.document.path(cx).clone())
                .collect(),
            active_tab: self.active_tab,
            split: self.split.as_ref().map(|split| SavedSplit {
                axis: split.axis,
                ratio: split.ratio,
                tabs: split
                    .tabs
                    .iter()
                    .map(|tab| tab.document.path(cx).clone())
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

    pub(super) fn refresh_panel(&mut self, placement: PanelPlacement) {
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

    pub(super) fn panel_ids(&self, placement: PanelPlacement) -> Vec<String> {
        self.panels
            .iter()
            .filter(|panel| panel.placement() == placement)
            .map(|panel| panel.id().to_string())
            .collect()
    }
}

pub(super) fn clamp_width(width: Pixels) -> Pixels {
    px(width
        .as_f32()
        .clamp(SIDEBAR_WIDTHS.0.as_f32(), SIDEBAR_WIDTHS.1.as_f32()))
}

pub(super) fn weighted_pane(ratio: f32, horizontal: bool, child: impl IntoElement) -> Div {
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
