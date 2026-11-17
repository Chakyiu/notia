//! The vault switcher: the name at the bottom of the file sidebar, and the
//! Manage vault list opened from a note's menu.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::DialogFooter,
    h_flex,
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    scroll::ScrollableElement as _,
    v_flex,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use notia_plugin::{Notia, actions::ManageVaults};

/// The Lucide star with its interior filled. Icons are drawn as a mask, so the
/// fill has to be opaque; the button tints it.
const FILLED_STAR: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="#000" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M11.525 2.295a.53.53 0 0 1 .95 0l2.31 4.679a2.123 2.123 0 0 0 1.595 1.16l5.166.756a.53.53 0 0 1 .294.904l-3.736 3.638a2.123 2.123 0 0 0-.611 1.878l.882 5.14a.53.53 0 0 1-.771.56l-4.618-2.428a2.122 2.122 0 0 0-1.973 0L6.396 21.01a.53.53 0 0 1-.77-.56l.881-5.139a2.122 2.122 0 0 0-.611-1.879L2.16 9.795a.53.53 0 0 1 .294-.906l5.165-.755a2.122 2.122 0 0 0 1.597-1.16z"/></svg>"##;

/// How the shell lists, opens and forgets vaults. Installed at startup.
pub struct VaultHooks {
    pub list: fn() -> Vec<PathBuf>,
    pub open: fn(PathBuf, &mut App) -> Result<(), String>,
    pub forget: fn(&Path),
    pub pick: fn(&mut App),
    pub default_vault: fn() -> Option<PathBuf>,
    pub set_default: fn(&Path),
}

struct InstalledHooks(VaultHooks);

impl Global for InstalledHooks {}

pub fn install(hooks: VaultHooks, cx: &mut App) {
    cx.set_global(InstalledHooks(hooks));
}

fn hooks(cx: &App) -> Option<&VaultHooks> {
    cx.try_global::<InstalledHooks>().map(|hooks| &hooks.0)
}

fn vault_paths(cx: &App) -> Vec<PathBuf> {
    let mut paths = hooks(cx).map(|hooks| (hooks.list)()).unwrap_or_default();
    if let Some(current) = current_root(cx)
        && !paths.iter().any(|path| path == &current)
    {
        paths.insert(0, current);
    }
    paths
}

fn current_root(cx: &App) -> Option<PathBuf> {
    cx.try_global::<Notia>()
        .map(|_| Notia::vault(cx).read(cx).vault().root().to_path_buf())
}

fn vault_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Vault".to_string())
}

fn select(path: PathBuf, window: &mut Window, cx: &mut App) {
    let Some(open) = hooks(cx).map(|hooks| hooks.open) else {
        window.push_notification("Vaults are not available.", cx);
        return;
    };
    window.close_dialog(cx);
    window.defer(cx, move |window, cx| {
        if let Err(error) = open(path, cx) {
            window.push_notification(error, cx);
        }
    });
}

/// The menu of vaults. The current one is checked. Choosing another opens it.
pub(crate) fn vault_menu(menu: PopupMenu, cx: &App) -> PopupMenu {
    let current = current_root(cx);
    let pick = hooks(cx).map(|hooks| hooks.pick);
    let mut menu = menu;
    for path in vault_paths(cx) {
        let name = vault_name(&path);
        let selected = current.as_ref() == Some(&path);
        let path = path.clone();
        menu = menu.item(
            PopupMenuItem::new(name)
                .checked(selected)
                .on_click(move |_, window, cx| select(path.clone(), window, cx)),
        );
    }
    menu = menu.separator();
    if let Some(pick) = pick {
        menu = menu.item(
            PopupMenuItem::new("Open folder…")
                .icon(Icon::new(Lucide::FolderOpen))
                .on_click(move |_, _, cx| pick(cx)),
        );
    }
    menu
}

/// The bar under the file list, showing the vault name.
pub(crate) fn footer(cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (sidebar, border, muted) = (theme.sidebar, theme.sidebar_border, theme.muted_foreground);
    let name = current_root(cx)
        .as_deref()
        .map(vault_name)
        .unwrap_or_else(|| "Vault".to_string());

    h_flex()
        .flex_none()
        .w_full()
        .h(px(40.))
        .px_1()
        .gap_1()
        .items_center()
        .bg(sidebar)
        .border_t_1()
        .border_color(border)
        .text_color(theme.sidebar_foreground)
        .child(
            Button::new("vault-switch")
                .flex_1()
                .min_w_0()
                .justify_start()
                .icon(Icon::new(Lucide::ChevronsUpDown).text_color(muted))
                .label(name)
                .ghost()
                .small()
                .tooltip("Vaults")
                .dropdown_menu_with_anchor(Anchor::BottomLeft, |menu, _, cx| {
                    vault_menu(menu, cx).item(PopupMenuItem::new("Manage vault…").on_click(
                        |_, window, cx| {
                            window.dispatch_action(Box::new(ManageVaults), cx);
                        },
                    ))
                }),
        )
}

