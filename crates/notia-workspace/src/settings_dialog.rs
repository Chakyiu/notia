//! The settings dialog.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::Button,
    h_flex,
    setting::{NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage, Settings},
    v_flex,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use notia_plugin::{
    AppSettings, Notia, PluginHost, PluginKind, Setting, SettingControl, SettingsSection, ViewMode,
    theme::{ColorScheme, ThemeCatalog},
};

pub(crate) fn open_settings(window: &mut Window, cx: &mut App) {
    if window.has_active_dialog(cx) {
        return;
    }
    window.open_dialog(cx, |dialog, window, cx| {
        let height = (window.viewport_size().height - px(120.)).min(px(680.));
        // Plugin pages come last, so enabling a plugin leaves the others in place.
        let mut pages = vec![
            appearance_page(cx),
            editor_page(),
            files_page(),
            plugins_page(cx),
            about_page(cx),
        ];
        let plugins = plugin_pages(cx);
        if !plugins.is_empty() {
            pages.push(plugin_separator());
            pages.extend(plugins);
        }
        dialog.title("Settings").w(px(920.)).child(
            div()
                .h(height)
                .child(Settings::new("notia-settings").pages(pages)),
        )
    });
}

/// A rule between Notia's own pages and the pages plugins add. It is a page
/// only because the settings sidebar is a list of pages; nothing selects it
/// on purpose, and a search hides it.
fn plugin_separator() -> SettingPage {
    SettingPage::new("────────────────").resettable(false).group(
        SettingGroup::new().item(SettingItem::render(|_, _, cx| {
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Settings added by plugins.")
        })),
    )
}

/// A page for each settings tab of an enabled plugin.
fn plugin_pages(cx: &mut App) -> Vec<SettingPage> {
    let tabs = PluginHost::global(cx).settings_tabs();
    tabs.into_iter()
        .map(|(manifest, tab)| {
            let title = tab.title().unwrap_or(manifest.name()).clone();
            let icon = tab
                .icon()
                .cloned()
                .unwrap_or_else(|| Icon::new(Lucide::Puzzle));
            SettingPage::new(title)
                .icon(icon)
                .groups(tab.build(cx).iter().map(setting_group))
        })
        .collect()
}

fn setting_group(section: &SettingsSection) -> SettingGroup {
    let mut group = SettingGroup::new().items(section.items().iter().map(setting_item));
    if let Some(title) = section.title() {
        group = group.title(title.clone());
    }
    if let Some(description) = section.description() {
        group = group.description(description.clone());
    }
    group
}

fn setting_item(setting: &Setting) -> SettingItem {
    let name = setting.name().clone();
    let item = match setting.control().clone() {
        SettingControl::Toggle(value) => {
            let set = value.clone();
            SettingItem::new(
                name,
                SettingField::switch(
                    move |cx: &App| value.get(cx),
                    move |on: bool, cx: &mut App| set.set(on, cx),
                ),
            )
        }
        SettingControl::Text(value) => {
            let set = value.clone();
            SettingItem::new(
                name,
                SettingField::input(
                    move |cx: &App| value.get(cx),
                    move |text: SharedString, cx: &mut App| set.set(text, cx),
                ),
            )
        }
        SettingControl::Number { value, range, step } => {
            let set = value.clone();
            SettingItem::new(
                name,
                SettingField::number_input(
                    NumberFieldOptions {
                        min: *range.start(),
                        max: *range.end(),
                        step,
                    },
                    move |cx: &App| value.get(cx),
                    move |number: f64, cx: &mut App| set.set(number, cx),
                ),
            )
        }
        SettingControl::Choice { value, options } => {
            let set = value.clone();
            SettingItem::new(
                name,
                SettingField::dropdown(
                    options,
                    move |cx: &App| value.get(cx),
                    move |choice: SharedString, cx: &mut App| set.set(choice, cx),
                ),
            )
        }
        SettingControl::Custom(render) if name.is_empty() => {
            SettingItem::render(move |_, window, cx| render(window, cx))
        }
        SettingControl::Custom(render) => SettingItem::new(
            name,
            SettingField::render(move |_, window, cx| render(window, cx)),
        ),
    };
    match setting.description() {
        Some(description) => item.description(description.clone()),
        None => item,
    }
}

