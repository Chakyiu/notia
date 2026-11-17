//! The first-run window, and the app-level file that remembers the vault.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, TitleBar, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    v_flex,
};
use gpui_kit::{prelude::FluentBuilder as _, *};

/// The setup window, so it can be closed once the workspace opens.
struct SetupWindow(AnyWindowHandle);

impl Global for SetupWindow {}

/// Opens the window that asks for a vault folder.
pub(crate) fn open(cx: &mut App) {
    let options = window_options(cx);
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| Setup::new(window, cx))
    });
    let (window, _) = match opened {
        Ok(opened) => opened,
        Err(error) => {
            log::error!("could not open the setup window: {error:#}");
            cx.quit();
            return;
        }
    };
    cx.set_global(SetupWindow(window));
    if let Err(error) = window.update(cx, |_, window, _| {
        window.activate_window();
        window.set_window_title("Notia");
    }) {
        log::error!("could not show the setup window: {error:#}");
    }
}

/// Closes the setup window after the workspace has opened.
pub(crate) fn close(cx: &mut App) {
    let Some(window) = cx.try_global::<SetupWindow>().map(|setup| setup.0) else {
        return;
    };
    cx.defer(move |cx| {
        cx.remove_global::<SetupWindow>();
        _ = window.update(cx, |_, window, _| window.remove_window());
    });
}

fn window_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(640.), px(560.)), cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(480.), px(360.))),
        ..TitleBar::window_options()
    }
}

/// The vault Notia opens on launch: the default, or the one opened last time.
pub fn remembered_vault() -> Option<PathBuf> {
    startup_vault(&config_file())
}

/// The vault chosen as the default, when that folder is still there.
pub(crate) fn default_vault() -> Option<PathBuf> {
    default_in(&config_file())
}

/// Makes `vault` the one Notia opens on the next launch. The vault that is
/// open now stays open.
pub(crate) fn set_default_vault(vault: &Path) {
    if let Err(error) = set_default_in(&config_file(), vault) {
        log::error!("could not save the default vault: {error}");
    }
}

/// Vaults opened before, most recent first, skipping folders that are gone.
pub(crate) fn known_vaults() -> Vec<PathBuf> {
    known_in(&config_file())
}

pub(crate) fn remember(vault: &Path) {
    if let Err(error) = remember_in(&config_file(), vault) {
        log::error!("could not remember the vault: {error}");
    }
}

/// Drops `vault` from the list. The one that is open stays, so the next
/// launch still has a folder to open.
pub(crate) fn forget_vault(vault: &Path) {
    if let Err(error) = forget_in(&config_file(), vault) {
        log::error!("could not forget the vault: {error}");
    }
}

/// Asks for a folder and opens it as the vault.
pub(crate) fn pick_vault(cx: &mut App) {
    if gpui_kit::is_mobile() {
        notify(
            "This device keeps its vault in the app's folder.".to_string(),
            cx,
        );
        return;
    }
    if cx.has_global::<VaultPick>() {
        return;
    }
    let start = cx
        .try_global::<notia_plugin::Notia>()
        .and_then(|_| {
            notia_plugin::Notia::vault(cx)
                .read(cx)
                .vault()
                .root()
                .parent()
                .map(|path| path.to_path_buf())
        })
        .filter(|path| path.is_dir())
        .unwrap_or_else(home_dir);
    let previous = std::env::current_dir().ok();
    if start.is_dir() {
        // The panel opens in the process directory the first time. Point it
        // at the vault's parent instead of the folder the app was launched from.
        let _ = std::env::set_current_dir(&start);
    }
    let receiver = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Open a vault folder".into()),
    });
    let task = cx.spawn(async move |cx| {
        let picked = match receiver.await {
            Ok(Ok(Some(mut paths))) => paths.pop(),
            Ok(Err(error)) => {
                restore_dir(previous);
                _ = cx.update(|cx| {
                    finish_pick(cx);
                    notify(format!("Could not open the folder picker: {error}"), cx);
                });
                return;
            }
            _ => None,
        };
        restore_dir(previous);
        _ = cx.update(|cx| {
            finish_pick(cx);
            if let Some(path) = picked.filter(|path| path.is_dir())
                && let Err(error) = crate::switch_to(path, cx)
            {
                notify(error, cx);
            }
        });
    });
    cx.set_global(VaultPick { _task: task });
}

/// Holds the folder-picker task so it is not cancelled when `pick_vault` returns.
struct VaultPick {
    _task: Task<()>,
}

impl Global for VaultPick {}

fn finish_pick(cx: &mut App) {
    // Dropping the task from inside it would cancel the rest of this update.
    cx.defer(|cx| {
        if cx.has_global::<VaultPick>() {
            cx.remove_global::<VaultPick>();
        }
    });
}

