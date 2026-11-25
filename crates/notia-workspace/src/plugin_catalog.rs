//! The community plugin store.
//!
//! [notia-manifest](https://github.com/Chakyiu/notia-manifest) publishes
//! `community-plugins.json`. Each entry names a GitHub repository. The Plugin
//! store downloads that repository's archive and copies the plugin files into
//! `.notia/plugins/<id>`, keeping an existing `data.json`.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    io::{Cursor, Read},
    path::{Component, Path, PathBuf},
    time::Duration,
};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, ColorName, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _, Toggle, ToggleGroup, ToggleVariants as _},
    empty::{Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle},
    h_flex,
    input::{Input, InputState},
    notification::Notification,
    scroll::ScrollableElement as _,
    setting::{RenderOptions, SettingGroup, SettingItem, SettingPage},
    skeleton::Skeleton,
    spinner::Spinner,
    tag::Tag,
    text::TextView,
    v_flex,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use notia_plugin::{Notia, PluginHost, PluginKind};
use serde::Deserialize;
use serde_json::Value;

/// The published list Notia installs from.
const CATALOG_URL: &str =
    "https://raw.githubusercontent.com/Chakyiu/notia-manifest/main/community-plugins.json";

const MAX_CATALOG_BYTES: usize = 512 * 1024;
const MAX_MANIFEST_BYTES: usize = 64 * 1024;
const MAX_README_BYTES: usize = 256 * 1024;
const MAX_ARCHIVE_BYTES: usize = 8 * 1024 * 1024;
const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
const MAX_UNPACKED_BYTES: usize = 16 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 2_000;
const MAX_PLUGIN_FILES: usize = 200;
const MAX_PLUGINS: usize = 100;

const SKIP_DIRS: &[&str] = &[
    ".git",
    ".github",
    ".cursor",
    ".impeccable",
    "node_modules",
    "test",
    "tests",
    "target",
];
const SKIP_FILES: &[&str] = &[
    ".DS_Store",
    ".gitignore",
    "README.md",
    "LICENSE",
    "LICENSE.md",
    "PRODUCT.md",
    "package.json",
    "package-lock.json",
    "jsconfig.json",
    "tsconfig.json",
    "notia.d.ts",
    "data.json",
];

/// The community plugin store, beside Plugins in Settings.
pub(crate) fn page() -> SettingPage {
    SettingPage::new("Plugin store")
        .icon(Icon::new(Lucide::Store))
        .resettable(false)
        .group(
            SettingGroup::new()
                .description(
                    "Community plugins from the notia-manifest list. Installing one copies it into this vault. Turn it on under Plugins.",
                )
                .item(
                    SettingItem::render(render_store).keywords([
                        "community",
                        "install",
                        "download",
                        "notia-manifest",
                        "store",
                    ]),
                ),
        )
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ReadmeBody {
    Loading,
    Ready(String),
    Missing,
    Failed(String),
}

enum ReadmeError {
    Missing,
    Failed(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum StoreFilter {
    #[default]
    All,
    Installed,
    Updates,
}

const TWO_PANE_MIN_WIDTH: Pixels = px(720.);

struct CatalogState {
    catalog: Option<Catalog>,
    error: Option<String>,
    loading: bool,
    task: Option<Task<()>>,
    busy: Option<String>,
    install_errors: BTreeMap<String, String>,
    query: Entity<InputState>,
    filter: StoreFilter,
    selected: Option<String>,
    readmes: BTreeMap<String, ReadmeBody>,
    /// The plugin whose README is being fetched. Dropping `readme_task`
    /// cancels that fetch.
    readme_for: Option<String>,
    readme_task: Option<Task<()>>,
}

fn render_store(options: &RenderOptions, window: &mut Window, cx: &mut App) -> AnyElement {
    let watched = update_controller(cx);
    let _updates = window.use_keyed_state("notia-plugin-update-watch-store", cx, move |_, cx| {
        PluginUpdateWatch::new(watched, cx)
    });
    let state = window.use_keyed_state("notia-plugin-store", cx, |window, cx| CatalogState {
        catalog: None,
        error: None,
        loading: false,
        task: None,
        busy: None,
        install_errors: BTreeMap::new(),
        query: cx.new(|cx| InputState::new(window, cx).placeholder("Search plugins")),
        filter: StoreFilter::default(),
        selected: None,
        readmes: BTreeMap::new(),
        readme_for: None,
        readme_task: None,
    });
    let (
        catalog,
        error,
        loading,
        mut busy,
        mut install_errors,
        query_field,
        filter,
        selected,
        readmes,
    ) = {
        let ui = state.read(cx);
        (
            ui.catalog.clone(),
            ui.error.clone(),
            ui.loading,
            ui.busy.clone(),
            ui.install_errors.clone(),
            ui.query.clone(),
            ui.filter,
            ui.selected.clone(),
            ui.readmes.clone(),
        )
    };
    if let Some(id) = installing_plugin(cx) {
        busy = Some(id);
    }
    install_errors.extend(update_errors(cx));
    let query = query_field.read(cx).value().to_string();
    if catalog.is_none() && error.is_none() && !loading {
        let state = state.clone();
        cx.defer(move |cx| start_load(&state, cx));
    }

    let plugins_dir = Notia::vault(cx)
        .read(cx)
        .vault()
        .config_dir()
        .join("plugins");
    let loader_on = PluginHost::global(cx)
        .command("community-plugins:reload")
        .is_some();
    let muted = cx.theme().muted_foreground;
    let danger = cx.theme().danger;
    let control_size = options.size();
    let controls_disabled = loading || busy.is_some();
    let state_for_refresh = state.clone();
    let state_for_filter = state.clone();
    let checking_updates = checking_plugin_updates(cx);
    let theme = cx.theme();
    let border = theme.border;
    let accent = theme.accent;
    let list_active = theme.list_active;
    let list_active_border = theme.list_active_border;
    let counts = catalog
        .as_ref()
        .map(|catalog| store_counts(catalog, &query, &plugins_dir, cx));

    let filter_group = counts.as_ref().map(|counts| {
        let updates_disabled = counts.updates == 0;
        ToggleGroup::new("plugin-store-filter")
            .segmented()
            .single()
            .outline()
            .with_size(control_size)
            .disabled(controls_disabled)
            .child(
                Toggle::new("filter-all")
                    .label(format!("All ({})", counts.total))
                    .checked(filter == StoreFilter::All),
            )
            .child(
                Toggle::new("filter-installed")
                    .label(format!("Installed ({})", counts.installed))
                    .checked(filter == StoreFilter::Installed),
            )
            .child(
                Toggle::new("filter-updates")
                    .label(format!("Updates ({})", counts.updates))
                    .checked(filter == StoreFilter::Updates)
                    .disabled(updates_disabled),
            )
            .on_click(move |next, _, cx| {
                let picked = if next.get(2).copied() == Some(true) && !updates_disabled {
                    StoreFilter::Updates
                } else if next.get(1).copied() == Some(true) {
                    StoreFilter::Installed
                } else {
                    StoreFilter::All
                };
                state_for_filter.update(cx, |this, cx| {
                    this.filter = picked;
                    cx.notify();
                });
            })
    });

    let toolbar = v_flex()
        .w_full()
        .gap_2()
        .child(
            h_flex()
                .w_full()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&query_field).appearance(true).small().w_full()),
                )
                .children(filter_group)
                .child(
                    Button::new("refresh-plugin-store")
                        .outline()
                        .icon(IconName::RefreshCw)
                        .tooltip("Refresh")
                        .accessibility_label("Refresh")
                        .with_size(control_size)
                        .disabled(controls_disabled || (catalog.is_none() && error.is_none()))
                        .on_click(move |_, _, cx| start_load(&state_for_refresh, cx)),
                ),
        )
        .child(div().w_full().border_b_1().border_color(border));

    let count_text = counts.as_ref().map(|counts| {
        let mut line = format!(
            "{} plugins · {} installed · {} update",
            counts.total, counts.installed, counts.updates
        );
        if counts.updates != 1 {
            line.push('s');
        }
        if let Some(catalog) = catalog.as_ref()
            && catalog.skipped > 0
        {
            line.push_str(" · ");
            line.push_str(&skipped_line(catalog.skipped).replace('.', ""));
        }
        if checking_updates {
            line.push_str(" · checking for updates…");
        }
        line
    });

    let loader_banner = (!loader_on).then(|| {
        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .px_3()
            .py_2()
            .rounded(theme.radius)
            .bg(theme.secondary)
            .text_sm()
            .child(
                "Community plugins is off, so installed plugins will not load. Turn them on under Plugins.",
            )
    });

    let catalog_for_layout = catalog.clone();
    let selected_for_layout = selected.clone();
    let store = move |content_size: Size<Pixels>, _: &mut Window, cx: &mut App| -> AnyElement {
        let selected_entry = selected_for_layout.as_deref().and_then(|id| {
            catalog_for_layout
                .as_ref()
                .and_then(|catalog| catalog.entries.iter().find(|entry| entry.id == id))
        });
        let two_pane = content_size.width >= TWO_PANE_MIN_WIDTH;

        let list_pane = render_list_pane(
            catalog_for_layout.as_ref(),
            error.as_deref(),
            loading,
            &query,
            filter,
            selected.as_deref(),
            &busy,
            controls_disabled,
            control_size,
            &state,
            &plugins_dir,
            &install_errors,
            border,
            muted,
            danger,
            accent,
            list_active,
            list_active_border,
            cx,
        );
        let detail_pane = if let Some(entry) = selected_entry {
            let control = entry_control(entry, &plugins_dir, cx);
            render_detail(
                entry,
                readmes.get(&entry.id),
                control,
                busy.as_deref() == Some(entry.id.as_str()),
                controls_disabled,
                control_size,
                &state,
                plugins_dir.clone(),
                border,
                muted,
                danger,
                false,
                install_errors.get(&entry.id).map(String::as_str),
                cx,
            )
            .into_any_element()
        } else {
            detail_placeholder(muted)
        };

        if two_pane || selected_entry.is_some() {
            h_flex()
                .w_full()
                .h_full()
                .min_h(px(360.))
                .min_h_0()
                .child(list_pane)
                .child(detail_pane)
                .into_any_element()
        } else {
            list_pane.into_any_element()
        }
    };

    v_flex()
        .w_full()
        .h_full()
        .min_h_0()
        .gap_2()
        .children(loader_banner)
        .child(toolbar)
        .when_some(count_text, |this, line| {
            this.child(div().text_xs().text_color(muted).child(line))
        })
        .child(
            div()
                .w_full()
                .flex_1()
                .min_h_0()
                .child(container_query(store)),
        )
        .into_any_element()
}