fn appearance_page(cx: &App) -> SettingPage {
    let catalog = ThemeCatalog::global(cx);
    let errors = catalog.errors().to_vec();
    let folder = catalog.folder().map(|folder| folder.to_path_buf());

    let mut options: Vec<(SharedString, SharedString)> = catalog
        .packs()
        .iter()
        .map(|pack| (pack.id().clone(), pack.name().clone()))
        .collect();
    let selected = catalog.appearance().theme.clone();
    if !options.iter().any(|(id, _)| id.as_ref() == selected) {
        options.insert(0, (selected.clone().into(), selected.into()));
    }
    let mut themes = SettingGroup::new().title("Themes").item(
        SettingItem::new(
            "Theme",
            SettingField::scrollable_dropdown(
                options,
                |cx: &App| ThemeCatalog::global(cx).appearance().theme.clone().into(),
                |theme: SharedString, cx: &mut App| {
                    ThemeCatalog::update_appearance(cx, |appearance| {
                        appearance.theme = theme.to_string()
                    });
                },
            ),
        )
        .description("Colors, fonts and note typography."),
    );
    themes = themes.item(
        SettingItem::new(
            "Themes folder",
            SettingField::render(move |options, _, _| {
                let folder = folder.clone();
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("reload-themes")
                            .outline()
                            .label("Reload")
                            .with_size(options.size())
                            .on_click(|_, _, cx| ThemeCatalog::reload_folder(cx)),
                    )
                    .child(
                        Button::new("open-themes-folder")
                            .outline()
                            .icon(IconName::FolderOpen)
                            .label("Open folder")
                            .with_size(options.size())
                            .on_click(move |_, _, cx| {
                                if let Some(folder) = &folder {
                                    cx.open_with_system(folder);
                                }
                            }),
                    )
            }),
        )
        .description("Drop a theme folder with a theme.json, or a .json file, into .notia/themes. Saved changes apply immediately."),
    );
    for error in errors {
        themes = themes.item(
            SettingItem::render(move |_, _, cx| {
                h_flex()
                    .gap_2()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(Icon::new(IconName::TriangleAlert).small())
                    .child(format!(
                        "{}: {}",
                        error
                            .path
                            .file_name()
                            .map(|n| n.to_string_lossy())
                            .unwrap_or_default(),
                        error.message
                    ))
            })
            .keywords(["error"]),
        );
    }

    SettingPage::new("Appearance")
        .icon(IconName::Palette)
        .default_open(true)
        .groups(vec![
            SettingGroup::new().title("Interface").items(vec![
                SettingItem::new(
                    "Base color scheme",
                    SettingField::dropdown(
                        vec![
                            ("system".into(), "Adapt to system".into()),
                            ("light".into(), "Light".into()),
                            ("dark".into(), "Dark".into()),
                        ],
                        |cx: &App| match ThemeCatalog::global(cx).appearance().scheme {
                            ColorScheme::System => "system".into(),
                            ColorScheme::Light => "light".into(),
                            ColorScheme::Dark => "dark".into(),
                        },
                        |value: SharedString, cx: &mut App| {
                            let scheme = match value.as_ref() {
                                "light" => ColorScheme::Light,
                                "dark" => ColorScheme::Dark,
                                _ => ColorScheme::System,
                            };
                            ThemeCatalog::update_appearance(cx, |appearance| {
                                appearance.scheme = scheme
                            });
                        },
                    ),
                )
                .description("Shows the light or the dark variant of the theme."),
                SettingItem::new(
                    "Interface size",
                    SettingField::number_input(
                        NumberFieldOptions {
                            min: 11.,
                            max: 22.,
                            ..Default::default()
                        },
                        |cx: &App| ThemeCatalog::global(cx).appearance().interface_font_size as f64,
                        |value: f64, cx: &mut App| {
                            ThemeCatalog::update_appearance(cx, |appearance| {
                                appearance.interface_font_size = value as f32
                            });
                        },
                    )
                    .default_value(15.),
                )
                .description("The base size of every control, in pixels."),
            ]),
            SettingGroup::new().title("Notes").items(vec![
                SettingItem::new(
                    "Note font size",
                    SettingField::number_input(
                        NumberFieldOptions {
                            min: 10.,
                            max: 32.,
                            ..Default::default()
                        },
                        |cx: &App| {
                            let catalog = ThemeCatalog::global(cx);
                            catalog
                                .appearance()
                                .editor_font_size
                                .map(|size| size as f64)
                                .unwrap_or_else(|| {
                                    notia_plugin::theme::EditorAppearance::global(cx)
                                        .font_size
                                        .as_f32() as f64
                                })
                        },
                        |value: f64, cx: &mut App| {
                            ThemeCatalog::update_appearance(cx, |appearance| {
                                appearance.editor_font_size = Some(value as f32)
                            });
                        },
                    ),
                )
                .description("Replaces the size the theme chose."),
                SettingItem::new(
                    "Note font",
                    SettingField::input(
                        |cx: &App| {
                            ThemeCatalog::global(cx)
                                .appearance()
                                .editor_font_family
                                .clone()
                                .unwrap_or_default()
                                .into()
                        },
                        |value: SharedString, cx: &mut App| {
                            let family = value.trim().to_string();
                            ThemeCatalog::update_appearance(cx, |appearance| {
                                appearance.editor_font_family =
                                    (!family.is_empty()).then_some(family)
                            });
                        },
                    ),
                )
                .description("An installed font family. Leave empty to use the theme's font."),
                SettingItem::new(
                    "Readable line length",
                    SettingField::switch(
                        |cx: &App| ThemeCatalog::global(cx).appearance().readable_line_length,
                        |value: bool, cx: &mut App| {
                            ThemeCatalog::update_appearance(cx, |appearance| {
                                appearance.readable_line_length = value
                            });
                        },
                    ),
                )
                .description("Limits the width of note text to the theme's line length."),
            ]),
            themes,
        ])
}

