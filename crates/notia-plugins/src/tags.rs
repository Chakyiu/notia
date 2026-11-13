use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, scroll::ScrollableElement as _, v_flex};
use gpui_kit::{prelude::FluentBuilder as _, *};
use notia_plugin::{
    Notia, Panel, PanelPlacement, Plugin, PluginContext, PluginManifest, core::TagCount,
};

use crate::ui;

pub struct TagsPlugin;

impl Plugin for TagsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new("tags", "Tags")
            .with_description("Lists every tag in the vault. Click one to search for it.")
    }

    fn activate(&self, ctx: &mut PluginContext, _: &mut App) {
        ctx.add_panel(
            Panel::new(
                "tags",
                "Tags",
                Icon::new(Lucide::Tags),
                PanelPlacement::Right,
                |_, cx| cx.new(TagsPanel::new),
            )
            .with_order(30),
        );
    }
}

pub struct TagsPanel {
    tags: Vec<TagCount>,
    _subscription: Subscription,
}

impl TagsPanel {
    fn new(cx: &mut Context<Self>) -> Self {
        let vault = Notia::vault(cx);
        let subscription = cx.observe(&vault, |this, vault, cx| {
            let tags = vault.read(cx).index().tags();
            if tags != this.tags {
                this.tags = tags;
                cx.notify();
            }
        });
        Self {
            tags: vault.read(cx).index().tags(),
            _subscription: subscription,
        }
    }
}

impl Render for TagsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, badge) = (theme.muted_foreground, theme.muted);
        let radius = theme.radius;
        v_flex()
            .id("tags")
            .size_full()
            .child(ui::section_header("Tags", Some(self.tags.len()), cx))
            .when(self.tags.is_empty(), |this| {
                this.child(ui::empty_message(
                    "Add #tags to your notes to see them here.",
                    cx,
                ))
            })
            .children(self.tags.iter().map(|tag| {
                let query: SharedString = format!("tag:#{}", tag.tag).into();
                ui::row(SharedString::from(format!("tag-{}", tag.tag)), cx)
                    .text_sm()
                    .child(
                        h_flex()
                            .justify_between()
                            .gap_2()
                            .child(div().truncate().child(format!("#{}", tag.tag)))
                            .child(
                                div()
                                    .px_1p5()
                                    .rounded(radius)
                                    .bg(badge)
                                    .text_xs()
                                    .text_color(muted)
                                    .child(tag.count.to_string()),
                            ),
                    )
                    .on_click(move |_, _, cx| {
                        let query = query.clone();
                        Notia::workspace(cx)
                            .update(cx, |model, cx| model.request_search(query, cx));
                    })
            }))
            .overflow_y_scrollbar()
    }
}