struct StoreCounts {
    total: usize,
    installed: usize,
    updates: usize,
}

fn entry_control(entry: &CatalogEntry, plugins_dir: &Path, cx: &App) -> RowControl {
    let known = known_plugin(&entry.id, cx);
    let folder = folder_kind(plugins_dir, &entry.id);
    let installed_version = installed_version(plugins_dir, &entry.id, known.as_ref());
    row_plan(
        folder,
        known.is_some(),
        &installed_version,
        entry.version.as_deref(),
    )
}

fn store_counts(catalog: &Catalog, query: &str, plugins_dir: &Path, cx: &App) -> StoreCounts {
    let mut counts = StoreCounts {
        total: 0,
        installed: 0,
        updates: 0,
    };
    for entry in catalog
        .entries
        .iter()
        .filter(|entry| matches_query(entry, query))
    {
        counts.total += 1;
        let control = entry_control(entry, plugins_dir, cx);
        if matches_filter(control, StoreFilter::Installed) {
            counts.installed += 1;
        }
        if control == RowControl::Update {
            counts.updates += 1;
        }
    }
    counts
}

fn matches_filter(control: RowControl, filter: StoreFilter) -> bool {
    match filter {
        StoreFilter::All => true,
        StoreFilter::Installed => matches!(
            control,
            RowControl::Installed | RowControl::Update | RowControl::Linked
        ),
        StoreFilter::Updates => control == RowControl::Update,
    }
}

fn status_tag(control: RowControl, busy: bool, size: gpui_kit::component::Size) -> Option<Tag> {
    if busy {
        return None;
    }
    let tag = match control {
        RowControl::Installed => Tag::secondary().child("Installed"),
        RowControl::Update => Tag::color(ColorName::Amber).child("Update"),
        RowControl::Linked => Tag::secondary().outline().child("Linked"),
        RowControl::Install => return None,
    };
    Some(tag.small().with_size(size))
}

fn install_error_banner(error: &str, danger: Hsla) -> impl IntoElement {
    let mut lines = error.lines();
    let summary = lines.next().unwrap_or_default().to_string();
    let details: Vec<&str> = lines.collect();
    h_flex()
        .gap_2()
        .items_start()
        .text_sm()
        .text_color(danger)
        .child(Icon::new(IconName::TriangleAlert).small().mt_0p5())
        .child(
            v_flex()
                .min_w_0()
                .child(summary)
                .when(!details.is_empty(), |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(danger.opacity(0.85))
                            .child(details.join("\n")),
                    )
                }),
        )
}

fn detail_placeholder(muted: Hsla) -> AnyElement {
    Empty::new()
        .header(
            EmptyHeader::new()
                .media(EmptyMedia::new().child(Icon::new(Lucide::Store)))
                .title(EmptyTitle::new().child("Choose a plugin"))
                .description(
                    EmptyDescription::new().child("Choose a plugin to read what it does."),
                ),
        )
        .text_color(muted)
        .into_any_element()
}

fn render_list_pane(
    catalog: Option<&Catalog>,
    error: Option<&str>,
    loading: bool,
    query: &str,
    filter: StoreFilter,
    selected: Option<&str>,
    busy: &Option<String>,
    controls_disabled: bool,
    size: gpui_kit::component::Size,
    state: &Entity<CatalogState>,
    plugins_dir: &Path,
    _install_errors: &BTreeMap<String, String>,
    border: Hsla,
    muted: Hsla,
    danger: Hsla,
    accent: Hsla,
    list_active: Hsla,
    list_active_border: Hsla,
    cx: &App,
) -> AnyElement {
    if loading && catalog.is_none() && error.is_none() {
        return plugin_list_shell(
            border,
            v_flex()
                .gap_2()
                .children((0..6).map(|_| Skeleton::new().h(px(44.)))),
        );
    }
    if let Some(error) = error
        && catalog.is_none()
    {
        let state = state.clone();
        return plugin_list_shell(
            border,
            Empty::new()
                .header(
                    EmptyHeader::new()
                        .title(EmptyTitle::new().child("Could not load the plugin list"))
                        .description(EmptyDescription::new().child(error.to_string())),
                )
                .child(
                    Button::new("plugin-store-retry")
                        .label("Try again")
                        .on_click(move |_, _, cx| start_load(&state, cx)),
                ),
        );
    }
    let Some(catalog) = catalog else {
        return plugin_list_shell(border, div());
    };
    if catalog.entries.is_empty() {
        return plugin_list_shell(
            border,
            Empty::new().header(
                EmptyHeader::new().title(EmptyTitle::new().child("No plugins are listed.")),
            ),
        );
    }

    let mut rows = Vec::new();
    let mut visible = 0usize;
    for entry in &catalog.entries {
        if !matches_query(entry, query) {
            continue;
        }
        let control = entry_control(entry, plugins_dir, cx);
        if !matches_filter(control, filter) {
            continue;
        }
        visible += 1;
        rows.push(render_plugin_row(
            entry,
            control,
            selected == Some(entry.id.as_str()),
            busy.as_deref() == Some(entry.id.as_str()),
            controls_disabled,
            size,
            state,
            plugins_dir,
            border,
            muted,
            danger,
            accent,
            list_active,
            list_active_border,
            cx,
        ));
    }

    if visible == 0 {
        let state_for_action = state.clone();
        let query_owned = query.to_string();
        let (title, clear_search): (SharedString, bool) = match filter {
            StoreFilter::All if !query.trim().is_empty() => {
                (format!("No plugins match “{query_owned}”").into(), true)
            }
            StoreFilter::Installed => ("No community plugins installed yet.".into(), false),
            StoreFilter::Updates => ("Everything is up to date.".into(), false),
            _ => ("No plugins match.".into(), false),
        };
        let show_all = filter == StoreFilter::Installed;
        let mut empty =
            Empty::new().header(EmptyHeader::new().title(EmptyTitle::new().child(title)));
        if clear_search {
            let state = state_for_action.clone();
            empty = empty.child(
                Button::new("plugin-store-clear-search")
                    .label("Clear search")
                    .on_click(move |_, window, cx| {
                        state.update(cx, |this, cx| {
                            this.query.update(cx, |input, cx| {
                                input.set_value("", window, cx);
                            });
                            cx.notify();
                        });
                    }),
            );
        } else if show_all {
            let state = state_for_action.clone();
            empty = empty.child(
                Button::new("plugin-store-show-all")
                    .label("Show all")
                    .on_click(move |_, _, cx| {
                        state.update(cx, |this, cx| {
                            this.filter = StoreFilter::All;
                            cx.notify();
                        });
                    }),
            );
        }
        return plugin_list_shell(border, empty);
    }

    plugin_list_shell(border, v_flex().gap_1().children(rows))
}

fn plugin_list_shell(border: Hsla, body: impl IntoElement) -> AnyElement {
    v_flex()
        .w_72()
        .h_full()
        .min_h_0()
        .flex_shrink_0()
        .border_r_1()
        .border_color(border)
        .py_2()
        .px_2()
        .gap_1()
        .overflow_y_scrollbar()
        .child(body)
        .into_any_element()
}

