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
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    setting::{RenderOptions, SettingGroup, SettingItem, SettingPage},
    text::TextView,
    v_flex,
};
use gpui_kit::*;
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

struct CatalogState {
    catalog: Option<Catalog>,
    error: Option<String>,
    loading: bool,
    task: Option<Task<()>>,
    busy: Option<String>,
    install_errors: BTreeMap<String, String>,
    query: Entity<InputState>,
    selected: Option<String>,
    readmes: BTreeMap<String, ReadmeBody>,
    /// The plugin whose README is being fetched. Dropping `readme_task`
    /// cancels that fetch.
    readme_for: Option<String>,
    readme_task: Option<Task<()>>,
}

fn render_store(options: &RenderOptions, window: &mut Window, cx: &mut App) -> AnyElement {
    let state = window.use_keyed_state("notia-plugin-store", cx, |window, cx| CatalogState {
        catalog: None,
        error: None,
        loading: false,
        task: None,
        busy: None,
        install_errors: BTreeMap::new(),
        query: cx.new(|cx| InputState::new(window, cx).placeholder("Search plugins")),
        selected: None,
        readmes: BTreeMap::new(),
        readme_for: None,
        readme_task: None,
    });
    let (catalog, error, loading, busy, install_errors, query_field, selected, readmes) = {
        let ui = state.read(cx);
        (
            ui.catalog.clone(),
            ui.error.clone(),
            ui.loading,
            ui.busy.clone(),
            ui.install_errors.clone(),
            ui.query.clone(),
            ui.selected.clone(),
            ui.readmes.clone(),
        )
    };
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
    let size = options.size();
    let controls_disabled = loading || busy.is_some();
    let refresh_label = if loading {
        "Loading…"
    } else if error.is_some() && catalog.is_none() {
        "Try again"
    } else {
        "Refresh"
    };
    let state_for_refresh = state.clone();

    let mut list = v_flex().w_full().gap_3().child(
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
            .child(
                Button::new("refresh-plugin-store")
                    .outline()
                    .icon(IconName::RefreshCw)
                    .label(refresh_label)
                    .with_size(size)
                    .disabled(controls_disabled || (catalog.is_none() && error.is_none()))
                    .on_click(move |_, _, cx| start_load(&state_for_refresh, cx)),
            ),
    );

    if let Some(error) = error {
        list = list.child(div().text_sm().text_color(danger).child(error));
    }

    let Some(catalog) = catalog else {
        return list.into_any_element();
    };
    if let Some(id) = selected.as_deref()
        && let Some(entry) = catalog.entries.iter().find(|entry| entry.id == id)
    {
        let known = known_plugin(&entry.id, cx);
        let folder = folder_kind(&plugins_dir, &entry.id);
        let installed_version = installed_version(&plugins_dir, &entry.id, known.as_ref());
        let control_kind = row_plan(
            folder,
            known.is_some(),
            &installed_version,
            entry.version.as_deref(),
        );
        return render_detail(
            entry,
            readmes.get(&entry.id),
            control_kind,
            busy.as_deref() == Some(entry.id.as_str()),
            controls_disabled,
            size,
            &state,
            plugins_dir,
            cx.theme().border,
            muted,
            danger,
            install_errors.get(&entry.id).map(String::as_str),
        )
        .into_any_element();
    }
    if catalog.skipped > 0 {
        list = list.child(
            div()
                .text_sm()
                .text_color(muted)
                .child(skipped_line(catalog.skipped)),
        );
    }
    if catalog.entries.is_empty() {
        return list
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("No plugins are listed."),
            )
            .into_any_element();
    }

    let mut shown = 0;
    for entry in catalog
        .entries
        .iter()
        .filter(|entry| matches_query(entry, &query))
    {
        let known = known_plugin(&entry.id, cx);
        let folder = folder_kind(&plugins_dir, &entry.id);
        let installed_version = installed_version(&plugins_dir, &entry.id, known.as_ref());
        let control_kind = row_plan(
            folder,
            known.is_some(),
            &installed_version,
            entry.version.as_deref(),
        );
        shown += 1;
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
        let hint = if folder == FolderKind::Linked {
            Some("Linked from another folder.")
        } else if matches!(control_kind, RowControl::Installed) && !loader_on {
            Some("Turn on Community plugins to load it.")
        } else {
            None
        };
        let error = install_errors.get(&entry.id).cloned();
        let this_busy = busy.as_deref() == Some(entry.id.as_str());

        let mut body = v_flex()
            .w_full()
            .gap_1()
            .child(div().text_sm().child(entry.name.clone()));
        if !description.is_empty() {
            body = body.child(div().text_sm().text_color(muted).child(description));
        }
        let meta = meta_line(&author, &version);
        if !meta.is_empty() {
            body = body.child(div().text_xs().text_color(muted).child(meta));
        }
        if matches!(control_kind, RowControl::Update)
            && let Some(remote) = entry.version.clone()
        {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(format!("Version {remote} is available.")),
            );
        }
        if let Some(hint) = hint {
            body = body.child(div().text_sm().text_color(muted).child(hint));
        }
        if let Some(error) = error {
            let summary = error.lines().next().unwrap_or("").to_string();
            body = body.child(div().text_sm().text_color(danger).child(summary));
        }
        let text = div()
            .id(SharedString::from(format!(
                "plugin-store-open-{}",
                entry.id
            )))
            .flex_1()
            .min_w_0()
            .cursor_pointer()
            .on_click({
                let state = state.clone();
                let entry = entry.clone();
                move |_, _, cx| open_detail(&state, entry.clone(), cx)
            })
            .child(body);

        let row = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_3()
            .child(text)
            .child(row_control(
                entry,
                control_kind,
                this_busy,
                controls_disabled,
                size,
                &state,
                plugins_dir.clone(),
            ));
        list = list.child(row);
    }
    if shown == 0 {
        let message = if query.trim().is_empty() {
            "No plugins are listed."
        } else {
            "No plugins match."
        };
        list = list.child(div().text_sm().text_color(muted).child(message));
    }
    list.into_any_element()
}