fn editor_page() -> SettingPage {
    SettingPage::new("Editor").icon(IconName::FileText).group(
        SettingGroup::new().items(vec![
            SettingItem::new(
                "Default view for new tabs",
                SettingField::dropdown(
                    vec![
                        ("editing".into(), "Edit".into()),
                        ("half".into(), "Half edit".into()),
                        ("reading".into(), "Preview".into()),
                    ],
                    |cx: &App| match AppSettings::global(cx).editor.default_view {
                        ViewMode::Editing => "editing".into(),
                        ViewMode::Half => "half".into(),
                        ViewMode::Reading => "reading".into(),
                    },
                    |value: SharedString, cx: &mut App| {
                        let view = match value.as_ref() {
                            "reading" => ViewMode::Reading,
                            "half" => ViewMode::Half,
                            _ => ViewMode::Editing,
                        };
                        AppSettings::update(cx, |settings| settings.editor.default_view = view);
                    },
                ),
            )
            .description(
                "Edit shows the Markdown. Half edit shows both. Preview shows the rendered note.",
            ),
            SettingItem::new(
                "Show line numbers",
                SettingField::switch(
                    |cx: &App| AppSettings::global(cx).editor.line_numbers,
                    |value: bool, cx: &mut App| {
                        AppSettings::update(cx, |settings| settings.editor.line_numbers = value);
                    },
                ),
            ),
            SettingItem::new(
                "Save delay",
                SettingField::number_input(
                    NumberFieldOptions {
                        min: 100.,
                        max: 10_000.,
                        step: 100.,
                    },
                    |cx: &App| AppSettings::global(cx).editor.autosave_delay_ms as f64,
                    |value: f64, cx: &mut App| {
                        AppSettings::update(cx, |settings| {
                            settings.editor.autosave_delay_ms = value as u64
                        });
                    },
                ),
            )
            .description("Milliseconds after typing stops before the note is saved."),
        ]),
    )
}

fn files_page() -> SettingPage {
    SettingPage::new("Files and links")
        .icon(IconName::Folder)
        .group(SettingGroup::new().items(vec![
                SettingItem::new(
                    "Folder for new notes",
                    SettingField::input(
                        |cx: &App| AppSettings::global(cx).files.new_note_folder.clone().into(),
                        |value: SharedString, cx: &mut App| {
                            AppSettings::update(cx, |settings| {
                                settings.files.new_note_folder =
                                    value.trim().trim_matches('/').to_string()
                            });
                        },
                    ),
                )
                .description("Relative to the vault. Leave empty for the vault root."),
                SettingItem::new(
                    "Folder for pasted images",
                    SettingField::input(
                        |cx: &App| {
                            AppSettings::global(cx)
                                .files
                                .attachment_folder
                                .clone()
                                .into()
                        },
                        |value: SharedString, cx: &mut App| {
                            AppSettings::update(cx, |settings| {
                                settings.files.attachment_folder =
                                    value.trim().trim_matches('/').to_string()
                            });
                        },
                    ),
                )
                .description(
                    "Relative to the vault. Leave empty to save images in the vault root.",
                ),
                SettingItem::new(
                    "Confirm file deletion",
                    SettingField::switch(
                        |cx: &App| AppSettings::global(cx).files.confirm_trash,
                        |value: bool, cx: &mut App| {
                            AppSettings::update(cx, |settings| {
                                settings.files.confirm_trash = value
                            });
                        },
                    ),
                )
                .description("Deleted files move to the vault's .trash folder."),
            ]))
}

/// Commands of the core plugin that loads community plugins.
const RELOAD_COMMUNITY_PLUGINS: &str = "community-plugins:reload";
const OPEN_COMMUNITY_PLUGINS: &str = "community-plugins:open-folder";