fn render_plugin_row(
    entry: &CatalogEntry,
    control: RowControl,
    selected: bool,
    this_busy: bool,
    controls_disabled: bool,
    size: gpui_kit::component::Size,
    state: &Entity<CatalogState>,
    plugins_dir: &Path,
    _border: Hsla,
    muted: Hsla,
    _danger: Hsla,
    accent: Hsla,
    list_active: Hsla,
    list_active_border: Hsla,
    cx: &App,
) -> AnyElement {
    let known = known_plugin(&entry.id, cx);
    let installed_version = installed_version(plugins_dir, &entry.id, known.as_ref());
    let description = known
        .as_ref()
        .map(|plugin| plugin.description.as_str())
        .filter(|text| !text.is_empty())
        .unwrap_or(entry.description.as_str())
        .to_string();
    let author = known
        .as_ref()
        .map(|plugin| plugin.author.as_str())
        .filter(|text| !text.is_empty())
        .unwrap_or(entry.author.as_str())
        .to_string();
    let version = if installed_version.is_empty() {
        entry.version.clone().unwrap_or_default()
    } else {
        installed_version.clone()
    };
    let show_action = matches!(control, RowControl::Install | RowControl::Update)
        && (selected || control == RowControl::Update);

    let mut row = h_flex()
        .id(SharedString::from(format!("plugin-store-row-{}", entry.id)))
        .w_full()
        .gap_2()
        .items_start()
        .px_3()
        .py_2()
        .rounded(cx.theme().radius)
        .cursor_pointer()
        .min_h(px(44.))
        .when(selected, |row| {
            row.bg(list_active)
                .border_l_2()
                .border_color(list_active_border)
        })
        .hover(move |style| style.bg(accent))
        .on_click({
            let state = state.clone();
            let entry = entry.clone();
            move |_, _, cx| open_detail(&state, entry.clone(), cx)
        })
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .child(entry.name.clone()),
                        )
                        .child(if this_busy {
                            Spinner::new().with_size(size).into_any_element()
                        } else {
                            status_tag(control, false, size)
                                .map(|tag| tag.into_any_element())
                                .unwrap_or_else(|| div().into_any_element())
                        }),
                )
                .child(div().text_xs().text_color(muted).child(description))
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(meta_line(&author, &version)),
                ),
        );

    if show_action {
        row = row.child(list_install_button(
            entry,
            control,
            this_busy,
            controls_disabled,
            size,
            state,
            plugins_dir,
        ));
    }

    row.into_any_element()
}

fn list_install_button(
    entry: &CatalogEntry,
    control: RowControl,
    this_busy: bool,
    controls_disabled: bool,
    size: gpui_kit::component::Size,
    state: &Entity<CatalogState>,
    plugins_dir: &Path,
) -> Button {
    let (label, icon) = if this_busy && matches!(control, RowControl::Update) {
        ("Updating…", Lucide::RefreshCw)
    } else if this_busy {
        ("Installing…", Lucide::Download)
    } else if matches!(control, RowControl::Update) {
        ("Update", Lucide::RefreshCw)
    } else {
        ("Install", Lucide::Download)
    };
    let state = state.clone();
    let entry = entry.clone();
    let plugins_dir = plugins_dir.to_path_buf();
    Button::new(SharedString::from(format!("list-install-{}", entry.id)))
        .outline()
        .small()
        .icon(icon)
        .label(label)
        .with_size(size)
        .disabled(controls_disabled || this_busy)
        .on_click(move |_, _, cx| {
            open_detail(&state, entry.clone(), cx);
            start_install(&state, plugins_dir.clone(), entry.clone(), cx);
        })
}

fn detail_action_row(
    entry: &CatalogEntry,
    control: RowControl,
    this_busy: bool,
    controls_disabled: bool,
    size: gpui_kit::component::Size,
    state: &Entity<CatalogState>,
    plugins_dir: PathBuf,
) -> impl IntoElement {
    let mut actions = h_flex().w_full().gap_2().items_center();
    if matches!(control, RowControl::Install | RowControl::Update) {
        let label = if this_busy && matches!(control, RowControl::Update) {
            "Updating…"
        } else if this_busy {
            "Installing…"
        } else if matches!(control, RowControl::Update) {
            "Update"
        } else {
            "Install"
        };
        actions = actions.child(
            Button::new(SharedString::from(format!("install-{}", entry.id)))
                .primary()
                .icon(if matches!(control, RowControl::Update) {
                    Lucide::RefreshCw
                } else {
                    Lucide::Download
                })
                .label(label)
                .with_size(size)
                .disabled(controls_disabled || this_busy)
                .on_click({
                    let state = state.clone();
                    let entry = entry.clone();
                    let plugins_dir = plugins_dir.clone();
                    move |_, _, cx| start_install(&state, plugins_dir.clone(), entry.clone(), cx)
                }),
        );
    }
    actions = actions.child(
        Button::new(SharedString::from(format!("github-{}", entry.id)))
            .outline()
            .icon(Lucide::Github)
            .tooltip("GitHub")
            .accessibility_label(format!("Open {} on GitHub", entry.name))
            .with_size(size)
            .on_click({
                let url = github_url(entry);
                move |_, _, cx| cx.open_url(&url)
            }),
    );
    if matches!(
        control,
        RowControl::Installed | RowControl::Update | RowControl::Linked
    ) {
        let label = if this_busy {
            "Uninstalling…"
        } else {
            "Uninstall"
        };
        actions = actions.child(
            Button::new(SharedString::from(format!("uninstall-{}", entry.id)))
                .danger()
                .ghost()
                .icon(Lucide::Trash)
                .label(label)
                .accessibility_label(format!("Uninstall {}", entry.name))
                .with_size(size)
                .disabled(controls_disabled || this_busy)
                .on_click({
                    let state = state.clone();
                    let id = entry.id.clone();
                    let plugins_dir = plugins_dir.clone();
                    move |_, _, cx| start_uninstall(&state, plugins_dir.clone(), id.clone(), cx)
                }),
        );
    }
    actions
}

fn open_detail(state: &Entity<CatalogState>, entry: CatalogEntry, cx: &mut App) {
    let start = state.update(cx, |this, cx| {
        this.selected = Some(entry.id.clone());
        let in_flight = this.readme_for.as_deref() == Some(entry.id.as_str());
        let start = match this.readmes.get(&entry.id) {
            Some(ReadmeBody::Ready(_) | ReadmeBody::Missing) => false,
            Some(ReadmeBody::Loading) => !in_flight,
            Some(ReadmeBody::Failed(_)) | None => true,
        };
        if start {
            this.readmes.insert(entry.id.clone(), ReadmeBody::Loading);
            this.readme_for = Some(entry.id.clone());
        }
        cx.notify();
        start
    });
    if start {
        start_readme(state, entry, cx);
    }
}

fn start_readme(state: &Entity<CatalogState>, entry: CatalogEntry, cx: &mut App) {
    state.update(cx, |this, cx| {
        let id = entry.id.clone();
        this.readme_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { fetch_readme(&entry) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.readme_for.as_deref() == Some(id.as_str()) {
                    this.readme_for = None;
                    this.readme_task = None;
                }
                this.readmes.insert(
                    id,
                    match result {
                        Ok(text) => ReadmeBody::Ready(text),
                        Err(ReadmeError::Missing) => ReadmeBody::Missing,
                        Err(ReadmeError::Failed(message)) => ReadmeBody::Failed(message),
                    },
                );
                cx.notify();
            });
        }));
    });
}

fn render_detail(
    entry: &CatalogEntry,
    readme: Option<&ReadmeBody>,
    control: RowControl,
    this_busy: bool,
    controls_disabled: bool,
    size: gpui_kit::component::Size,
    state: &Entity<CatalogState>,
    plugins_dir: PathBuf,
    border: Hsla,
    muted: Hsla,
    danger: Hsla,
    narrow: bool,
    error: Option<&str>,
    cx: &App,
) -> impl IntoElement {
    let version = entry.version.as_deref().unwrap_or("");
    let meta = format!("by {} · v{version} · {}", entry.author, entry.id);
    let back_state = state.clone();
    let radius = cx.theme().radius;

    let mut header = v_flex()
        .w_full()
        .gap_2()
        .pb_3()
        .border_b_1()
        .border_color(border)
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .gap_2()
                .items_start()
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(entry.name.clone()),
                )
                .children(status_tag(control, this_busy, size)),
        )
        .child(div().text_xs().text_color(muted).child(meta))
        .child(detail_action_row(
            entry,
            control,
            this_busy,
            controls_disabled,
            size,
            state,
            plugins_dir,
        ));
    if control == RowControl::Update
        && let Some(remote) = entry.version.as_ref()
    {
        header = header.child(
            div()
                .w_full()
                .px_3()
                .py_2()
                .rounded(radius)
                .border_1()
                .border_color(border)
                .text_sm()
                .child(format!("Version {remote} is available.")),
        );
    }
    if let Some(error) = error {
        header = header.child(install_error_banner(error, danger));
    }

    v_flex()
        .flex_1()
        .h_full()
        .min_h_0()
        .min_w(px(360.))
        .min_w_0()
        .px_4()
        .py_3()
        .gap_3()
        .when(narrow, |panel| {
            panel.child(
                Button::new("plugin-store-back")
                    .ghost()
                    .icon(Lucide::ArrowLeft)
                    .tooltip("Back")
                    .accessibility_label("Back")
                    .with_size(size)
                    .on_click(move |_, _, cx| {
                        back_state.update(cx, |this, cx| {
                            this.selected = None;
                            cx.notify();
                        });
                    }),
            )
        })
        .child(header)
        .child(
            div()
                .w_full()
                .min_w_0()
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .child(readme_view(
                    &entry.id,
                    readme,
                    state.clone(),
                    entry.clone(),
                    muted,
                    danger,
                )),
        )
}

