//! The phone window: one page at a time, with the system status bar and
//! keyboard left clear. The desktop window keeps its sidebars.

use gpui_kit::component::{
    button::{Button, ButtonVariants as _},
    h_flex,
    scroll::ScrollableElement as _,
    v_flex, ActiveTheme as _, IconName, Selectable as _, Sizable as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use notia_plugin::{
    actions::{NewNote, OpenSettings, WORKSPACE_CONTEXT},
    PanelPlacement,
};

use super::Workspace;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PhoneScreen {
    Browse,
    Note,
    Inspect,
}

const BAR: Pixels = px(48.);

impl Workspace {
    pub(super) fn render_phone(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let insets = window.insets();
        let keyboard = insets.ime.bottom > px(0.);
        let bottom = if keyboard {
            insets.ime.bottom
        } else {
            insets.safe_area.bottom
        };
        let (background, foreground, font_family) = (
            theme.background,
            theme.foreground,
            theme.font_family.clone(),
        );

        let body = match self.phone {
            PhoneScreen::Browse => self
                .render_phone_panel(PanelPlacement::Left, cx)
                .into_any_element(),
            PhoneScreen::Inspect => self
                .render_phone_panel(PanelPlacement::Right, cx)
                .into_any_element(),
            PhoneScreen::Note => self.render_phone_note(window, cx).into_any_element(),
        };

        v_flex()
            .id("workspace")
            .key_context(WORKSPACE_CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_hidden()
            .bg(background)
            .text_color(foreground)
            .font_family(font_family)
            .pt(insets.safe_area.top)
            .pl(insets.safe_area.left)
            .pr(insets.safe_area.right)
            .pb(bottom)
            .child(self.render_phone_header(cx))
            .when(
                self.phone == PhoneScreen::Note && self.tabs.len() > 1,
                |this| this.child(self.render_phone_tabs(cx)),
            )
            .when(self.phone != PhoneScreen::Note, |this| {
                this.child(self.render_phone_switcher(cx))
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_hidden()
                    .child(body),
            )
            .when(self.phone == PhoneScreen::Browse && !keyboard, |this| {
                this.child(crate::vaults::footer(cx))
            })
            .when(!keyboard, |this| this.child(self.render_phone_bar(cx)))
    }

    fn render_phone_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (sidebar, border) = (theme.sidebar, theme.sidebar_border);
        let title = self.phone_title(cx);
        h_flex()
            .flex_none()
            .w_full()
            .h(BAR)
            .px_2()
            .gap_1()
            .items_center()
            .bg(sidebar)
            .border_b_1()
            .border_color(border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px_2()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .truncate()
                    .child(title),
            )
            .child(
                Button::new("phone-search")
                    .icon(IconName::Search)
                    .ghost()
                    .small()
                    .tooltip("Search")
                    .on_click(cx.listener(|this, _, _, cx| this.show_panel("search:search", cx))),
            )
            .child(
                Button::new("phone-new")
                    .icon(IconName::Plus)
                    .ghost()
                    .small()
                    .tooltip("New note")
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(NewNote), cx)),
            )
            .child(
                Button::new("phone-settings")
                    .icon(IconName::Settings)
                    .ghost()
                    .small()
                    .tooltip("Settings")
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenSettings), cx)),
            )
    }

    fn phone_title(&self, cx: &App) -> SharedString {
        let placement = match self.phone {
            PhoneScreen::Note => {
                return self
                    .active_editor()
                    .map(|editor| editor.read(cx).title())
                    .unwrap_or_else(|| "Notia".into());
            }
            PhoneScreen::Browse => PanelPlacement::Left,
            PhoneScreen::Inspect => PanelPlacement::Right,
        };
        self.sidebar(placement)
            .active
            .as_ref()
            .and_then(|id| {
                self.panels
                    .iter()
                    .find(|panel| panel.id() == id)
                    .map(|panel| panel.title().clone())
            })
            .unwrap_or_else(|| match placement {
                PanelPlacement::Left => "Files".into(),
                _ => "Outline".into(),
            })
    }

    fn render_phone_switcher(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let placement = match self.phone {
            PhoneScreen::Inspect => PanelPlacement::Right,
            _ => PanelPlacement::Left,
        };
        let theme = cx.theme();
        let (sidebar, border) = (theme.sidebar, theme.sidebar_border);
        // Fixed height: the horizontal scroller's wrapper is height 100%
        // unless this row sets one, which covers the file list and tab bar.
        h_flex()
            .flex_none()
            .w_full()
            .h(px(40.))
            .px_2()
            .items_center()
            .bg(sidebar)
            .border_b_1()
            .border_color(border)
            .overflow_x_scrollbar()
            .child(self.render_panel_switcher(placement, cx))
    }

    fn render_phone_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_tab;
        let sidebar = cx.theme().sidebar;
        // Fixed height: the horizontal scroller's wrapper is height 100%
        // unless this row sets one, and then it covers the note.
        div()
            .w_full()
            .h(px(36.))
            .flex_none()
            .overflow_x_scrollbar()
            .bg(sidebar)
            .child(
                h_flex()
                    .gap_1()
                    .px_2()
                    .pb_1()
                    .children(self.tabs.iter().enumerate().map(|(ix, tab)| {
                        let title = tab.document.title(cx);
                        Button::new(("phone-tab", ix))
                            .label(title)
                            .ghost()
                            .small()
                            .selected(active == Some(ix))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.activate_tab(ix, window, cx);
                            }))
                    })),
            )
    }

    fn render_phone_note(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.active_editor() {
            Some(editor) => editor.clone().into_any_element(),
            None => self.render_empty_state(window, cx).into_any_element(),
        }
    }

    fn render_phone_panel(
        &self,
        placement: PanelPlacement,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (sidebar, foreground) = (theme.sidebar, theme.sidebar_foreground);
        let view = self
            .sidebar(placement)
            .active
            .as_ref()
            .and_then(|id| self.panel_views.get(id))
            .map(|(_, view)| view.clone());
        let missing = view.is_none();
        // The list is virtualized, so it needs a bounded height the way the
        // desktop sidebar gives it one.
        div().size_full().bg(sidebar).text_color(foreground).child(
            v_flex().size_full().overflow_hidden().child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .children(view)
                    .when(missing, |body| {
                        body.child(self.panel_placeholder(placement, cx))
                    }),
            ),
        )
    }

    fn render_phone_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let item = |id: &'static str,
                    label: &'static str,
                    screen: PhoneScreen,
                    current: PhoneScreen,
                    cx: &mut Context<Self>| {
            div().flex_1().flex().justify_center().child(
                Button::new(id)
                    .label(label)
                    .ghost()
                    .small()
                    .selected(current == screen)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.phone = screen;
                        cx.notify();
                    })),
            )
        };
        h_flex()
            .flex_none()
            .w_full()
            .h(px(52.))
            .items_center()
            .px_2()
            .bg(theme.sidebar)
            .border_t_1()
            .border_color(theme.sidebar_border)
            .child(item(
                "phone-files",
                "Files",
                PhoneScreen::Browse,
                self.phone,
                cx,
            ))
            .child(item(
                "phone-note",
                "Note",
                PhoneScreen::Note,
                self.phone,
                cx,
            ))
            .child(item(
                "phone-more",
                "More",
                PhoneScreen::Inspect,
                self.phone,
                cx,
            ))
    }
}
