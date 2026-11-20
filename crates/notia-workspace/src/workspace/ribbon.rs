//! The ribbon of plugin icons and the menus that reorder navigation.

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, ThemeMode, menu::ContextMenuExt as _, v_flex,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use notia_plugin::{
    Panel, PanelPlacement, PluginHost, RibbonItem,
    actions::OpenSettings,
    theme::{ColorScheme, ThemeCatalog},
};

use super::RIBBON_WIDTH;
use super::Workspace;
use super::nav;

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

impl Workspace {
    pub(super) fn move_panel(
        &mut self,
        placement: PanelPlacement,
        id: &str,
        slot: &nav::Slot,
    ) -> bool {
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

    pub(super) fn apply_nav_move(&mut self, id: &str, target: &nav::NavDrop, cx: &App) -> bool {
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

    pub(super) fn hover_nav(
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

    pub(super) fn clear_nav_drop(&mut self, cx: &mut Context<Self>) {
        if self.nav_drop.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn drop_nav(
        &mut self,
        drag: &nav::NavDrag,
        expected: nav::NavBar,
        cx: &mut Context<Self>,
    ) {
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

    pub(super) fn toggle_ribbon(&mut self, id: &str, cx: &mut Context<Self>) {
        nav::toggle_hidden(&mut self.ribbon.hidden, id);
        self.schedule_layout_save(cx);
        cx.notify();
    }

    pub(super) fn toggle_panel(
        &mut self,
        placement: PanelPlacement,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        nav::toggle_hidden(&mut self.sidebar_mut(placement).hidden, id);
        self.refresh_panel(placement);
        self.schedule_layout_save(cx);
        cx.notify();
    }

    pub(super) fn nav_button(
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

    pub(super) fn panel_rows(&self, placement: PanelPlacement) -> Vec<nav::MenuRow> {
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

    pub(super) fn render_ribbon(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
    pub(super) fn ribbon_gap(
        &self,
        marked: bool,
        color: Hsla,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