fn readme_view(
    id: &str,
    readme: Option<&ReadmeBody>,
    state: Entity<CatalogState>,
    entry: CatalogEntry,
    muted: Hsla,
    danger: Hsla,
) -> AnyElement {
    match readme {
        Some(ReadmeBody::Ready(text)) => div()
            .w_full()
            .min_w_0()
            .child(
                TextView::markdown(
                    SharedString::from(format!("plugin-readme-{id}")),
                    text.clone(),
                )
                .scrollable(false)
                .w_full(),
            )
            .into_any_element(),
        Some(ReadmeBody::Missing) => div()
            .text_sm()
            .text_color(muted)
            .child("This repository has no README.")
            .into_any_element(),
        Some(ReadmeBody::Failed(message)) => v_flex()
            .gap_2()
            .child(div().text_sm().text_color(danger).child(message.clone()))
            .child(
                Button::new(SharedString::from(format!("retry-readme-{id}")))
                    .outline()
                    .label("Retry")
                    .on_click(move |_, _, cx| start_readme(&state, entry.clone(), cx)),
            )
            .into_any_element(),
        Some(ReadmeBody::Loading) => v_flex()
            .gap_2()
            .children((0..3).map(|_| Skeleton::new()))
            .into_any_element(),
        None => v_flex()
            .gap_2()
            .children((0..3).map(|_| Skeleton::new()))
            .into_any_element(),
    }
}

