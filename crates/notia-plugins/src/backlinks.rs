use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, h_flex, scroll::ScrollableElement as _, v_flex,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use notia_plugin::{
    Notia, OpenOptions, Panel, PanelPlacement, Plugin, PluginContext, PluginManifest, StatusItem,
    core::{Backlink, Link, VaultPath},
    note_tooltip, workspace,
};

use crate::ui;

const PANEL_ID: &str = "backlinks:backlinks";

pub struct BacklinksPlugin;

impl Plugin for BacklinksPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new("backlinks", "Backlinks").with_description(
            "Shows the notes that link to the active note, and the links it makes.",
        )
    }

    fn activate(&self, ctx: &mut PluginContext, _: &mut App) {
        ctx.add_panel(
            Panel::new(
                "backlinks",
                "Backlinks",
                Icon::new(Lucide::Link),
                PanelPlacement::Right,
                |_, cx| cx.new(BacklinksPanel::new),
            )
            .with_order(20),
        );
        ctx.add_status_item(
            StatusItem::new("count", |_, cx| cx.new(BacklinkCount::new)).with_order(0),
        );
    }
}

/// The backlinks and outgoing links of the active note.
#[derive(Default, PartialEq)]
struct Links {
    active: Option<VaultPath>,
    /// Backlinks grouped by the note they come from.
    incoming: Vec<(VaultPath, Vec<Backlink>)>,
    outgoing: Vec<(Link, Option<VaultPath>)>,
}

impl Links {
    fn read(cx: &App) -> Self {
        let Some(active) = Notia::workspace(cx).read(cx).active_path().cloned() else {
            return Self::default();
        };
        let vault = Notia::vault(cx);
        let index = vault.read(cx).index();
        let mut incoming: Vec<(VaultPath, Vec<Backlink>)> = Vec::new();
        for backlink in index.backlinks(&active) {
            match incoming.last_mut() {
                Some((source, links)) if *source == backlink.source => links.push(backlink),
                _ => incoming.push((backlink.source.clone(), vec![backlink])),
            }
        }
        let mut outgoing = index.outgoing_links(&active);
        outgoing.dedup_by(|a, b| a.0.target == b.0.target);
        Self {
            active: Some(active),
            incoming,
            outgoing,
        }
    }

    fn count(&self) -> usize {
        self.incoming.iter().map(|(_, links)| links.len()).sum()
    }
}

/// Re-reads the links whenever the active note or the vault changes.
fn observe_links<T: 'static>(
    cx: &mut Context<T>,
    refresh: fn(&mut T, &mut Context<T>),
) -> Vec<Subscription> {
    vec![
        cx.observe(&Notia::workspace(cx), move |this, _, cx| refresh(this, cx)),
        cx.observe(&Notia::vault(cx), move |this, _, cx| refresh(this, cx)),
    ]
}

pub struct BacklinksPanel {
    links: Links,
    _subscriptions: Vec<Subscription>,
}

impl BacklinksPanel {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            links: Links::read(cx),
            _subscriptions: observe_links(cx, Self::refresh),
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let links = Links::read(cx);
        if links != self.links {
            self.links = links;
            cx.notify();
        }
    }
}

impl Render for BacklinksPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let link_color = cx.theme().link;
        let Some(active) = self.links.active.clone() else {
            return v_flex()
                .id("backlinks")
                .size_full()
                .child(ui::empty_message("No note is open.", cx))
                .overflow_y_scrollbar();
        };

        let incoming = self.links.incoming.iter().map(|(source, links)| {
            let open = source.clone();
            let tip = source.clone();
            v_flex()
                .child(
                    ui::row(SharedString::from(format!("backlink-source-{source}")), cx)
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .truncate()
                        .tooltip(note_tooltip(tip))
                        .child(source.file_stem().to_string())
                        .on_click(move |_, window, cx| {
                            workspace::open_note(open.clone(), OpenOptions::new(), window, cx)
                        }),
                )
                .children(links.iter().map(|link| {
                    let (path, line) = (link.source.clone(), link.line);
                    ui::row(SharedString::from(format!("backlink-{source}-{line}")), cx)
                        .ml_3()
                        .text_xs()
                        .text_color(muted)
                        .child(link.context.clone())
                        .on_click(move |_, window, cx| {
                            workspace::open_note(
                                path.clone(),
                                OpenOptions::new().at_line(line),
                                window,
                                cx,
                            );
                        })
                }))
        });

        let outgoing = self
            .links
            .outgoing
            .iter()
            .enumerate()
            .map(|(ix, (link, resolved))| {
                let target = link.target.clone();
                let from = active.clone();
                let tip = resolved.clone();
                ui::row(("outgoing-link", ix), cx)
                    .text_sm()
                    .when_some(tip, |row, path| row.tooltip(note_tooltip(path)))
                    .child(
                        h_flex()
                            .gap_1p5()
                            .child(
                                Icon::new(if resolved.is_some() {
                                    Lucide::Link
                                } else {
                                    Lucide::Link2Off
                                })
                                .xsmall()
                                .text_color(muted),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .when(resolved.is_none(), |label| label.text_color(muted))
                                    .when(resolved.is_some(), |label| label.text_color(link_color))
                                    .child(link.target.clone()),
                            ),
                    )
                    .on_click(move |_, window, cx| {
                        workspace::open_link(&target, Some(from.clone()), false, window, cx)
                    })
            });

        v_flex()
            .id("backlinks")
            .size_full()
            .child(ui::section_header(
                "Linked mentions",
                Some(self.links.count()),
                cx,
            ))
            .when(self.links.incoming.is_empty(), |this| {
                this.child(ui::empty_message("No backlinks found.", cx))
            })
            .children(incoming)
            .child(ui::section_header(
                "Outgoing links",
                Some(self.links.outgoing.len()),
                cx,
            ))
            .when(self.links.outgoing.is_empty(), |this| {
                this.child(ui::empty_message("This note links nowhere.", cx))
            })
            .children(outgoing)
            .overflow_y_scrollbar()
    }
}

/// The backlink count in the status bar.
pub struct BacklinkCount {
    count: Option<usize>,
    _subscriptions: Vec<Subscription>,
}

impl BacklinkCount {
    fn new(cx: &mut Context<Self>) -> Self {
        let links = Links::read(cx);
        Self {
            count: links.active.as_ref().map(|_| links.count()),
            _subscriptions: observe_links(cx, Self::refresh),
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let links = Links::read(cx);
        let count = links.active.as_ref().map(|_| links.count());
        if count != self.count {
            self.count = count;
            cx.notify();
        }
    }
}

impl Render for BacklinkCount {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("backlink-count")
            .cursor_pointer()
            .children(self.count.map(|count| {
                format!(
                    "{count} {}",
                    if count == 1 { "backlink" } else { "backlinks" }
                )
            }))
            .on_click(|_, window, cx| workspace::show_panel(PANEL_ID, window, cx))
    }
}