fn row_control(
    entry: &CatalogEntry,
    control: RowControl,
    this_busy: bool,
    controls_disabled: bool,
    size: gpui_kit::component::Size,
    state: &Entity<CatalogState>,
    plugins_dir: PathBuf,
) -> AnyElement {
    let mut actions = h_flex().gap_1().flex_shrink_0();
    if matches!(control, RowControl::Install | RowControl::Update) {
        let label = if this_busy {
            "Installing…"
        } else if matches!(control, RowControl::Update) {
            "Update"
        } else {
            "Install"
        };
        actions = actions.child(action_button(
            format!("install-{}", entry.id),
            if matches!(control, RowControl::Update) {
                Lucide::RefreshCw
            } else {
                Lucide::Download
            },
            label,
            false,
            controls_disabled,
            size,
            {
                let state = state.clone();
                let entry = entry.clone();
                let plugins_dir = plugins_dir.clone();
                move |cx| start_install(&state, plugins_dir.clone(), entry.clone(), cx)
            },
        ));
    }
    if matches!(
        control,
        RowControl::Installed | RowControl::Update | RowControl::Linked
    ) {
        let label = if this_busy {
            "Uninstalling…"
        } else {
            "Uninstall"
        };
        actions = actions.child(action_button(
            format!("uninstall-{}", entry.id),
            Lucide::Trash,
            label,
            true,
            controls_disabled,
            size,
            {
                let state = state.clone();
                let id = entry.id.clone();
                let plugins_dir = plugins_dir.clone();
                move |cx| start_uninstall(&state, plugins_dir.clone(), id.clone(), cx)
            },
        ));
    }
    actions.into_any_element()
}