fn start_load(state: &Entity<CatalogState>, cx: &mut App) {
    state.update(cx, |this, cx| {
        if this.loading || this.task.is_some() || this.busy.is_some() {
            return;
        }
        this.loading = true;
        this.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async { fetch_catalog() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                this.task = None;
                match result {
                    Ok(catalog) => {
                        this.catalog = Some(catalog);
                        this.error = None;
                    }
                    Err(message) => {
                        log::warn!("could not load the plugin list: {message}");
                        this.error = Some(message);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    });
}

fn start_install(
    state: &Entity<CatalogState>,
    plugins_dir: PathBuf,
    entry: CatalogEntry,
    cx: &mut App,
) {
    let blocked = {
        let ui = state.read(cx);
        ui.loading || ui.task.is_some() || ui.busy.is_some()
    };
    if blocked || installing_plugin(cx).is_some() {
        return;
    }
    let id = entry.id.clone();
    state.update(cx, |this, cx| {
        this.install_errors.remove(&id);
        cx.notify();
    });
    install_plugin(plugins_dir, entry, cx);
}

fn start_uninstall(state: &Entity<CatalogState>, plugins_dir: PathBuf, id: String, cx: &mut App) {
    if installing_plugin(cx).is_some() {
        return;
    }
    state.update(cx, |this, cx| {
        if this.loading || this.task.is_some() || this.busy.is_some() {
            return;
        }
        this.busy = Some(id.clone());
        this.install_errors.remove(&id);
        let plugin_id = id.clone();
        this.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { uninstall_plugin(&plugins_dir, &plugin_id) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.task = None;
                this.busy = None;
                match &result {
                    Ok(()) => {
                        this.install_errors.remove(&id);
                        notia_js::reload_community_plugins(cx);
                    }
                    Err(message) => {
                        log::warn!("could not uninstall {id}: {message}");
                        this.install_errors.insert(id, message.clone());
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    });
}

struct KnownPlugin {
    description: String,
    author: String,
    version: String,
}

fn known_plugin(id: &str, cx: &App) -> Option<KnownPlugin> {
    let host = PluginHost::global(cx);
    let (manifest, _) = host.plugins().find(|(manifest, _)| {
        manifest.kind() == PluginKind::Community && manifest.id().as_ref() == id
    })?;
    Some(KnownPlugin {
        description: manifest.description().to_string(),
        author: manifest.author().to_string(),
        version: manifest.version().to_string(),
    })
}

fn installed_version(plugins_dir: &Path, id: &str, known: Option<&KnownPlugin>) -> String {
    known
        .map(|plugin| plugin.version.clone())
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| version_on_disk(&plugins_dir.join(id)))
}

fn version_on_disk(dir: &Path) -> String {
    let Ok(text) = fs::read_to_string(dir.join("manifest.json")) else {
        return String::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return String::new();
    };
    value
        .get("version")
        .and_then(|version| version.as_str())
        .map(str::trim)
        .filter(|version| valid_version(version))
        .unwrap_or("")
        .to_string()
}

fn folder_kind(plugins_dir: &Path, id: &str) -> FolderKind {
    match fs::symlink_metadata(plugins_dir.join(id)) {
        Ok(meta) if meta.file_type().is_symlink() => FolderKind::Linked,
        Ok(meta) if meta.is_dir() => FolderKind::Directory,
        _ => FolderKind::Missing,
    }
}

fn meta_line(author: &str, version: &str) -> String {
    let mut parts = Vec::new();
    if !author.is_empty() {
        parts.push(format!("by {author}"));
    }
    if !version.is_empty() {
        parts.push(format!("v{version}"));
    }
    parts.join(" · ")
}

fn skipped_line(skipped: usize) -> String {
    if skipped == 1 {
        "1 entry was skipped.".into()
    } else {
        format!("{skipped} entries were skipped.")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Catalog {
    entries: Vec<CatalogEntry>,
    skipped: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CatalogEntry {
    id: String,
    name: String,
    author: String,
    description: String,
    repo: String,
    branch: String,
    version: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FolderKind {
    Missing,
    Directory,
    Linked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowControl {
    Install,
    Installed,
    Linked,
    Update,
}

/// What the store row should offer.
fn row_plan(
    folder: FolderKind,
    registered: bool,
    installed_version: &str,
    remote_version: Option<&str>,
) -> RowControl {
    let newer = remote_version.is_some_and(|remote| remote_is_newer(remote, installed_version));
    if folder == FolderKind::Linked {
        return RowControl::Linked;
    }
    let installed = folder == FolderKind::Directory || registered;
    if !installed {
        return RowControl::Install;
    }
    if newer {
        RowControl::Update
    } else {
        RowControl::Installed
    }
}

fn matches_query(entry: &CatalogEntry, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    entry.name.to_lowercase().contains(&query)
        || entry.description.to_lowercase().contains(&query)
        || entry.author.to_lowercase().contains(&query)
        || entry.id.to_lowercase().contains(&query)
}

fn remote_is_newer(remote: &str, installed: &str) -> bool {
    let remote = remote.trim().trim_start_matches('v');
    let installed = installed.trim().trim_start_matches('v');
    if remote.is_empty() {
        return false;
    }
    if installed.is_empty() {
        return true;
    }
    let Some(remote) = version_parts(remote) else {
        return false;
    };
    let Some(installed) = version_parts(installed) else {
        return false;
    };
    let len = remote.len().max(installed.len());
    for index in 0..len {
        let left = remote.get(index).copied().unwrap_or(0);
        let right = installed.get(index).copied().unwrap_or(0);
        if left != right {
            return left > right;
        }
    }
    false
}

fn version_parts(version: &str) -> Option<Vec<u64>> {
    let core = version.split(['-', '+']).next().unwrap_or(version);
    if core.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    for part in core.split('.') {
        if part.is_empty() {
            return None;
        }
        parts.push(part.parse().ok()?);
    }
    Some(parts)
}

fn valid_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 40
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
}

#[derive(Deserialize)]
struct CatalogFile {
    id: String,
    name: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    description: String,
    repo: String,
    #[serde(default)]
    branch: String,
}

fn parse_catalog(text: &str) -> Result<Catalog, String> {
    let values: Vec<Value> = serde_json::from_str(text).map_err(|error| {
        log::warn!("plugin list is invalid: {error}");
        "The plugin list could not be read.".to_string()
    })?;
    let mut entries = Vec::new();
    let mut skipped = 0;
    let mut seen = BTreeSet::new();
    for value in values {
        if entries.len() >= MAX_PLUGINS {
            skipped += 1;
            continue;
        }
        match serde_json::from_value::<CatalogFile>(value).map(|file| accept_entry(file, &mut seen))
        {
            Ok(Ok(entry)) => entries.push(entry),
            Ok(Err(reason)) => {
                log::warn!("skipping a plugin list entry: {reason}");
                skipped += 1;
            }
            Err(error) => {
                log::warn!("skipping a plugin list entry: {error}");
                skipped += 1;
            }
        }
    }
    Ok(Catalog { entries, skipped })
}

fn accept_entry(file: CatalogFile, seen: &mut BTreeSet<String>) -> Result<CatalogEntry, String> {
    let id = file.id.trim();
    if !valid_plugin_id(id) {
        return Err(format!("“{id}” is not a valid plugin id"));
    }
    if !seen.insert(id.to_string()) {
        return Err(format!("“{id}” is listed twice"));
    }
    let name = bounded(file.name.trim(), 80);
    if name.is_empty() {
        return Err(format!("“{id}” has no name"));
    }
    let repo = file.repo.trim();
    let Some((owner, repo_name)) = repo.split_once('/') else {
        return Err(format!("“{id}” has no repository"));
    };
    if repo.split('/').nth(2).is_some()
        || !valid_github_name(owner)
        || !valid_github_name(repo_name)
    {
        return Err(format!("“{id}” names a repository Notia will not download"));
    }
    let branch = if file.branch.trim().is_empty() {
        "main"
    } else {
        file.branch.trim()
    };
    if !valid_branch(branch) {
        return Err(format!("“{id}” has a branch Notia will not download"));
    }
    Ok(CatalogEntry {
        id: id.to_string(),
        name,
        author: bounded(file.author.trim(), 80),
        description: bounded(file.description.trim(), 400),
        repo: format!("{owner}/{repo_name}"),
        branch: branch.to_string(),
        version: None,
    })
}

fn valid_plugin_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        && !id.starts_with('-')
        && id != "."
        && id != ".."
}

fn valid_github_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 100
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_branch(branch: &str) -> bool {
    !branch.is_empty()
        && branch.len() <= 100
        && !branch.starts_with('-')
        && !branch.starts_with('/')
        && !branch.ends_with('/')
        && !branch.ends_with('.')
        && !branch.contains("//")
        && !branch.contains("..")
        && !branch.contains('@')
        && branch
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
}

fn bounded(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let end = text
        .char_indices()
        .nth(max)
        .map(|(index, _)| index)
        .unwrap_or(text.len());
    text[..end].trim_end().to_string()
}

fn archive_url(entry: &CatalogEntry) -> String {
    format!(
        "https://codeload.github.com/{}/zip/refs/heads/{}",
        entry.repo, entry.branch
    )
}

fn manifest_url(entry: &CatalogEntry) -> String {
    format!(
        "https://raw.githubusercontent.com/{}/{}/manifest.json",
        entry.repo, entry.branch
    )
}

fn github_url(entry: &CatalogEntry) -> String {
    format!("https://github.com/{}", entry.repo)
}

fn readme_url(entry: &CatalogEntry) -> String {
    format!(
        "https://raw.githubusercontent.com/{}/{}/README.md",
        entry.repo, entry.branch
    )
}

enum HttpError {
    Network,
    Status(u16),
    TooLarge,
    Read,
}

fn explain(error: HttpError, not_found: &str) -> String {
    match error {
        HttpError::Network | HttpError::Read => "Could not reach GitHub.".into(),
        HttpError::TooLarge => "The download is too large.".into(),
        HttpError::Status(404) => not_found.into(),
        HttpError::Status(403 | 429) => "GitHub is limiting requests. Try again later.".into(),
        HttpError::Status(_) => "GitHub returned an unexpected reply.".into(),
    }
}

fn http_get(url: &str, limit: usize, timeout: Duration) -> Result<Vec<u8>, HttpError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .into();
    let response = agent
        .get(url)
        .header("User-Agent", concat!("Notia/", env!("CARGO_PKG_VERSION")))
        .header(
            "Accept",
            "application/vnd.github+json, application/zip, application/octet-stream, application/json",
        )
        .call();
    let mut response = match response {
        Ok(response) => response,
        Err(error) => {
            log::warn!("plugin list request failed: {error}");
            return Err(HttpError::Network);
        }
    };
    let status = response.status().as_u16();
    if status != 200 {
        return Err(HttpError::Status(status));
    }
    if response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > limit)
    {
        return Err(HttpError::TooLarge);
    }
    match response
        .body_mut()
        .with_config()
        .limit(limit as u64)
        .read_to_vec()
    {
        Ok(body) => Ok(body),
        Err(error) => {
            log::warn!("could not read a plugin download: {error}");
            if error.to_string().to_ascii_lowercase().contains("limit") {
                Err(HttpError::TooLarge)
            } else {
                Err(HttpError::Read)
            }
        }
    }
}

enum RemoteManifest {
    Unread,
    Match { version: Option<String> },
    Mismatch,
}

fn remote_manifest(entry: &CatalogEntry) -> RemoteManifest {
    let body = match http_get(
        &manifest_url(entry),
        MAX_MANIFEST_BYTES,
        Duration::from_secs(20),
    ) {
        Ok(body) => body,
        Err(HttpError::Status(404)) => return RemoteManifest::Mismatch,
        Err(_) => return RemoteManifest::Unread,
    };
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        return RemoteManifest::Mismatch;
    };
    let Some(id) = value.get("id").and_then(|id| id.as_str()) else {
        return RemoteManifest::Mismatch;
    };
    if id.trim() != entry.id {
        return RemoteManifest::Mismatch;
    }
    let version = value
        .get("version")
        .and_then(|version| version.as_str())
        .map(str::trim)
        .filter(|version| valid_version(version))
        .map(str::to_string);
    RemoteManifest::Match { version }
}

fn fetch_catalog_list() -> Result<Catalog, String> {
    let body = http_get(CATALOG_URL, MAX_CATALOG_BYTES, Duration::from_secs(20))
        .map_err(|error| explain(error, "The plugin list was not found."))?;
    let text =
        String::from_utf8(body).map_err(|_| "The plugin list could not be read.".to_string())?;
    parse_catalog(&text)
}

fn fetch_catalog() -> Result<Catalog, String> {
    let mut catalog = fetch_catalog_list()?;
    let mut entries = Vec::new();
    for mut entry in catalog.entries.drain(..) {
        match remote_manifest(&entry) {
            RemoteManifest::Unread => entries.push(entry),
            RemoteManifest::Match { version } => {
                entry.version = version;
                entries.push(entry);
            }
            RemoteManifest::Mismatch => {
                log::warn!(
                    "plugin list entry “{}” does not match its repository manifest",
                    entry.id
                );
                catalog.skipped += 1;
            }
        }
    }
    catalog.entries = entries;
    Ok(catalog)
}

fn download_archive(entry: &CatalogEntry) -> Result<Vec<u8>, String> {
    http_get(
        &archive_url(entry),
        MAX_ARCHIVE_BYTES,
        Duration::from_secs(60),
    )
    .map_err(|error| explain(error, "That plugin repository was not found."))
}

fn fetch_readme(entry: &CatalogEntry) -> Result<String, ReadmeError> {
    match http_get(
        &readme_url(entry),
        MAX_README_BYTES,
        Duration::from_secs(20),
    ) {
        Ok(bytes) => String::from_utf8(bytes)
            .map_err(|_| ReadmeError::Failed("The README could not be read.".into())),
        Err(HttpError::Status(404)) => Err(ReadmeError::Missing),
        Err(HttpError::TooLarge) => Err(ReadmeError::Failed("The README is too large.".into())),
        Err(error) => Err(ReadmeError::Failed(explain(
            error,
            "This repository has no README.",
        ))),
    }
}

fn uninstall_plugin(plugins_dir: &Path, id: &str) -> Result<(), String> {
    if !valid_plugin_id(id) {
        return Err("The plugin id is not valid.".into());
    }
    let dest = plugins_dir.join(id);
    match fs::symlink_metadata(&dest) {
        Ok(meta) if meta.file_type().is_symlink() => fs::remove_file(&dest).map_err(|error| {
            log::warn!("could not remove the plugin link: {error}");
            "Could not uninstall the plugin.".to_string()
        }),
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(&dest).map_err(|error| {
            log::warn!("could not remove the plugin folder: {error}");
            "Could not uninstall the plugin.".to_string()
        }),
        Ok(_) => Err("Could not uninstall the plugin.".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err("This plugin is not installed in the plugins folder.".into())
        }
        Err(error) => {
            log::warn!("could not uninstall {id}: {error}");
            Err("Could not uninstall the plugin.".into())
        }
    }
}

fn install_archive(plugins_dir: &Path, id: &str, archive: &[u8]) -> Result<(), String> {
    if !valid_plugin_id(id) {
        return Err("The plugin id is not valid.".into());
    }
    let files = extract_plugin(archive)?;
    let manifest = files
        .get("manifest.json")
        .ok_or("This repository has no manifest.json.")?;
    let parsed = parse_plugin_manifest(manifest)?;
    if parsed.id != id {
        return Err(format!(
            "The list names this plugin “{id}”, and the manifest id is “{}”.",
            parsed.id
        ));
    }
    if !files.contains_key(&parsed.main) {
        return Err(format!("This repository is missing {}.", parsed.main));
    }
    swap_in(plugins_dir, id, &files)
}

struct PluginManifestFile {
    id: String,
    main: String,
}

fn parse_plugin_manifest(bytes: &[u8]) -> Result<PluginManifestFile, String> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| "This repository's manifest.json could not be read.".to_string())?;
    let id = value
        .get("id")
        .and_then(|id| id.as_str())
        .unwrap_or("")
        .trim();
    if !valid_plugin_id(id) {
        return Err("This repository's manifest.json has no usable id.".into());
    }
    let name = value
        .get("name")
        .and_then(|name| name.as_str())
        .unwrap_or("")
        .trim();
    if name.is_empty() {
        return Err("This repository's manifest.json has no name.".into());
    }
    let main = value
        .get("main")
        .and_then(|main| main.as_str())
        .unwrap_or("main.js");
    let main = notia_plugin::core::VaultPath::new(main);
    if main.is_root() {
        return Err("This repository's manifest.json has an empty main.".into());
    }
    Ok(PluginManifestFile {
        id: id.to_string(),
        main: main.as_str().to_string(),
    })
}

fn extract_plugin(archive: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(archive))
        .map_err(|_| "The download is not a plugin archive.".to_string())?;
    if zip.len() > MAX_ARCHIVE_ENTRIES {
        return Err("The plugin archive has too many files.".into());
    }
    let mut listed = Vec::new();
    for index in 0..zip.len() {
        let file = zip
            .by_index(index)
            .map_err(|_| "The plugin archive could not be read.".to_string())?;
        if !file.is_file() {
            continue;
        }
        if file
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            continue;
        }
        let Some(name) = file.enclosed_name() else {
            continue;
        };
        if name.to_string_lossy().contains('\\') {
            continue;
        }
        listed.push((index, name.to_path_buf()));
    }
    let strip_root = !listed.is_empty()
        && listed
            .iter()
            .all(|(_, path)| path.components().nth(1).is_some())
        && {
            let root = listed[0].1.components().next();
            listed
                .iter()
                .all(|(_, path)| path.components().next() == root)
        };
    let mut files = BTreeMap::new();
    let mut total = 0usize;
    for (index, path) in listed {
        let relative = if strip_root {
            path.components().skip(1).collect::<PathBuf>()
        } else {
            path
        };
        if !keep_file(&relative) {
            continue;
        }
        let mut file = zip
            .by_index(index)
            .map_err(|_| "The plugin archive could not be read.".to_string())?;
        let mut bytes = Vec::new();
        (&mut file)
            .take((MAX_FILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "The plugin archive could not be read.".to_string())?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err("A file in the plugin is too large.".into());
        }
        let key = relative.to_string_lossy().replace('\\', "/");
        total += bytes.len();
        if total > MAX_UNPACKED_BYTES || files.len() >= MAX_PLUGIN_FILES {
            return Err("The plugin is too large.".into());
        }
        files.insert(key, bytes);
    }
    if files.is_empty() {
        return Err("The plugin archive has no plugin files.".into());
    }
    Ok(files)
}

fn keep_file(relative: &Path) -> bool {
    let Some(file_name) = relative.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if file_name.starts_with('.') || SKIP_FILES.contains(&file_name) {
        return false;
    }
    relative.components().all(|component| {
        if matches!(component, Component::ParentDir | Component::RootDir) {
            return false;
        }
        let Some(name) = component.as_os_str().to_str() else {
            return false;
        };
        !name.starts_with('.') && !SKIP_DIRS.contains(&name)
    })
}

fn swap_in(plugins_dir: &Path, id: &str, files: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    fs::create_dir_all(plugins_dir).map_err(|error| {
        log::warn!("could not create the plugins folder: {error}");
        "Could not create the plugins folder.".to_string()
    })?;
    let dest = plugins_dir.join(id);
    if let Ok(meta) = fs::symlink_metadata(&dest) {
        if meta.file_type().is_symlink() {
            return Err(
                "This plugin is linked from another folder. Remove the link before installing it."
                    .into(),
            );
        }
        if !meta.is_dir() {
            return Err("Could not replace the installed plugin.".into());
        }
    }
    let staging = plugins_dir.join(format!(".install-{id}"));
    clear_dir(&staging)?;
    if let Err(error) = write_tree(&staging, files) {
        let _ = clear_dir(&staging);
        return Err(error);
    }
    if let Ok(data) = fs::read(dest.join("data.json"))
        && let Err(error) = fs::write(staging.join("data.json"), data)
    {
        log::warn!("could not keep plugin settings: {error}");
        let _ = clear_dir(&staging);
        return Err("Could not keep the plugin's settings.".into());
    }
    let backup = plugins_dir.join(format!(".backup-{id}"));
    if fs::symlink_metadata(&dest).is_ok() {
        if let Err(error) = clear_dir(&backup) {
            let _ = clear_dir(&staging);
            return Err(error);
        }
        fs::rename(&dest, &backup).map_err(|error| {
            log::warn!("could not move the installed plugin aside: {error}");
            let _ = clear_dir(&staging);
            "Could not replace the installed plugin.".to_string()
        })?;
    }
    if let Err(error) = fs::rename(&staging, &dest) {
        log::warn!("could not move the plugin into place: {error}");
        if fs::symlink_metadata(&backup).is_ok() {
            let _ = fs::rename(&backup, &dest);
        }
        let _ = clear_dir(&staging);
        return Err("Could not install the plugin.".into());
    }
    if fs::symlink_metadata(&backup).is_ok() {
        let _ = clear_dir(&backup);
    }
    Ok(())
}

fn clear_dir(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err("Could not prepare the plugins folder.".into())
        }
        Ok(_) => fs::remove_dir_all(path).map_err(|error| {
            log::warn!("could not remove {}: {error}", path.display());
            "Could not prepare the plugins folder.".to_string()
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => {
            log::warn!("could not prepare {}: {error}", path.display());
            Err("Could not prepare the plugins folder.".into())
        }
    }
}

fn write_tree(root: &Path, files: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    for (path, bytes) in files {
        let relative = Path::new(path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, Component::ParentDir | Component::RootDir))
        {
            return Err("The plugin archive has an unsafe path.".into());
        }
        let target = root.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                log::warn!("could not create a plugin folder: {error}");
                "Could not write the plugin.".to_string()
            })?;
        }
        fs::write(&target, bytes).map_err(|error| {
            log::warn!("could not write a plugin file: {error}");
            "Could not write the plugin.".to_string()
        })?;
    }
    Ok(())
}

