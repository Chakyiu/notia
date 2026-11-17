//! Order and visibility for the ribbon and the sidebar icon rows.
//!
//! A saved list remembers where the user dropped each icon, including icons
//! whose plugin is currently off. Icons the user has not placed yet keep
//! their default spot: plugin buttons along the top of the ribbon, theme
//! and settings along the bottom, panels in the order their plugins set.

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, Side, Sizable as _, Size,
    menu::{PopupMenu, PopupMenuItem},
    tooltip::Tooltip,
};
use gpui_kit::{
    App, AppContext as _, ClickEvent, Context, DragMoveEvent, ElementId, Hsla,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement, Render, Role, SharedString,
    StatefulInteractiveElement as _, Styled, Window, div, prelude::FluentBuilder as _, px,
};
use serde::{Deserialize, Serialize};

pub(crate) const THEME_ID: &str = "notia:theme";
pub(crate) const SETTINGS_ID: &str = "notia:settings";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct RibbonLayout {
    pub leading: Vec<String>,
    pub trailing: Vec<String>,
    pub hidden: Vec<String>,
}

/// Which strip an icon belongs to. Ribbon icons can move between the top
/// and bottom groups; a sidebar icon stays in its sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NavBar {
    RibbonTop,
    RibbonBottom,
    Left,
    Right,
}

