//! Desktop rendering of the workspace: title row, panes, and sidebars.

use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex,
    kbd::Kbd,
    menu::{ContextMenuExt as _, PopupMenu},
    resizable::{HandleEdge, resize_handle},
    status_bar::StatusBar,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use notia_plugin::{
    PanelPlacement,
    actions::{NewNote, OpenQuickSwitcher, OpenSettings, WORKSPACE_CONTEXT},
    note_tooltip,
};

use super::Workspace;
use super::layout::{SidebarResize, SplitAxis, clamp_width, weighted_pane};
use super::nav;
use super::tabs::{EditorPane, EditorSplitDrag, OpenMenu, TabDrag, TabGhost, tab_context_menu};
use super::{RIBBON_WIDTH, TRAFFIC_LIGHTS_WIDTH};

impl Workspace {
    pub(super) fn render_title_row(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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

    pub(super) fn sidebar_toggle(
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

    pub(super) fn panel_placeholder(&self, placement: PanelPlacement, cx: &App) -> Div {
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

    pub(super) fn render_panel_switcher(
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

    pub(super) fn note_tabs(&self, pane: EditorPane, cx: &mut Context<Self>) -> Vec<Tab> {
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
    pub(super) fn tab_end_mark(&self, pane: EditorPane, cx: &App) -> Option<Div> {
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

    pub(super) fn render_tab_strip(
        &self,
        traffic_lights: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
    pub(super) fn render_title_tabs(
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

    pub(super) fn render_sidebar(
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

    pub(super) fn render_empty_state(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
    pub(super) fn open_tab_menu(
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

    pub(super) fn render_tab_menu(&self, window: &Window) -> Option<AnyElement> {
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

    pub(super) fn render_editor_area(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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

    pub(super) fn render_pane(
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

    pub(super) fn render_split_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

    pub(super) fn render_desktop(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