fn action_button(
    name: String,
    icon: Lucide,
    label: &'static str,
    danger: bool,
    disabled: bool,
    size: gpui_kit::component::Size,
    on_click: impl Fn(&mut App) + 'static,
) -> Button {
    let button = Button::new(SharedString::from(name))
        .icon(icon)
        .tooltip(label)
        .accessibility_label(label)
        .with_size(size)
        .disabled(disabled)
        .on_click(move |_, _, cx| on_click(cx));
    if danger {
        button.danger()
    } else {
        button.outline()
    }
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
    error: Option<&str>,
) -> impl IntoElement {
    let meta = meta_line(&entry.author, entry.version.as_deref().unwrap_or(""));
    let github = github_url(entry);
    let back_state = state.clone();
    let mut overview = v_flex()
        .w_full()
        .gap_1()
        .pb_3()
        .border_b_1()
        .border_color(border)
        .child(div().text_sm().child(entry.name.clone()));
    if !entry.description.is_empty() {
        overview = overview.child(
            div()
                .text_sm()
                .text_color(muted)
                .child(entry.description.clone()),
        );
    }
    if !meta.is_empty() {
        overview = overview.child(div().text_xs().text_color(muted).child(meta));
    }
    if let Some(error) = error {
        let summary = error.lines().next().unwrap_or("").to_string();
        overview = overview.child(div().text_sm().text_color(danger).child(summary));
    }
    v_flex()
        .w_full()
        .gap_3()
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .child(
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
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new(SharedString::from(format!("github-{}", entry.id)))
                                .outline()
                                .icon(Lucide::Github)
                                .tooltip("GitHub")
                                .accessibility_label(format!("Open {} on GitHub", entry.name))
                                .with_size(size)
                                .on_click(move |_, _, cx| cx.open_url(&github)),
                        )
                        .child(row_control(
                            entry,
                            control,
                            this_busy,
                            controls_disabled,
                            size,
                            state,
                            plugins_dir,
                        )),
                ),
        )
        .child(overview)
        .child(readme_view(&entry.id, readme, muted, danger))
}

fn readme_view(id: &str, readme: Option<&ReadmeBody>, muted: Hsla, danger: Hsla) -> AnyElement {
    match readme {
        Some(ReadmeBody::Ready(text)) => TextView::markdown(
            SharedString::from(format!("plugin-readme-{id}")),
            text.clone(),
        )
        .into_any_element(),
        Some(ReadmeBody::Missing) => div()
            .text_sm()
            .text_color(muted)
            .child("This repository has no README.")
            .into_any_element(),
        Some(ReadmeBody::Failed(message)) => div()
            .text_sm()
            .text_color(danger)
            .child(message.clone())
            .into_any_element(),
        Some(ReadmeBody::Loading) | None => div()
            .text_sm()
            .text_color(muted)
            .child("Loading the README…")
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
    state.update(cx, |this, cx| {
        if this.loading || this.task.is_some() || this.busy.is_some() {
            return;
        }
        let id = entry.id.clone();
        this.busy = Some(id.clone());
        this.install_errors.remove(&id);
        this.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let bytes = download_archive(&entry)?;
                    install_archive(&plugins_dir, &entry.id, &bytes)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.task = None;
                this.busy = None;
                match result {
                    Ok(()) => {
                        this.install_errors.remove(&id);
                        notia_js::reload_community_plugins(cx);
                    }
                    Err(message) => {
                        log::warn!("could not install {id}: {message}");
                        this.install_errors.insert(id, message);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    });
}

fn start_uninstall(state: &Entity<CatalogState>, plugins_dir: PathBuf, id: String, cx: &mut App) {
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

fn fetch_catalog() -> Result<Catalog, String> {
    let body = http_get(CATALOG_URL, MAX_CATALOG_BYTES, Duration::from_secs(20))
        .map_err(|error| explain(error, "The plugin list was not found."))?;
    let text =
        String::from_utf8(body).map_err(|_| "The plugin list could not be read.".to_string())?;
    let mut catalog = parse_catalog(&text)?;
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::{Cursor, Write},
        path::PathBuf,
    };

    use super::{
        CatalogEntry, FolderKind, RowControl, archive_url, github_url, install_archive,
        manifest_url, matches_query, parse_catalog, readme_url, remote_is_newer, row_plan,
        uninstall_plugin,
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
}
