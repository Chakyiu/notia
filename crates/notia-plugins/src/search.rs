use std::{collections::HashSet, time::Duration};

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement as _,
    v_flex,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use notia_plugin::{
    Command, Notia, OpenOptions, Panel, PanelPlacement, Plugin, PluginContext, PluginManifest,
    VaultEvent, WorkspaceEvent,
    core::{FileMatches, SearchQuery, VaultPath, search},
    workspace,
};

use crate::ui;

const PANEL_ID: &str = "search:search";

pub struct SearchPlugin;

impl Plugin for SearchPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new("search", "Search").with_description(
            "Search note names and text. Supports \"phrases\", tag:#name and path:folder.",
        )
    }

    fn activate(&self, ctx: &mut PluginContext, _: &mut App) {
        ctx.add_panel(
            Panel::new(
                "search",
                "Search",
                IconName::Search,
                PanelPlacement::Left,
                |window, cx| cx.new(|cx| SearchPanel::new(window, cx)),
            )
            .with_order(10),
        );
        ctx.add_command(
            Command::new("search", "Search in all files", |_, cx| {
                Notia::workspace(cx).update(cx, |model, cx| {
                    model.request_search(SharedString::default(), cx)
                });
            })
            .with_icon(IconName::Search)
            .with_keybinding("secondary-shift-f"),
        );
    }
}

pub struct SearchPanel {
    input: Entity<InputState>,
    query: SharedString,
    results: Vec<FileMatches>,
    collapsed: HashSet<VaultPath>,
    pending: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl SearchPanel {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search…"));
        let subscriptions = vec![
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.schedule_search(cx);
                }
            }),
            cx.subscribe_in(
                &Notia::workspace(cx),
                window,
                |this, _, event, window, cx| {
                    if let WorkspaceEvent::SearchRequested(query) = event {
                        this.show_query(query.clone(), window, cx);
                    }
                },
            ),
            cx.subscribe(&Notia::vault(cx), |this, _, event, cx| {
                if matches!(event, VaultEvent::NotesChanged(_) | VaultEvent::TreeChanged)
                    && !this.query.is_empty()
                {
                    this.schedule_search(cx);
                }
            }),
        ];
        Self {
            input,
            query: SharedString::default(),
            results: Vec::new(),
            collapsed: HashSet::new(),
            pending: None,
            _subscriptions: subscriptions,
        }
    }

    /// Shows the panel with `query`, or keeps the current query when it is
    /// empty, and focuses the search field.
    fn show_query(&mut self, query: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        if !query.is_empty() {
            self.input.update(cx, |input, cx| {
                input.set_value(query.to_string(), window, cx)
            });
            self.run_search(cx);
        }
        workspace::show_panel(PANEL_ID, window, cx);
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn schedule_search(&mut self, cx: &mut Context<Self>) {
        self.pending = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(120))
                .await;
            _ = this.update(cx, |this, cx| this.run_search(cx));
        }));
    }

    fn run_search(&mut self, cx: &mut Context<Self>) {
        self.pending = None;
        self.query = self.input.read(cx).value();
        let query = SearchQuery::parse(&self.query);
        let vault = Notia::vault(cx);
        self.results = search(vault.read(cx).index().notes(), &query, 6);
        cx.notify();
    }

    fn render_file(&self, file: &FileMatches, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, highlight) = (theme.muted_foreground, theme.yellow.opacity(0.35));
        let collapsed = self.collapsed.contains(&file.path);
        let path = file.path.clone();
        let toggle_path = file.path.clone();

        let title = ui::row(SharedString::from(format!("search-file-{}", file.path)), cx)
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new(SharedString::from(format!("search-toggle-{}", file.path)))
                            .icon(if collapsed {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            })
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                if !this.collapsed.remove(&toggle_path) {
                                    this.collapsed.insert(toggle_path.clone());
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(file.path.file_stem().to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(file.line_count.to_string()),
                    ),
            )
            .on_click(move |event, window, cx| {
                let options = if event.modifiers().secondary(cx) {
                    OpenOptions::new().in_new_tab()
                } else {
                    OpenOptions::new()
                };
                workspace::open_note(path.clone(), options, window, cx);
            });

        v_flex().child(title).when(!collapsed, |this| {
            this.children(file.lines.iter().map(|line| {
                let path = file.path.clone();
                let line_ix = line.line;
                let highlights = line.ranges.iter().map(|range| {
                    (
                        range.clone(),
                        HighlightStyle {
                            background_color: Some(highlight),
                            ..Default::default()
                        },
                    )
                });
                ui::row(
                    SharedString::from(format!("search-line-{}-{line_ix}", file.path)),
                    cx,
                )
                .ml_6()
                .text_xs()
                .text_color(muted)
                .child(StyledText::new(line.text.clone()).with_highlights(highlights))
                .on_click(move |_, window, cx| {
                    workspace::open_note(
                        path.clone(),
                        OpenOptions::new().at_line(line_ix),
                        window,
                        cx,
                    );
                })
            }))
        })
    }
}

impl Render for SearchPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let summary = if self.query.trim().is_empty() {
            None
        } else if self.results.is_empty() {
            Some("No matches".to_string())
        } else {
            let lines: usize = self.results.iter().map(|file| file.line_count).sum();
            Some(format!(
                "{} {} · {lines} {}",
                self.results.len(),
                if self.results.len() == 1 {
                    "note"
                } else {
                    "notes"
                },
                if lines == 1 { "line" } else { "lines" }
            ))
        };
        let files: Vec<AnyElement> = self
            .results
            .iter()
            .map(|file| self.render_file(file, cx).into_any_element())
            .collect();

        v_flex()
            .size_full()
            .child(
                div().p_2().child(
                    Input::new(&self.input)
                        .small()
                        .cleanable(true)
                        .prefix(Icon::new(IconName::Search).small().text_color(muted)),
                ),
            )
            .children(summary.map(|summary| {
                div()
                    .px_3()
                    .pb_1()
                    .text_xs()
                    .text_color(muted)
                    .child(summary)
            }))
            .child(
                v_flex()
                    .id("search-results")
                    .flex_1()
                    .min_h_0()
                    .pb_2()
                    .children(files)
                    .overflow_y_scrollbar(),
            )
    }
}
