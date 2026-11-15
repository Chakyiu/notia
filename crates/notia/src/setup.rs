//! The first-run window, and the app-level file that remembers the vault.

use std::{
    fs,
    path::{Path, PathBuf},
};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, TitleBar,
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

/// The vault opened last time, when that folder is still there.
pub fn remembered_vault() -> Option<PathBuf> {
    let path = read_vault(&config_file())?;
    path.is_dir().then_some(path)
}

pub(crate) fn remember(vault: &Path) {
    if let Err(error) = write_vault(&config_file(), vault) {
        log::error!("could not remember the vault: {error}");
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

fn read_vault(file: &Path) -> Option<PathBuf> {
    let text = fs::read_to_string(file).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let path = value.get("vault")?.as_str()?;
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
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

fn write_vault(file: &Path, vault: &Path) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(&serde_json::json!({
        "vault": vault.to_string_lossy(),
    }))
    .map_err(std::io::Error::other)?;
    fs::write(file, json)
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

    use super::{home_dir, read_vault, vault_name, write_vault};

    #[test]
    fn remembers_a_vault_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.json");
        let vault = dir.path().join("Notes");
        write_vault(&file, &vault).unwrap();
        assert_eq!(read_vault(&file), Some(vault));
        assert_eq!(read_vault(&dir.path().join("missing.json")), None);
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