fn notify(message: String, cx: &mut App) {
    let Some(window) = cx.active_window() else {
        log::error!("{message}");
        return;
    };
    if let Err(error) = window.update(cx, |_, window, cx| {
        window.push_notification(message, cx);
    }) {
        log::error!("{error:#}");
    }
}

fn config_file() -> PathBuf {
    app_config_dir().join("config.json")
}

/// The vault a phone keeps in its own sandbox.
/// `NOTIA_VAULT` overrides it, which is how the Android app points at its files.
pub fn mobile_vault_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("NOTIA_VAULT").filter(|path| !path.is_empty()) {
        return PathBuf::from(path);
    }
    let home = home_dir();
    if cfg!(target_os = "ios") {
        home.join("Documents").join("Vault")
    } else if cfg!(target_os = "android") {
        home.join("Vault")
    } else {
        home.join("Notia")
    }
}

fn app_config_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    let Some(home) = home else {
        return PathBuf::from(".notia");
    };
    if cfg!(any(target_os = "macos", target_os = "ios")) {
        home.join("Library/Application Support/Notia")
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or(home)
            .join("Notia")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("notia")
    }
}

fn read_config(file: &Path) -> serde_json::Value {
    let value = fs::read_to_string(file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if value.is_object() {
        value
    } else {
        serde_json::json!({})
    }
}

fn write_config(file: &Path, value: &serde_json::Value) -> io::Result<()> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    fs::write(file, json)
}

fn push_path(paths: &mut Vec<PathBuf>, text: &str) {
    if text.is_empty() || paths.iter().any(|path| path.as_os_str() == text) {
        return;
    }
    paths.push(PathBuf::from(text));
}

fn config_paths(value: &serde_json::Value) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(vault) = value.get("vault").and_then(|vault| vault.as_str()) {
        push_path(&mut paths, vault);
    }
    if let Some(vaults) = value.get("vaults").and_then(|vaults| vaults.as_array()) {
        for vault in vaults {
            if let Some(vault) = vault.as_str() {
                push_path(&mut paths, vault);
            }
        }
    }
    paths
}

fn known_in(file: &Path) -> Vec<PathBuf> {
    config_paths(&read_config(file))
        .into_iter()
        .filter(|path| path.is_dir())
        .collect()
}

fn existing_dir(path: &str) -> Option<PathBuf> {
    let path = PathBuf::from(path);
    path.is_dir().then_some(path)
}

fn default_in(file: &Path) -> Option<PathBuf> {
    read_config(file)
        .get("default")
        .and_then(|vault| vault.as_str())
        .and_then(existing_dir)
}

/// The default vault when its folder is still there, otherwise the last one.
fn startup_vault(file: &Path) -> Option<PathBuf> {
    if let Some(path) = default_in(file) {
        return Some(path);
    }
    read_config(file)
        .get("vault")
        .and_then(|vault| vault.as_str())
        .and_then(existing_dir)
}

fn remember_in(file: &Path, vault: &Path) -> io::Result<()> {
    let mut value = read_config(file);
    let text = vault.to_string_lossy().into_owned();
    let mut vaults = Vec::new();
    push_path(&mut vaults, &text);
    if let Some(list) = value.get("vaults").and_then(|vaults| vaults.as_array()) {
        for entry in list {
            if let Some(entry) = entry.as_str() {
                push_path(&mut vaults, entry);
            }
        }
    }
    if let Some(previous) = value.get("vault").and_then(|vault| vault.as_str()) {
        push_path(&mut vaults, previous);
    }
    vaults.truncate(20);
    value["vault"] = serde_json::Value::String(text);
    value["vaults"] = serde_json::Value::Array(
        vaults
            .iter()
            .map(|path| serde_json::Value::String(path.to_string_lossy().into_owned()))
            .collect(),
    );
    write_config(file, &value)
}

fn set_default_in(file: &Path, vault: &Path) -> io::Result<()> {
    let mut value = read_config(file);
    let text = vault.to_string_lossy().into_owned();
    let mut vaults = Vec::new();
    if let Some(list) = value.get("vaults").and_then(|vaults| vaults.as_array()) {
        for entry in list {
            if let Some(entry) = entry.as_str() {
                push_path(&mut vaults, entry);
            }
        }
    }
    if let Some(previous) = value.get("vault").and_then(|vault| vault.as_str()) {
        push_path(&mut vaults, previous);
    }
    push_path(&mut vaults, &text);
    vaults.truncate(20);
    value["default"] = serde_json::Value::String(text);
    value["vaults"] = serde_json::Value::Array(
        vaults
            .iter()
            .map(|path| serde_json::Value::String(path.to_string_lossy().into_owned()))
            .collect(),
    );
    write_config(file, &value)
}

