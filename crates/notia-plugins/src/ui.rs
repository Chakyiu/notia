//! Presentation shared by the sidebar panels.

use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::*;

/// The small heading above a panel section, with an optional count.
pub(crate) fn section_header(
    title: impl Into<SharedString>,
    count: Option<usize>,
    cx: &App,
) -> Div {
    let muted = cx.theme().muted_foreground;
    h_flex()
        .px_3()
        .pt_3()
        .pb_1()
        .gap_1()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(muted)
        .child(title.into())
        .children(count.map(|count| format!("({count})")))
}

/// The message a panel shows when it has nothing to list.
pub(crate) fn empty_message(text: impl Into<SharedString>, cx: &App) -> Div {
    v_flex()
        .px_4()
        .py_6()
        .items_center()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// A clickable row in a panel list.
pub(crate) fn row(id: impl Into<ElementId>, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let hover = theme.list_hover;
    div()
        .id(id)
        .mx_1()
        .px_2()
        .py_0p5()
        .rounded(theme.radius)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
}