fn plugins_page(cx: &App) -> SettingPage {
    let host = PluginHost::global(cx);
    let mut core = SettingGroup::new()
        .title("Core plugins")
        .description("Everything outside the editor is a plugin. Turn off what you don't use.");
    let mut community = SettingGroup::new()
        .title("Community plugins")
        .description(
            "Plugins you added. JavaScript plugins live in the vault's .notia/plugins folder, and their code loads only when you turn them on.",
        )
        .item(community_folder_item(
            host.command(RELOAD_COMMUNITY_PLUGINS).is_some(),
        ));
    for (manifest, _) in host.plugins() {
        let id = manifest.id().clone();
        let mut description = manifest.description().to_string();
        if manifest.kind() == PluginKind::Community {
            let details: Vec<String> = [
                (!manifest.author().is_empty()).then(|| format!("by {}", manifest.author())),
                (!manifest.version().is_empty()).then(|| format!("v{}", manifest.version())),
            ]
            .into_iter()
            .flatten()
            .collect();
            if !details.is_empty() {
                description = format!("{description}\n{}", details.join(" · "));
            }
        }
        let mut items = vec![
            SettingItem::new(
                manifest.name().clone(),
                SettingField::switch(
                    {
                        let id = id.clone();
                        move |cx: &App| PluginHost::global(cx).is_enabled(&id)
                    },
                    move |enabled: bool, cx: &mut App| PluginHost::set_enabled(&id, enabled, cx),
                ),
            )
            .description(description.trim().to_string())
            .keywords(["plugin"]),
        ];
        if let Some(error) = host.error(manifest.id()) {
            items.push(plugin_error_item(error.clone()));
        }
        match manifest.kind() {
            PluginKind::Core => core = core.items(items),
            PluginKind::Community => community = community.items(items),
        }
    }
    SettingPage::new("Plugins")
        .icon(IconName::LayoutDashboard)
        .groups([core, community])
}

fn community_folder_item(loader_enabled: bool) -> SettingItem {
    let run = |id: &'static str| {
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            if let Some(command) = PluginHost::global(cx).command(id) {
                command.run(window, cx);
            }
        }
    };
    SettingItem::new(
        "Plugins folder",
        SettingField::render(move |options, _, _| {
            h_flex()
                .gap_2()
                .child(
                    Button::new("reload-community-plugins")
                        .outline()
                        .label("Reload")
                        .with_size(options.size())
                        .disabled(!loader_enabled)
                        .on_click(run(RELOAD_COMMUNITY_PLUGINS)),
                )
                .child(
                    Button::new("open-community-plugins")
                        .outline()
                        .icon(IconName::FolderOpen)
                        .label("Open folder")
                        .with_size(options.size())
                        .disabled(!loader_enabled)
                        .on_click(run(OPEN_COMMUNITY_PLUGINS)),
                )
        }),
    )
    .description(if loader_enabled {
        "Put each plugin in a folder with a manifest.json and a main.js. Saving a plugin's files reloads it."
    } else {
        "Turn on Community plugins under Core plugins to load them."
    })
}

/// Why a plugin failed to load, under the plugin.
fn plugin_error_item(error: SharedString) -> SettingItem {
    SettingItem::render(move |_, _, cx| {
        let mut lines = error.lines();
        let summary = lines.next().unwrap_or_default().to_string();
        let details: Vec<&str> = lines.take(6).collect();
        h_flex()
            .gap_2()
            .items_start()
            .text_sm()
            .text_color(cx.theme().danger)
            .child(Icon::new(IconName::TriangleAlert).small().mt_0p5())
            .child(
                v_flex()
                    .min_w_0()
                    .child(summary)
                    .when(!details.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(details.join("\n")),
                        )
                    }),
            )
    })
    .keywords(["error"])
}

fn about_page(cx: &App) -> SettingPage {
    let vault = Notia::vault(cx).read(cx).vault().root().to_path_buf();
    SettingPage::new("About")
        .icon(IconName::Info)
        .group(SettingGroup::new().items(vec![
            SettingItem::render(|_, _, cx| {
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Notia"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("Version {}", env!("CARGO_PKG_VERSION"))),
                    )
            }),
            SettingItem::new(
                "Vault",
                SettingField::render(move |options, _, _| {
                    let vault = vault.clone();
                    Button::new("reveal-vault")
                        .outline()
                        .icon(IconName::FolderOpen)
                        .label(vault.display().to_string())
                        .with_size(options.size())
                        .on_click(move |_, _, cx| cx.open_with_system(&vault))
                }),
            ),
        ]))
}
