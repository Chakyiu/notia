//! Assembles Notia: the theme system, the plugin host, the workspace and the
//! core plugins. The binary opens a window with it; the screenshot test
//! renders the same setup offscreen.

extern crate gpui_kit as gpui;

mod setup;
pub mod welcome;

use std::path::PathBuf;

use gpui_kit::*;
use notia_plugin::{Notia, actions::*, core::Vault};

pub use setup::{mobile_vault_dir, remembered_vault};

/// Every Lucide icon, so that any plugin can use any of them by
/// `gpui_kit::assets::IconName`.
pub use gpui_kit::assets::AllAssets as Assets;

actions!(notia_app, [Quit]);

/// GPUI is initialized. A second `gpui_kit::init` would bind every key again.
struct UiReady;
impl Global for UiReady {}

/// Quit is bound. Binding it twice would run the action twice.
struct QuitBound;
impl Global for QuitBound {}

fn ensure_gpui(cx: &mut App) {
    if !cx.has_global::<UiReady>() {
        gpui_kit::init(cx);
        cx.set_global(UiReady);
    }
}

fn bind_quit(cx: &mut App) {
    if cx.has_global::<QuitBound>() {
        return;
    }
    cx.set_global(QuitBound);
    let quit = notia_workspace::key_binding("secondary-q", Quit, None, cx);
    cx.bind_keys([quit]);
    cx.on_action(|_: &Quit, cx| cx.quit());
}

/// Sets everything up for `vault`. Open the window afterwards with
/// [`notia_workspace::open_window`], or use [`open_at`] to do both.
pub fn init(vault: Vault, cx: &mut App) {
    ensure_gpui(cx);
    boot(vault, cx);
}

/// Opens `folder` as the vault, remembers it, and shows the workspace.
/// When `seed_if_empty` is set, an empty folder receives the starter notes.
pub fn open_at(folder: &std::path::Path, seed_if_empty: bool, cx: &mut App) -> Result<(), String> {
    let vault = Vault::open(folder)
        .map_err(|error| format!("Could not open {}: {error}", folder.display()))?;
    if seed_if_empty
        && vault.scan().is_ok_and(|tree| tree.entries().is_empty())
        && let Err(error) = welcome::create(&vault)
    {
        log::warn!("could not create the starter notes: {error}");
    }
    setup::remember(vault.root());
    init(vault, cx);
    notia_workspace::open_window(notia_workspace::window_options(cx), cx)
        .map_err(|error| format!("Could not open the window: {error:#}"))?;
    setup::close(cx);
    Ok(())
}

/// Shows why the phone could not open its vault. The desktop asks for a
/// folder instead of using this.
pub fn open_failure(message: String, cx: &mut App) {
    ensure_gpui(cx);
    if !cx.has_global::<notia_plugin::theme::ThemeCatalog>() {
        notia_plugin::theme::init(cx);
        notia_plugins::install_bundled_theme(cx);
    }
    let message: SharedString = message.into();
    let opened = gpui_kit::open_window(WindowOptions::default(), cx, move |_, cx| {
        cx.new(|_| Failure { message })
    });
    if let Err(error) = opened {
        log::error!("could not show the startup error: {error:#}");
    }
}

struct Failure {
    message: SharedString,
}

impl Render for Failure {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::ActiveTheme as _;

        let theme = cx.theme();
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p_6()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.font_family.clone())
            .child(
                div()
                    .max_w(px(360.))
                    .text_sm()
                    .text_color(theme.danger)
                    .child(self.message.clone()),
            )
    }
}

/// Shows the window that asks where the vault should live.
pub fn open_setup(cx: &mut App) {
    ensure_gpui(cx);
    if !cx.has_global::<notia_plugin::theme::ThemeCatalog>() {
        notia_plugin::theme::init(cx);
        notia_plugins::install_bundled_theme(cx);
    }
    bind_quit(cx);
    setup::open(cx);
}

fn boot(vault: Vault, cx: &mut App) {
    if !cx.has_global::<notia_plugin::theme::ThemeCatalog>() {
        notia_plugin::theme::init(cx);
    }
    notia_workspace::vaults::install(
        notia_workspace::vaults::VaultHooks {
            list: setup::known_vaults,
            open: switch_to,
            forget: setup::forget_vault,
            pick: setup::pick_vault,
            default_vault: setup::default_vault,
            set_default: setup::set_default_vault,
        },
        cx,
    );
    notia_plugin::init(vault, cx);
    notia_workspace::init(cx);
    notia_plugins::register(cx);
    notia_js::register(cx);
    bind_quit(cx);
    cx.set_menus(menus());
}

/// Saves the open notes, then opens `folder` as the vault in a new window.
pub(crate) fn switch_to(folder: PathBuf, cx: &mut App) -> Result<(), String> {
    if !cx.has_global::<notia_plugin::PluginHost>() {
        return open_at(&folder, true, cx);
    }
    let vault = Vault::open(&folder)
        .map_err(|error| format!("Could not open {}: {error}", folder.display()))?;
    if vault.scan().is_ok_and(|tree| tree.entries().is_empty())
        && let Err(error) = welcome::create(&vault)
    {
        log::warn!("could not create the starter notes: {error}");
    }
    let root = vault.root().to_path_buf();
    if Notia::vault(cx).read(cx).vault().root() == root {
        return Ok(());
    }
    notia_workspace::persist_workspace(cx);
    setup::remember(&root);
    notia_plugin::replace_vault(vault, cx);
    notia_plugins::register(cx);
    notia_js::register(cx);
    notia_workspace::reopen_workspace(cx)
        .map_err(|error| format!("Could not open the window: {error:#}"))
}

fn menus() -> Vec<Menu> {
    use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
    vec![
        Menu {
            name: "Notia".into(),
            items: vec![
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::Separator,
                MenuItem::action("Quit Notia", Quit),
            ],
            disabled: false,
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Note", NewNote),
                MenuItem::action("Go to File…", OpenQuickSwitcher),
                MenuItem::Separator,
                MenuItem::action("Save", SaveNote),
                MenuItem::action("Close Tab", CloseTab),
                MenuItem::Separator,
                MenuItem::action("Manage Vault…", ManageVaults),
            ],
            disabled: false,
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::action("Undo", Undo),
                MenuItem::action("Redo", Redo),
                MenuItem::Separator,
                MenuItem::action("Cut", Cut),
                MenuItem::action("Copy", Copy),
                MenuItem::action("Paste", Paste),
                MenuItem::action("Select All", SelectAll),
            ],
            disabled: false,
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Command Palette…", OpenCommandPalette),
                MenuItem::action("Edit Mode", EditMode),
                MenuItem::action("Half Edit Mode", HalfEditMode),
                MenuItem::action("Preview Mode", PreviewMode),
                MenuItem::action("Toggle Edit and Preview", ToggleReadingView),
                MenuItem::Separator,
                MenuItem::action("Toggle Left Sidebar", ToggleLeftSidebar),
                MenuItem::action("Toggle Right Sidebar", ToggleRightSidebar),
            ],
            disabled: false,
        },
    ]
}