impl NavBar {
    pub(crate) fn tag(self) -> &'static str {
        match self {
            Self::RibbonTop => "ribbon-top",
            Self::RibbonBottom => "ribbon-bottom",
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    pub(crate) fn horizontal(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

/// Where a dragged icon lands in its strip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    Before(String),
    After(String),
    /// After the last icon currently shown in the group.
    End,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NavDrop {
    pub bar: NavBar,
    pub slot: Slot,
}

#[derive(Clone)]
pub(crate) struct NavDrag {
    pub bar: NavBar,
    pub id: SharedString,
    pub icon: Icon,
}

#[derive(Clone)]
pub(crate) struct MenuRow {
    pub id: SharedString,
    pub label: SharedString,
    pub icon: Icon,
    pub visible: bool,
}

pub(crate) struct IconSpec {
    pub element_id: ElementId,
    pub icon: Icon,
    pub tooltip: SharedString,
    pub selected: bool,
    pub disabled: bool,
    pub compact: bool,
    pub dimmed: bool,
    pub horizontal: bool,
    pub mark_after: Option<bool>,
    pub drag: NavDrag,
}

pub(crate) fn same_menu(source: NavBar, target: NavBar) -> bool {
    matches!(
        (source, target),
        (
            NavBar::RibbonTop | NavBar::RibbonBottom,
            NavBar::RibbonTop | NavBar::RibbonBottom
        ) | (NavBar::Left, NavBar::Left)
            | (NavBar::Right, NavBar::Right)
    )
}

pub(crate) fn toggle_hidden(hidden: &mut Vec<String>, id: &str) {
    if let Some(index) = hidden.iter().position(|item| item == id) {
        hidden.remove(index);
    } else {
        hidden.push(id.to_string());
    }
}

pub(crate) fn unhide(hidden: &mut Vec<String>, id: &str) {
    hidden.retain(|item| item != id);
}

/// Icons to draw, in order. Hidden ids and ids from plugins that are off
/// stay out of the result.
pub(crate) fn visible_ids(
    available: &[String],
    order: &[String],
    hidden: &[String],
) -> Vec<String> {
    shown(&placed_linear(available, order), available, hidden)
}

pub(crate) fn ribbon_zones(
    available: &[String],
    layout: &RibbonLayout,
) -> (Vec<String>, Vec<String>) {
    let (leading, trailing) = placed_ribbon(available, layout);
    (
        shown(&leading, available, &layout.hidden),
        shown(&trailing, available, &layout.hidden),
    )
}

pub(crate) fn move_linear(
    order: &mut Vec<String>,
    available: &[String],
    hidden: &[String],
    id: &str,
    slot: &Slot,
) -> bool {
    let mut next = placed_linear(available, order);
    let before = next.clone();
    insert_slot(&mut next, id, slot, hidden);
    if next == before {
        return false;
    }
    *order = next;
    true
}

pub(crate) fn move_ribbon(
    layout: &mut RibbonLayout,
    available: &[String],
    id: &str,
    bar: NavBar,
    slot: &Slot,
) -> bool {
    let (mut leading, mut trailing) = placed_ribbon(available, layout);
    let before = (leading.clone(), trailing.clone());
    leading.retain(|item| item != id);
    trailing.retain(|item| item != id);
    let list = if bar == NavBar::RibbonBottom {
        &mut trailing
    } else {
        &mut leading
    };
    insert_slot(list, id, slot, &layout.hidden);
    if leading == before.0 && trailing == before.1 {
        return false;
    }
    layout.leading = leading;
    layout.trailing = trailing;
    true
}

/// The right-click menu: a check for every icon, shown ones first.
pub(crate) fn visibility_menu(
    menu: PopupMenu,
    rows: Vec<MenuRow>,
    toggle: Rc<dyn Fn(&str, &mut App)>,
) -> PopupMenu {
    if rows.is_empty() {
        return menu;
    }
    let mut menu = menu
        .check_side(Side::Right)
        .scrollable(true)
        .item(PopupMenuItem::label("Show"));
    let mut separated = false;
    let mut any_visible = false;
    for row in rows {
        if !row.visible && any_visible && !separated {
            menu = menu.separator();
            separated = true;
        }
        any_visible |= row.visible;
        let id = row.id;
        let toggle = toggle.clone();
        menu = menu.item(
            PopupMenuItem::new(row.label)
                .icon(row.icon)
                .checked(row.visible)
                .on_click(move |_, _, cx| toggle(id.as_ref(), cx)),
        );
    }
    menu
}

pub(crate) fn icon_button(
    spec: IconSpec,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
    on_start: impl Fn(&NavDrag, &mut App) + 'static,
    on_move: impl Fn(&DragMoveEvent<NavDrag>, &mut Window, &mut App) + 'static,
    on_drop: impl Fn(&NavDrag, &mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let IconSpec {
        element_id,
        icon,
        tooltip,
        selected,
        disabled,
        compact,
        dimmed,
        horizontal,
        mark_after,
        drag,
    } = spec;
    let theme = cx.theme();
    let side = if compact { px(24.) } else { px(32.) };
    let glyph = icon.with_size(if compact { Size::Small } else { Size::Medium });
    let fg = if disabled {
        theme.muted_foreground
    } else {
        theme.secondary_foreground
    };
    let hover_fg = theme.accent_foreground;
    let selected_bg = theme.tokens.secondary_active.background;
    let active_bg = theme.tokens.button_active.background;
    let mut hover_bg = theme.tokens.accent.background;
    if theme.is_dark() {
        hover_bg = hover_bg.opacity(0.5);
    }
    let mark = theme.drag_border;
    let radius = theme.radius;
    let focus = theme.primary;

    div()
        .id(element_id)
        .relative()
        .flex_none()
        .size(side)
        .rounded(radius)
        .flex()
        .items_center()
        .justify_center()
        .text_color(fg)
        .cursor_grab()
        .role(Role::Button)
        .aria_label(tooltip.clone())
        .tab_index(0)
        .tab_stop(!disabled)
        .focus(move |style| style.border_1().border_color(focus))
        .when(dimmed, |this| this.opacity(0.35))
        .when(selected, move |this| this.bg(selected_bg))
        .when(!disabled && !selected, move |this| {
            this.hover(move |style| style.bg(hover_bg).text_color(hover_fg))
                .active(move |style| style.bg(active_bg))
        })
        .child(glyph)
        .when_some(mark_after, move |this, after| {
            this.child(drop_mark(after, horizontal, mark))
        })
        .tooltip({
            let tooltip = tooltip.clone();
            move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx)
        })
        .on_mouse_down(MouseButton::Left, |_, window, _| {
            window.prevent_default();
        })
        .on_click(move |_: &ClickEvent, window, cx| {
            if disabled || cx.has_active_drag() {
                return;
            }
            on_click(window, cx);
        })
        .on_drag(drag, move |drag, _, _, cx| {
            on_start(drag, cx);
            cx.new(|_| NavGhost {
                icon: drag.icon.clone(),
            })
        })
        .on_drag_move(on_move)
        .on_drop(on_drop)
}

fn drop_mark(after: bool, horizontal: bool, color: Hsla) -> gpui_kit::Div {
    div()
        .absolute()
        .bg(color)
        .when(horizontal, |bar| {
            bar.top(px(3.))
                .bottom(px(3.))
                .w(px(2.))
                .when(after, |bar| bar.right(px(0.)))
                .when(!after, |bar| bar.left(px(0.)))
        })
        .when(!horizontal, |bar| {
            bar.left(px(6.))
                .right(px(6.))
                .h(px(2.))
                .when(after, |bar| bar.bottom(px(0.)))
                .when(!after, |bar| bar.top(px(0.)))
        })
}

struct NavGhost {
    icon: Icon,
}

impl Render for NavGhost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .size(px(32.))
            .rounded(theme.radius)
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.popover)
            .border_1()
            .border_color(theme.border)
            .shadow_sm()
            .text_color(theme.foreground)
            .child(self.icon.clone())
    }
}