fn default_root(cx: &App) -> Option<PathBuf> {
    hooks(cx).and_then(|hooks| (hooks.default_vault)())
}

struct VaultManager {
    vaults: Vec<PathBuf>,
    current: Option<PathBuf>,
    default: Option<PathBuf>,
}

impl VaultManager {
    fn new(cx: &App) -> Self {
        Self {
            vaults: vault_paths(cx),
            current: current_root(cx),
            default: default_root(cx),
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.vaults = vault_paths(cx);
        self.current = current_root(cx);
        self.default = default_root(cx);
        cx.notify();
    }

    fn forget(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Some(forget) = hooks(cx).map(|hooks| hooks.forget) {
            forget(&path);
        }
        self.reload(cx);
    }

    fn set_default(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Some(set_default) = hooks(cx).map(|hooks| hooks.set_default) {
            set_default(&path);
        }
        self.reload(cx);
    }
}

impl Render for VaultManager {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, active, radius, star) = (
            theme.muted_foreground,
            theme.list_active,
            theme.radius,
            theme.yellow,
        );
        let current = self.current.clone();
        let default = self.default.clone();
        v_flex().w_full().gap_2().child(
            div().max_h(px(360.)).overflow_y_scrollbar().child(
                v_flex()
                    .gap_1()
                    .children(self.vaults.iter().enumerate().map(|(index, path)| {
                        let selected = current.as_ref() == Some(path);
                        let is_default = default.as_ref() == Some(path);
                        let name: SharedString = vault_name(path).into();
                        let location: SharedString = path
                            .parent()
                            .map(|parent| parent.display().to_string())
                            .unwrap_or_default()
                            .into();
                        let open_path = path.clone();
                        let forget_path = path.clone();
                        let default_path = path.clone();
                        h_flex()
                            .id(("vault-row", index))
                            .w_full()
                            .items_center()
                            .gap_1()
                            .px_1()
                            .py_0p5()
                            .rounded(radius)
                            .when(selected, |row| row.bg(active))
                            .child(
                                Button::new(("vault-default", index))
                                    .icon(if is_default {
                                        Icon::default().data(FILLED_STAR).text_color(star)
                                    } else {
                                        Icon::new(Lucide::Star).text_color(muted)
                                    })
                                    .ghost()
                                    .small()
                                    .tooltip(if is_default {
                                        "Default vault"
                                    } else {
                                        "Set as default"
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if !is_default {
                                            this.set_default(default_path.clone(), cx);
                                        }
                                    })),
                            )
                            .child(
                                v_flex()
                                    .id(("vault-open", index))
                                    .flex_1()
                                    .min_w_0()
                                    .cursor_pointer()
                                    .gap_0p5()
                                    .py_1()
                                    .on_click(cx.listener(move |_, _, window, cx| {
                                        select(open_path.clone(), window, cx);
                                    }))
                                    .child(div().truncate().child(name))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .truncate()
                                            .child(location),
                                    ),
                            )
                            .when(!selected, |row| {
                                row.child(
                                    Button::new(("vault-forget", index))
                                        .icon(Icon::new(Lucide::Trash).text_color(muted))
                                        .ghost()
                                        .small()
                                        .tooltip("Remove from this list")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.forget(forget_path.clone(), cx);
                                        })),
                                )
                            })
                    }))
                    .when(self.vaults.is_empty(), |column| {
                        column.child(
                            div()
                                .px_2()
                                .py_3()
                                .text_sm()
                                .text_color(muted)
                                .child("No vaults yet. Open a folder to add one."),
                        )
                    }),
            ),
        )
    }
}

/// The list of vaults. Choosing one opens it.
pub(crate) fn open_manager(window: &mut Window, cx: &mut App) {
    let manager = cx.new(|cx| VaultManager::new(cx));
    let mobile = gpui_kit::is_mobile();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Manage vault")
            .w(px(480.))
            .child(manager.clone())
            .footer(DialogFooter::new().when(!mobile, |footer| {
                footer.child(
                    Button::new("vault-open-folder")
                        .outline()
                        .label("Open folder…")
                        .icon(Icon::new(Lucide::FolderOpen))
                        .on_click(|_, _, cx| {
                            if let Some(pick) = hooks(cx).map(|hooks| hooks.pick) {
                                pick(cx);
                            }
                        }),
                )
            }))
    });
}
