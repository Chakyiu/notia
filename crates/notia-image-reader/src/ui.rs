use gpui_kit::*;

pub fn empty_message(message: &str, muted: Hsla) -> impl IntoElement {
    div()
        .p_4()
        .text_sm()
        .text_color(muted)
        .child(message.to_string())
}