/// A newer published copy of a plugin that is installed in this vault.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AvailablePluginUpdate {
    pub available: String,
}

/// Notices that a plugin update is ready. A later notice with this id
/// replaces the one already on screen.
struct PluginUpdateNotice;

/// The result of installing or updating one plugin from the store.
struct PluginInstallNotice;

#[derive(Clone)]
struct TrackedUpdate {
    id: String,
    name: String,
    available: String,
    entry: CatalogEntry,
}

struct PluginUpdateHost {
    controller: Entity<PluginUpdateController>,
}

impl Global for PluginUpdateHost {}

pub(crate) struct PluginUpdateController {
    updates: BTreeMap<String, TrackedUpdate>,
    errors: BTreeMap<String, String>,
    checking: bool,
    installing: Option<String>,
    plugins_dir: Option<PathBuf>,
    /// Bumped when a new check starts, so a slower check for another vault
    /// cannot publish over it.
    check_generation: u64,
    check_task: Option<Task<()>>,
    install_task: Option<Task<()>>,
}

impl PluginUpdateController {
    fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            updates: BTreeMap::new(),
            errors: BTreeMap::new(),
            checking: false,
            installing: None,
            plugins_dir: None,
            check_generation: 0,
            check_task: None,
            install_task: None,
        }
    }

    fn check(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if self.checking && self.plugins_dir.as_ref() == Some(&dir) {
            return;
        }
        if self.plugins_dir.as_ref() != Some(&dir) {
            self.updates.clear();
            self.errors.clear();
        }
        self.plugins_dir = Some(dir.clone());
        self.checking = true;
        self.check_generation = self.check_generation.wrapping_add(1);
        let generation = self.check_generation;
        cx.notify();
        let listed = dir.clone();
        self.check_task = Some(cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { look_for_updates(&listed) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if generation != this.check_generation {
                    return;
                }
                this.checking = false;
                this.check_task = None;
                match found {
                    Ok(updates) => {
                        let updates: Vec<_> = updates
                            .into_iter()
                            .filter(|update| still_newer(&dir, update))
                            .collect();
                        if !updates.is_empty() {
                            announce_available(&updates, cx);
                        }
                        this.errors
                            .retain(|id, _| updates.iter().any(|update| update.id == *id));
                        this.updates = updates
                            .into_iter()
                            .map(|update| (update.id.clone(), update))
                            .collect();
                    }
                    Err(message) => {
                        log::warn!("could not check plugin updates: {message}");
                    }
                }
                cx.notify();
            });
        }));
    }

    fn install(&mut self, dir: PathBuf, entry: CatalogEntry, cx: &mut Context<Self>) {
        if self.installing.is_some() {
            return;
        }
        let id = entry.id.clone();
        let name = entry.name.clone();
        let replacing =
            self.updates.contains_key(&id) || folder_kind(&dir, &id) == FolderKind::Directory;
        self.installing = Some(id.clone());
        self.errors.remove(&id);
        self.plugins_dir = Some(dir.clone());
        cx.notify();
        let archive_id = id.clone();
        let archive_dir = dir;
        self.install_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let bytes = download_archive(&entry)?;
                    install_archive(&archive_dir, &archive_id, &bytes)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.finish_install(id, name, replacing, result, cx);
            });
        }));
    }

    fn finish_install(
        &mut self,
        id: String,
        name: String,
        replacing: bool,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        self.installing = None;
        self.install_task = None;
        match result {
            Ok(()) => {
                let was_update = self.updates.remove(&id).is_some();
                self.errors.remove(&id);
                notia_js::reload_community_plugins(cx);
                if was_update && !self.updates.is_empty() {
                    let left: Vec<_> = self.updates.values().cloned().collect();
                    announce_available(&left, cx);
                } else if was_update {
                    push_note(
                        Notification::success(format!("Updated {name}."))
                            .id::<PluginUpdateNotice>(),
                        cx,
                    );
                } else {
                    let verb = if replacing { "Updated" } else { "Installed" };
                    push_note(
                        Notification::success(format!("{verb} {name}."))
                            .id::<PluginInstallNotice>(),
                        cx,
                    );
                }
            }
            Err(message) => {
                log::warn!("could not install {id}: {message}");
                self.errors.insert(id, message.clone());
                push_note(Notification::error(message).id::<PluginInstallNotice>(), cx);
            }
        }
        cx.notify();
    }
}