fn forget_in(file: &Path, vault: &Path) -> io::Result<()> {
    let mut value = read_config(file);
    let text = vault.to_string_lossy();
    if value.get("vault").and_then(|vault| vault.as_str()) == Some(text.as_ref()) {
        return Ok(());
    }
    if let Some(list) = value
        .get_mut("vaults")
        .and_then(|vaults| vaults.as_array_mut())
    {
        list.retain(|entry| entry.as_str() != Some(text.as_ref()));
    }
    if value.get("default").and_then(|vault| vault.as_str()) == Some(text.as_ref())
        && let Some(object) = value.as_object_mut()
    {
        object.remove("default");
    }
    write_config(file, &value)
}

/// Where a new vault is created unless the user picks another folder.
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

fn vault_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        None
    } else {
        Some(name.to_string())
    }
}

fn restore_dir(previous: Option<PathBuf>) {
    if let Some(path) = previous {
        let _ = std::env::set_current_dir(path);
    }
}

struct Setup {
    error: Option<SharedString>,
    picking: bool,
    /// The folder a new vault is created in. This starts at the home
    /// directory, never the folder the app was launched from.
    location: PathBuf,
    name: Entity<InputState>,
    _observe: Subscription,
    _task: Option<Task<()>>,
}

impl Setup {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).default_value("Notia"));
        let observe = cx.observe(&name, |_, _, cx| cx.notify());
        let focus = name.clone();
        window.defer(cx, move |window, cx| {
            focus.update(cx, |name, cx| {
                name.focus(window, cx);
                name.select_all(window, cx);
            });
        });
        Self {
            error: None,
            picking: false,
            location: home_dir(),
            name,
            _observe: observe,
            _task: None,
        }
    }

    fn destination(&self, cx: &App) -> Option<PathBuf> {
        let name = vault_name(&self.name.read(cx).value())?;
        Some(self.location.join(name))
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.destination(cx) else {
            self.fail("Enter a folder name without / or \\.".into(), cx);
            return;
        };
        self.open_folder(path, true, cx);
    }

    /// `open_when_chosen` opens the folder as a vault. Otherwise it becomes
    /// the parent of a new vault.
    fn choose(&mut self, prompt: &'static str, open_when_chosen: bool, cx: &mut Context<Self>) {
        if self.picking {
            return;
        }
        self.picking = true;
        self.error = None;
        let start = self.location.clone();
        let previous = std::env::current_dir().ok();
        if start.is_dir() {
            // The folder panel opens in the process directory when this app
            // has not shown one before. Launched from the project, that is
            // the project. Point it at the home directory instead.
            let _ = std::env::set_current_dir(&start);
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(prompt.into()),
        });
        self._task = Some(cx.spawn(async move |this, cx| {
            let picked = match receiver.await {
                Ok(Ok(Some(mut paths))) => paths.pop(),
                Ok(Err(error)) => {
                    restore_dir(previous);
                    _ = this.update(cx, |this, cx| {
                        this.fail(format!("Could not open the folder picker: {error}"), cx);
                    });
                    return;
                }
                _ => None,
            };
            restore_dir(previous);
            _ = this.update(cx, |this, cx| match picked {
                Some(path) if path.is_dir() && open_when_chosen => {
                    this.open_folder(path, false, cx)
                }
                Some(path) if path.is_dir() => {
                    this.location = path;
                    this.picking = false;
                    cx.notify();
                }
                Some(_) => this.fail("Choose a folder.".into(), cx),
                None => {
                    this.picking = false;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    fn open_folder(&mut self, path: PathBuf, seed_if_empty: bool, cx: &mut Context<Self>) {
        if let Err(error) = crate::open_at(&path, seed_if_empty, cx) {
            self.fail(error, cx);
        }
    }

    fn fail(&mut self, error: String, cx: &mut Context<Self>) {
        self.picking = false;
        self.error = Some(error.into());
        cx.notify();
    }
}

impl Render for Setup {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let traffic_lights = cfg!(target_os = "macos") && !window.is_fullscreen();
        let destination = self.destination(cx);
        let destination_text = match &destination {
            Some(path) if path.is_dir() => format!("Opens {}", path.display()),
            Some(path) => format!("Creates {}", path.display()),
            None => "Enter a folder name.".to_string(),
        };

        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.font_family.clone())
            .child(
                TitleBar::new().child(
                    div()
                        .when(traffic_lights, |this| this.pl(px(76.)))
                        .when(!traffic_lights, |this| this.pl_3())
                        .text_sm()
                        .child("Notia"),
                ),
            )
            .child(
                v_flex().flex_1().items_center().justify_center().child(
                    v_flex()
                        .w(px(360.))
                        .gap_4()
                        .items_stretch()
                        .text_center()
                        .child(
                            Icon::new(Lucide::FolderOpen)
                                .large()
                                .text_color(muted),
                        )
                        .child(
                            div()
                                .text_xl()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Welcome to Notia"),
                        )
                        .child(
                            div().text_sm().text_color(muted).child(
                                "Your notes are plain Markdown files in a folder on this computer. That folder is your vault.",
                            ),
                        )
                        .child(
                            v_flex().w_full().gap_1().text_left().child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("Name"),
                            )
                            .child(Input::new(&self.name)),
                        )
                        .child(
                            v_flex()
                                .w_full()
                                .gap_1()
                                .text_left()
                                .child(div().text_xs().text_color(muted).child("Location"))
                                .child(
                                    h_flex().gap_2().items_center().child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .px_2()
                                            .py_1()
                                            .rounded(theme.radius)
                                            .border_1()
                                            .border_color(theme.border)
                                            .text_sm()
                                            .truncate()
                                            .child(self.location.display().to_string()),
                                    )
                                    .child(
                                        Button::new("setup-location")
                                            .outline()
                                            .small()
                                            .label("Choose…")
                                            .disabled(self.picking)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.choose("Choose where to keep the vault", false, cx)
                                            })),
                                    ),
                                ),
                        )
                        .child(
                            div()
                                .w_full()
                                .text_left()
                                .text_xs()
                                .text_color(muted)
                                .child(destination_text),
                        )
                        .child(
                            v_flex()
                                .w_full()
                                .gap_2()
                                .pt_2()
                                .child(
                                    Button::new("setup-create")
                                        .primary()
                                        .label("Create vault")
                                        .icon(Icon::new(Lucide::FolderPlus))
                                        .disabled(self.picking || destination.is_none())
                                        .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
                                )
                                .child(
                                    Button::new("setup-open")
                                        .outline()
                                        .label("Open an existing folder")
                                        .icon(Icon::new(Lucide::FolderOpen))
                                        .disabled(self.picking)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.choose("Open a vault folder", true, cx)
                                        })),
                                ),
                        )
                        .when_some(self.error.clone(), |column, error| {
                            column.child(
                                div()
                                    .text_sm()
                                    .text_color(theme.danger)
                                    .child(error),
                            )
                        }),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use std::fs;

    use super::{
        forget_in, home_dir, known_in, read_config, remember_in, set_default_in, startup_vault,
        vault_name,
    };

    #[test]
    fn remembers_a_vault_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.json");
        let vault = dir.path().join("Notes");
        fs::create_dir(&vault).unwrap();
        remember_in(&file, &vault).unwrap();
        let stored = read_config(&file);
        assert_eq!(
            stored.get("vault").and_then(|vault| vault.as_str()),
            Some(vault.to_str().unwrap())
        );
        assert_eq!(known_in(&file), vec![vault]);
        assert!(known_in(&dir.path().join("missing.json")).is_empty());
    }

    #[test]
    fn remembers_several_vaults_and_keeps_the_open_one() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.json");
        let first = dir.path().join("Notes");
        let second = dir.path().join("Work");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        remember_in(&file, &first).unwrap();
        remember_in(&file, &second).unwrap();
        assert_eq!(known_in(&file), vec![second.clone(), first.clone()]);
        forget_in(&file, &first).unwrap();
        assert_eq!(known_in(&file), vec![second.clone()]);
        forget_in(&file, &second).unwrap();
        assert_eq!(known_in(&file), vec![second]);
    }

    #[test]
    fn the_default_vault_opens_on_the_next_launch() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.json");
        let first = dir.path().join("Notes");
        let second = dir.path().join("Work");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        remember_in(&file, &first).unwrap();
        remember_in(&file, &second).unwrap();
        assert_eq!(startup_vault(&file), Some(second.clone()));
        set_default_in(&file, &first).unwrap();
        assert_eq!(startup_vault(&file), Some(first.clone()));
        assert_eq!(
            read_config(&file)
                .get("vault")
                .and_then(|vault| vault.as_str()),
            Some(second.to_str().unwrap())
        );
        forget_in(&file, &first).unwrap();
        assert_eq!(startup_vault(&file), Some(second));
        assert!(read_config(&file).get("default").is_none());
    }

    #[test]
    fn a_new_vault_is_named_under_the_home_directory() {
        let home = home_dir();
        assert!(home.is_absolute());
        if let Some(env_home) = std::env::var_os("HOME") {
            assert_eq!(home, PathBuf::from(env_home));
        }
        assert_eq!(vault_name("  Notia "), Some("Notia".into()));
        assert_eq!(vault_name("notes/secret"), None);
        assert_eq!(vault_name(".."), None);
    }
}
