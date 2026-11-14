use std::{cell::Cell, rc::Rc};

use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, WindowExt as _,
    command::{Command as CommandList, CommandItem, CommandState},
};
use gpui_kit::*;
use notia_plugin::{Command, Plugin, PluginContext, PluginManifest, theme::ThemeCatalog};

pub(crate) const THEMES: &[&str] = &[
    include_str!("../themes/nord.json"),
    include_str!("../themes/rose-pine.json"),
    include_str!("../themes/catppuccin.json"),
    include_str!("../themes/paper.json"),
];

/// Contributes more themes, and a picker that previews them as you move
/// through the list.
pub struct ThemeGalleryPlugin;

impl Plugin for ThemeGalleryPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new("theme-gallery", "Theme gallery").with_description(
            "Adds the Nord, Rosé Pine, Catppuccin and Paper themes, and a theme picker.",
        )
    }

    fn activate(&self, ctx: &mut PluginContext, _: &mut App) {
        for json in THEMES {
            if let Err(error) = ctx.add_theme_json(json) {
                log::error!("a bundled theme is invalid: {error:#}");
            }
        }
        ctx.add_command(
            Command::new("change-theme", "Change theme", pick_theme)
                .with_icon(IconName::Palette)
                .with_keybinding("secondary-k secondary-t"),
        );
        ctx.on_deactivate(|cx| ThemeCatalog::preview(None, cx));
    }
}

/// Opens a list of the installed themes. Moving the highlight previews a
/// theme; Enter keeps it and Escape restores the previous one.
fn pick_theme(window: &mut Window, cx: &mut App) {
    let state = cx.new(|cx| CommandState::new(window, cx));
    let confirmed = Rc::new(Cell::new(false));
    let focus_on_mount = Rc::new(Cell::new(true));

    window.open_dialog(cx, move |dialog, _, _| {
        let state = state.clone();
        let (close_confirmed, confirm_flag) = (confirmed.clone(), confirmed.clone());
        let focus_on_mount = focus_on_mount.clone();
        dialog
            .close_button(false)
            .p_0()
            .w(px(460.))
            .on_close(move |_, _, cx| {
                if !close_confirmed.get() {
                    ThemeCatalog::preview(None, cx);
                }
            })
            .content(move |content, window, cx| {
                if focus_on_mount.replace(false) {
                    let state = state.clone();
                    window.defer(cx, move |window, cx| {
                        state.update(cx, |state, cx| state.focus(window, cx))
                    });
                }
                let catalog = ThemeCatalog::global(cx);
                let selected = catalog.appearance().theme.clone();
                let packs = catalog.packs();
                let ids: Rc<Vec<String>> =
                    Rc::new(packs.iter().map(|pack| pack.id().to_string()).collect());
                let muted = cx.theme().muted_foreground;
                let items = packs.iter().map(|pack| {
                    let author = pack.manifest().author.clone().unwrap_or_default();
                    let name = pack.name().clone();
                    CommandItem::new()
                        .label(name.clone())
                        .icon(IconName::Palette)
                        .checked(pack.id().as_ref() == selected)
                        .keywords([author.to_string()])
                        .child(move |_, _| {
                            gpui_kit::component::h_flex()
                                .w_full()
                                .gap_2()
                                .child(Icon::new(IconName::Palette).text_color(muted))
                                .child(div().flex_1().child(name.clone()))
                                .child(div().text_xs().text_color(muted).child(author.clone()))
                        })
                });
                let (preview_ids, confirm_ids, confirmed) =
                    (ids.clone(), ids, confirm_flag.clone());
                content.child(
                    CommandList::new(&state)
                        .bordered(false)
                        .placeholder("Choose a theme…")
                        .max_h(px(360.))
                        .items(items)
                        .on_select(move |index, _, cx| {
                            if let Some(id) = preview_ids.get(index.row) {
                                ThemeCatalog::preview(Some(id), cx);
                            }
                        })
                        .on_confirm(move |index, window, cx| {
                            if let Some(id) = confirm_ids.get(index.row).cloned() {
                                confirmed.set(true);
                                ThemeCatalog::update_appearance(cx, |appearance| {
                                    appearance.theme = id
                                });
                                ThemeCatalog::preview(None, cx);
                            }
                            window.close_dialog(cx);
                        }),
                )
            })
    });
}
