use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{ActiveTheme as _, Icon, scroll::ScrollableElement as _, v_flex};
use gpui_kit::{prelude::FluentBuilder as _, *};
use notia_plugin::{
    Notia, Panel, PanelPlacement, Plugin, PluginContext, PluginManifest,
    core::{Heading, parse_note},
    workspace,
};

use crate::ui;

pub struct OutlinePlugin;

impl Plugin for OutlinePlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new("outline", "Outline")
            .with_description("Shows the headings of the active note.")
    }

    fn activate(&self, ctx: &mut PluginContext, _: &mut App) {
        ctx.add_panel(
            Panel::new(
                "outline",
                "Outline",
                Icon::new(Lucide::ListTree),
                PanelPlacement::Right,
                |_, cx| cx.new(OutlinePanel::new),
            )
            .with_order(10),
        );
    }
}

pub struct OutlinePanel {
    has_note: bool,
    headings: Vec<Heading>,
    _subscription: Subscription,
}

impl OutlinePanel {
    fn new(cx: &mut Context<Self>) -> Self {
        let workspace = Notia::workspace(cx);
        let subscription = cx.observe(&workspace, |this, _, cx| this.refresh(cx));
        let mut panel = Self {
            has_note: false,
            headings: Vec::new(),
            _subscription: subscription,
        };
        panel.refresh(cx);
        panel
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let workspace = Notia::workspace(cx);
        let note = workspace.read(cx).active_note();
        let headings = note
            .map(|note| parse_note(note.text()).headings)
            .unwrap_or_default();
        let has_note = note.is_some();
        if headings != self.headings || has_note != self.has_note {
            self.headings = headings;
            self.has_note = has_note;
            cx.notify();
        }
    }
}

impl Render for OutlinePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let content = if !self.has_note {
            ui::empty_message("No note is open.", cx).into_any_element()
        } else if self.headings.is_empty() {
            ui::empty_message("This note has no headings.", cx).into_any_element()
        } else {
            let base = self
                .headings
                .iter()
                .map(|heading| heading.level)
                .min()
                .unwrap_or(1);
            v_flex()
                .children(self.headings.iter().enumerate().map(|(ix, heading)| {
                    let line = heading.line;
                    let depth = (heading.level - base) as f32;
                    ui::row(("outline-heading", ix), cx)
                        .pl(px(8. + 14. * depth))
                        .text_sm()
                        .truncate()
                        .when(heading.level > base, |row| row.text_color(muted))
                        .child(heading.text.clone())
                        .on_click(move |_, window, cx| workspace::reveal_line(line, window, cx))
                }))
                .into_any_element()
        };
        v_flex()
            .id("outline")
            .size_full()
            .child(ui::section_header("Outline", None, cx))
            .child(content)
            .overflow_y_scrollbar()
    }
}