/// Keeps a settings page in step with the update check.
pub(crate) struct PluginUpdateWatch {
    _observe: Subscription,
}

impl PluginUpdateWatch {
    pub(crate) fn new(controller: Entity<PluginUpdateController>, cx: &mut Context<Self>) -> Self {
        Self {
            _observe: cx.observe(&controller, |_, _, cx| cx.notify()),
        }
    }
}

/// Looks for newer copies of the plugins installed in the open vault.
pub(crate) fn on_launch(cx: &mut App) {
    let dir = Notia::vault(cx)
        .read(cx)
        .vault()
        .config_dir()
        .join("plugins");
    controller(cx).update(cx, |this, cx| this.check(dir, cx));
}

pub(crate) fn update_controller(cx: &mut App) -> Entity<PluginUpdateController> {
    controller(cx)
}

pub(crate) fn available_update(id: &str, cx: &App) -> Option<AvailablePluginUpdate> {
    let host = cx.try_global::<PluginUpdateHost>()?;
    host.controller
        .read(cx)
        .updates
        .get(id)
        .map(|update| AvailablePluginUpdate {
            available: update.available.clone(),
        })
}

pub(crate) fn checking_plugin_updates(cx: &App) -> bool {
    cx.try_global::<PluginUpdateHost>()
        .is_some_and(|host| host.controller.read(cx).checking)
}

pub(crate) fn installing_plugin(cx: &App) -> Option<String> {
    cx.try_global::<PluginUpdateHost>()
        .and_then(|host| host.controller.read(cx).installing.clone())
}

pub(crate) fn update_error(id: &str, cx: &App) -> Option<String> {
    cx.try_global::<PluginUpdateHost>()
        .and_then(|host| host.controller.read(cx).errors.get(id).cloned())
}

fn update_errors(cx: &App) -> BTreeMap<String, String> {
    cx.try_global::<PluginUpdateHost>()
        .map(|host| host.controller.read(cx).errors.clone())
        .unwrap_or_default()
}

/// Downloads the newer copy found for an installed plugin.
pub(crate) fn install_available(id: &str, cx: &mut App) {
    controller(cx).update(cx, |this, cx| {
        let Some(dir) = this.plugins_dir.clone() else {
            return;
        };
        let Some(update) = this.updates.get(id).cloned() else {
            return;
        };
        this.install(dir, update.entry, cx);
    });
}

fn install_plugin(dir: PathBuf, entry: CatalogEntry, cx: &mut App) {
    controller(cx).update(cx, |this, cx| this.install(dir, entry, cx));
}

fn controller(cx: &mut App) -> Entity<PluginUpdateController> {
    if let Some(host) = cx.try_global::<PluginUpdateHost>() {
        return host.controller.clone();
    }
    let controller = cx.new(PluginUpdateController::new);
    cx.set_global(PluginUpdateHost {
        controller: controller.clone(),
    });
    controller
}

fn installed_copies(dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let Some(id) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !valid_plugin_id(&id) {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }
        found.push((id, version_on_disk(&path)));
    }
    found.sort();
    found
}

fn select_updates(
    installed: &[(String, String)],
    entries: &[CatalogEntry],
    remotes: &BTreeMap<String, Option<String>>,
) -> Vec<TrackedUpdate> {
    let mut updates = Vec::new();
    for (id, installed_version) in installed {
        let Some(remote) = remotes.get(id).and_then(|version| version.clone()) else {
            continue;
        };
        if !remote_is_newer(&remote, installed_version) {
            continue;
        }
        let Some(entry) = entries.iter().find(|entry| entry.id == *id) else {
            continue;
        };
        let mut entry = entry.clone();
        entry.version = Some(remote.clone());
        updates.push(TrackedUpdate {
            id: id.clone(),
            name: entry.name.clone(),
            available: remote,
            entry,
        });
    }
    updates
}

fn still_newer(dir: &Path, update: &TrackedUpdate) -> bool {
    remote_is_newer(&update.available, &version_on_disk(&dir.join(&update.id)))
}

fn look_for_updates(dir: &Path) -> Result<Vec<TrackedUpdate>, String> {
    let installed = installed_copies(dir);
    if installed.is_empty() {
        return Ok(Vec::new());
    }
    let catalog = fetch_catalog_list()?;
    let mut remotes = BTreeMap::new();
    for (id, _) in &installed {
        let Some(entry) = catalog.entries.iter().find(|entry| entry.id == *id) else {
            continue;
        };
        let version = match remote_manifest(entry) {
            RemoteManifest::Match { version } => version,
            RemoteManifest::Unread | RemoteManifest::Mismatch => None,
        };
        remotes.insert(id.clone(), version);
    }
    Ok(select_updates(&installed, &catalog.entries, &remotes))
}

fn announce_available(updates: &[TrackedUpdate], cx: &mut App) {
    let message = if updates.len() == 1 {
        format!("An update is available for {}.", updates[0].name)
    } else {
        format!("{} plugin updates are available.", updates.len())
    };
    let mut note = Notification::info(message).id::<PluginUpdateNotice>();
    if updates.len() == 1 {
        let id = updates[0].id.clone();
        note = note.action(move |_, _, _| {
            let id = id.clone();
            Button::new("install-plugin-update")
                .label("Update")
                .on_click(move |_, _, cx| install_available(&id, cx))
        });
    } else {
        note = note.action(|_, _, _| {
            Button::new("review-plugin-updates")
                .label("Review")
                .on_click(|_, window, cx| crate::settings_dialog::open_settings(window, cx))
        });
    }
    push_note(note, cx);
}