fn contains(ids: &[String], id: &str) -> bool {
    ids.iter().any(|item| item == id)
}

fn dedupe(ids: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for id in ids {
        if !contains(&out, &id) {
            out.push(id);
        }
    }
    out
}

fn placed_linear(available: &[String], order: &[String]) -> Vec<String> {
    let mut out = dedupe(order.to_vec());
    for id in available {
        if !contains(&out, id) {
            out.push(id.clone());
        }
    }
    out
}

fn placed_ribbon(available: &[String], layout: &RibbonLayout) -> (Vec<String>, Vec<String>) {
    let mut leading = dedupe(layout.leading.clone());
    let mut trailing = dedupe(layout.trailing.clone());
    trailing.retain(|id| !contains(&leading, id));
    for id in available {
        if contains(&leading, id) || contains(&trailing, id) {
            continue;
        }
        if id == THEME_ID || id == SETTINGS_ID {
            trailing.push(id.clone());
        } else {
            leading.push(id.clone());
        }
    }
    (leading, trailing)
}

fn shown(ids: &[String], available: &[String], hidden: &[String]) -> Vec<String> {
    ids.iter()
        .filter(|id| contains(available, id) && !contains(hidden, id))
        .cloned()
        .collect()
}

fn insert_slot(ids: &mut Vec<String>, id: &str, slot: &Slot, hidden: &[String]) {
    ids.retain(|item| item != id);
    let index = match slot {
        Slot::End => ids
            .iter()
            .rposition(|item| !contains(hidden, item))
            .map(|index| index + 1)
            .unwrap_or(0),
        Slot::Before(target) => ids
            .iter()
            .position(|item| item == target)
            .unwrap_or(ids.len()),
        Slot::After(target) => ids
            .iter()
            .position(|item| item == target)
            .map(|index| index + 1)
            .unwrap_or(ids.len()),
    };
    ids.insert(index.min(ids.len()), id.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    fn sample() -> Vec<String> {
        ids(&["sync:sync", "notes:calendar", THEME_ID, SETTINGS_ID])
    }

    #[test]
    fn ribbon_defaults_keep_theme_and_settings_at_the_bottom() {
        let (top, bottom) = ribbon_zones(&sample(), &RibbonLayout::default());
        assert_eq!(top, ids(&["sync:sync", "notes:calendar"]));
        assert_eq!(bottom, ids(&[THEME_ID, SETTINGS_ID]));
    }

    #[test]
    fn hidden_icons_stay_out_of_the_strip_until_shown_again() {
        let mut layout = RibbonLayout::default();
        toggle_hidden(&mut layout.hidden, THEME_ID);
        let (top, bottom) = ribbon_zones(&sample(), &layout);
        assert_eq!(top, ids(&["sync:sync", "notes:calendar"]));
        assert_eq!(bottom, ids(&[SETTINGS_ID]));

        toggle_hidden(&mut layout.hidden, THEME_ID);
        let (_, bottom) = ribbon_zones(&sample(), &layout);
        assert_eq!(bottom, ids(&[THEME_ID, SETTINGS_ID]));
    }

    #[test]
    fn dragging_settings_up_joins_the_top_group() {
        let mut layout = RibbonLayout::default();
        let moved = move_ribbon(
            &mut layout,
            &sample(),
            SETTINGS_ID,
            NavBar::RibbonTop,
            &Slot::Before("sync:sync".into()),
        );
        assert!(moved);
        let (top, bottom) = ribbon_zones(&sample(), &layout);
        assert_eq!(top, ids(&[SETTINGS_ID, "sync:sync", "notes:calendar"]));
        assert_eq!(bottom, ids(&[THEME_ID]));
    }

    #[test]
    fn dragging_a_plugin_onto_the_bottom_group_pins_it_there() {
        let mut layout = RibbonLayout::default();
        move_ribbon(
            &mut layout,
            &sample(),
            "notes:calendar",
            NavBar::RibbonBottom,
            &Slot::Before(SETTINGS_ID.into()),
        );
        let (top, bottom) = ribbon_zones(&sample(), &layout);
        assert_eq!(top, ids(&["sync:sync"]));
        assert_eq!(bottom, ids(&[THEME_ID, "notes:calendar", SETTINGS_ID]));
    }

    #[test]
    fn dropping_on_the_gap_appends_to_the_top_group() {
        let mut layout = RibbonLayout::default();
        move_ribbon(
            &mut layout,
            &sample(),
            SETTINGS_ID,
            NavBar::RibbonTop,
            &Slot::End,
        );
        let (top, bottom) = ribbon_zones(&sample(), &layout);
        assert_eq!(top, ids(&["sync:sync", "notes:calendar", SETTINGS_ID]));
        assert_eq!(bottom, ids(&[THEME_ID]));
    }

    #[test]
    fn a_new_plugin_appends_to_the_top_without_forgetting_a_disabled_one() {
        let mut layout = RibbonLayout {
            leading: ids(&["gone:plugin", "sync:sync"]),
            ..RibbonLayout::default()
        };
        move_ribbon(
            &mut layout,
            &sample(),
            "notes:calendar",
            NavBar::RibbonTop,
            &Slot::Before("sync:sync".into()),
        );
        assert_eq!(
            layout.leading,
            ids(&["gone:plugin", "notes:calendar", "sync:sync"])
        );
        let (top, _) = ribbon_zones(&sample(), &layout);
        assert_eq!(top, ids(&["notes:calendar", "sync:sync"]));
    }

    #[test]
    fn leaving_an_icon_where_it_is_does_not_rewrite_the_layout() {
        let mut layout = RibbonLayout::default();
        let moved = move_ribbon(
            &mut layout,
            &sample(),
            "sync:sync",
            NavBar::RibbonTop,
            &Slot::Before("notes:calendar".into()),
        );
        assert!(!moved);
        assert_eq!(layout, RibbonLayout::default());
    }

    #[test]
    fn panels_follow_the_saved_order_and_skip_hidden_ones() {
        let available = ids(&["files", "search", "outline"]);
        assert_eq!(
            visible_ids(&available, &[], &[]),
            ids(&["files", "search", "outline"])
        );

        let mut order = ids(&["search", "files"]);
        assert_eq!(
            visible_ids(&available, &order, &ids(&["files"])),
            ids(&["search", "outline"])
        );

        move_linear(
            &mut order,
            &available,
            &ids(&["files"]),
            "outline",
            &Slot::Before("search".into()),
        );
        assert_eq!(order, ids(&["outline", "search", "files"]));
        assert_eq!(
            visible_ids(&available, &order, &[]),
            ids(&["outline", "search", "files"])
        );
    }

    #[test]
    fn an_old_workspace_file_without_ribbon_fields_still_parses() {
        let layout: RibbonLayout = serde_json::from_str("{}").unwrap();
        assert!(layout.leading.is_empty());
        assert!(layout.trailing.is_empty());
        assert!(layout.hidden.is_empty());
    }

    #[test]
    fn ribbon_icons_share_a_menu_and_sidebar_icons_do_not() {
        assert!(same_menu(NavBar::RibbonTop, NavBar::RibbonBottom));
        assert!(!same_menu(NavBar::RibbonTop, NavBar::Left));
        assert!(!same_menu(NavBar::Left, NavBar::Right));
    }
}