fn push_note(note: Notification, cx: &mut App) {
    let Some(window) = cx.active_window() else {
        log::info!("plugin update notice");
        return;
    };
    if let Err(error) = window.update(cx, |_, window, cx| window.push_notification(note, cx)) {
        log::info!("{error:#}");
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{Cursor, Write},
        path::PathBuf,
    };

    use super::{
        CatalogEntry, FolderKind, RowControl, StoreFilter, archive_url, github_url,
        install_archive, installed_copies, manifest_url, matches_filter, matches_query,
        parse_catalog, readme_url, remote_is_newer, row_plan, select_updates, uninstall_plugin,
    };

    fn entry(branch: &str) -> CatalogEntry {
        CatalogEntry {
            id: "daily-notes".into(),
            name: "Daily notes".into(),
            author: "Notia".into(),
            description: String::new(),
            repo: "Chakyiu/notia-daily-note".into(),
            branch: branch.into(),
            version: None,
        }
    }

    fn sample_zip(files: &[(&str, &str)]) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, contents) in files {
                writer.start_file(*name, options).unwrap();
                writer.write_all(contents.as_bytes()).unwrap();
            }
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }

    fn plugin_zip(id: &str) -> Vec<u8> {
        let manifest =
            format!(r#"{{"id":"{id}","name":"Daily notes","version":"0.2.0","main":"main.js"}}"#);
        sample_zip(&[
            ("plugin-master/manifest.json", manifest.as_str()),
            ("plugin-master/main.js", "export default class {}"),
            ("plugin-master/lib/util.js", "export const n = 1"),
            ("plugin-master/theme.css", ".theme { color: #fff; }"),
            ("plugin-master/README.md", "docs"),
            ("plugin-master/test/util.test.js", "test"),
            ("plugin-master/notia.d.ts", "types"),
            ("plugin-master/data.json", r#"{"shipped":true}"#),
            ("plugin-master/.gitignore", "node_modules"),
            ("../outside.txt", "nope"),
        ])
    }

    fn temp_plugins(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "notia-catalog-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("plugins")).unwrap();
        dir
    }

    #[test]
    fn parses_the_list_and_skips_entries_it_will_not_download() {
        let catalog = parse_catalog(
            r#"[
                {"id":"daily-notes","name":"Daily notes","author":"Notia","description":"Days","repo":"Chakyiu/notia-daily-note","branch":"master"},
                {"id":"templater","name":"Templater","repo":"Chakyiu/notia-templater"},
                {"id":"nope","name":"Nope","repo":"https://example.com/plugin"},
                {"id":"daily-notes","name":"Again","repo":"Chakyiu/notia-daily-note"}
            ]"#,
        )
        .unwrap();
        assert_eq!(catalog.entries.len(), 2);
        assert_eq!(catalog.skipped, 2);
        assert_eq!(catalog.entries[0].branch, "master");
        assert_eq!(catalog.entries[1].branch, "main");
        assert!(catalog.entries[0].version.is_none());
    }

    #[test]
    fn rejects_a_list_that_is_not_json() {
        assert!(parse_catalog("not json").is_err());
    }

    #[test]
    fn archive_url_uses_the_repository_and_branch() {
        let daily = entry("master");
        assert_eq!(
            archive_url(&daily),
            "https://codeload.github.com/Chakyiu/notia-daily-note/zip/refs/heads/master"
        );
        assert_eq!(
            manifest_url(&daily),
            "https://raw.githubusercontent.com/Chakyiu/notia-daily-note/master/manifest.json"
        );
        assert_eq!(
            github_url(&daily),
            "https://github.com/Chakyiu/notia-daily-note"
        );
        assert_eq!(
            readme_url(&daily),
            "https://raw.githubusercontent.com/Chakyiu/notia-daily-note/master/README.md"
        );
    }

    #[test]
    fn installs_plugin_files_and_keeps_settings() {
        let dir = temp_plugins("install");
        let plugins = dir.join("plugins");
        let dest = plugins.join("daily-notes");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("data.json"), r#"{"folder":"Daily"}"#).unwrap();
        fs::write(dest.join("old.js"), "stale").unwrap();

        install_archive(&plugins, "daily-notes", &plugin_zip("daily-notes")).unwrap();

        assert_eq!(
            fs::read_to_string(dest.join("data.json")).unwrap(),
            r#"{"folder":"Daily"}"#
        );
        assert_eq!(
            fs::read_to_string(dest.join("main.js")).unwrap(),
            "export default class {}"
        );
        assert_eq!(
            fs::read_to_string(dest.join("lib/util.js")).unwrap(),
            "export const n = 1"
        );
        assert!(dest.join("theme.css").is_file());
        assert!(!dest.join("README.md").exists());
        assert!(!dest.join("test").exists());
        assert!(!dest.join("notia.d.ts").exists());
        assert!(!dest.join("old.js").exists());
        assert!(!dir.join("outside.txt").exists());
        assert!(!plugins.join(".install-daily-notes").exists());
        assert!(!plugins.join(".backup-daily-notes").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn leaves_an_install_alone_when_the_manifest_id_differs() {
        let dir = temp_plugins("mismatch");
        let plugins = dir.join("plugins");
        let dest = plugins.join("daily-notes");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("main.js"), "old").unwrap();

        let error = install_archive(&plugins, "daily-notes", &plugin_zip("other")).unwrap_err();
        assert!(error.contains("other"), "{error}");
        assert_eq!(fs::read_to_string(dest.join("main.js")).unwrap(), "old");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn refuses_to_replace_a_linked_folder() {
        let dir = temp_plugins("link");
        let source = dir.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("marker"), "keep").unwrap();
        std::os::unix::fs::symlink(&source, dir.join("plugins/daily-notes")).unwrap();

        let error = install_archive(
            &dir.join("plugins"),
            "daily-notes",
            &plugin_zip("daily-notes"),
        )
        .unwrap_err();
        assert!(error.contains("linked"), "{error}");
        assert_eq!(fs::read_to_string(source.join("marker")).unwrap(), "keep");
        assert!(!source.join("main.js").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn uninstall_removes_an_installed_plugin() {
        let dir = temp_plugins("uninstall");
        let plugins = dir.join("plugins");
        install_archive(&plugins, "daily-notes", &plugin_zip("daily-notes")).unwrap();
        uninstall_plugin(&plugins, "daily-notes").unwrap();
        assert!(!plugins.join("daily-notes").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn uninstall_removes_a_link_and_keeps_its_source() {
        let dir = temp_plugins("unlink");
        let source = dir.join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("marker"), "keep").unwrap();
        std::os::unix::fs::symlink(&source, dir.join("plugins/daily-notes")).unwrap();
        uninstall_plugin(&dir.join("plugins"), "daily-notes").unwrap();
        assert!(!dir.join("plugins/daily-notes").exists());
        assert_eq!(fs::read_to_string(source.join("marker")).unwrap(), "keep");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn plans_install_and_update() {
        assert_eq!(
            row_plan(FolderKind::Missing, false, "", Some("0.1.0")),
            RowControl::Install
        );
        assert_eq!(
            row_plan(FolderKind::Directory, true, "0.1.0", Some("0.1.0")),
            RowControl::Installed
        );
        assert_eq!(
            row_plan(FolderKind::Directory, true, "0.1.0", Some("0.2.0")),
            RowControl::Update
        );
        assert_eq!(
            row_plan(FolderKind::Linked, true, "0.1.0", Some("0.2.0")),
            RowControl::Linked
        );
        assert!(!remote_is_newer("0.1.0", "0.1.0"));
        assert!(remote_is_newer("0.2.0", "0.1.9"));
        assert!(remote_is_newer("0.1.0", ""));
    }

    #[test]
    fn filter_matches_install_and_update_rows() {
        assert!(matches_filter(RowControl::Install, StoreFilter::All));
        assert!(!matches_filter(RowControl::Install, StoreFilter::Installed));
        assert!(matches_filter(
            RowControl::Installed,
            StoreFilter::Installed
        ));
        assert!(matches_filter(RowControl::Update, StoreFilter::Updates));
        assert!(!matches_filter(RowControl::Installed, StoreFilter::Updates));
    }

    #[test]
    fn search_matches_the_name_description_and_author() {
        let plugin = CatalogEntry {
            id: "theme-enhancer".into(),
            name: "Theme enhancer".into(),
            author: "Notia".into(),
            description: "Folder icon packs and a CSS file.".into(),
            repo: "Chakyiu/notia-theme-enhancer".into(),
            branch: "main".into(),
            version: Some("0.1.0".into()),
        };
        assert!(matches_query(&plugin, ""));
        assert!(matches_query(&plugin, "theme"));
        assert!(matches_query(&plugin, "css"));
        assert!(matches_query(&plugin, "notia"));
        assert!(!matches_query(&plugin, "templater"));
    }

    #[test]
    fn installed_copies_skip_links_and_read_versions() {
        let dir = temp_plugins("copies");
        let plugins = dir.join("plugins");
        install_archive(&plugins, "daily-notes", &plugin_zip("daily-notes")).unwrap();
        fs::write(plugins.join("notes.txt"), "not a plugin").unwrap();
        let source = dir.join("source");
        fs::create_dir_all(&source).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&source, plugins.join("linked-plugin")).unwrap();

        let copies = installed_copies(&plugins);
        assert_eq!(copies, vec![("daily-notes".into(), "0.2.0".into())]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn selects_only_installed_plugins_with_a_newer_copy() {
        let daily = entry("main");
        let mut other = entry("main");
        other.id = "other".into();
        other.name = "Other".into();
        let installed = vec![
            ("daily-notes".into(), "0.1.0".into()),
            ("other".into(), "2.0.0".into()),
            ("local".into(), "0.1.0".into()),
        ];
        let mut remotes = std::collections::BTreeMap::new();
        remotes.insert("daily-notes".into(), Some("0.2.0".into()));
        remotes.insert("other".into(), Some("2.0.0".into()));
        remotes.insert("local".into(), Some("9.0.0".into()));
        let updates = select_updates(&installed, &[daily, other], &remotes);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].id, "daily-notes");
        assert_eq!(updates[0].available, "0.2.0");
    }
}
