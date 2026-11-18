//! The settings dialog.

use std::{cell::Cell, rc::Rc};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, AxisExt as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::Button,
    h_flex,
    input::{Input, InputEvent, InputState},
    setting::{
        NumberFieldOptions, RenderOptions, SettingField, SettingGroup, SettingItem, SettingPage,
        Settings,
    },
    switch::Switch,
    v_flex,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use notia_plugin::{
    AppSettings, Notia, PluginHost, PluginKind, Setting, SettingControl, SettingValue,
    SettingsSection, ViewMode,
    theme::{ColorScheme, ThemeCatalog},
};

pub(crate) fn open_settings(window: &mut Window, cx: &mut App) {
    if window.has_active_dialog(cx) {
        return;
    }
    window.open_dialog(cx, |dialog, window, cx| {
        let height = (window.viewport_size().height - px(120.)).min(px(680.));
        // Sync and the history it keeps sit under Files and links. Other
        // plugin pages come last, so enabling one leaves the pages above it.
        let (sync, plugins) = plugin_pages(cx);
        let mut pages = vec![
            appearance_page(cx),
            editor_page(cx),
            crate::palette_settings::page(cx),
            files_page(),
        ];
        pages.extend(sync);
        pages.extend([
            plugins_page(cx),
            crate::plugin_catalog::page(),
            about_page(cx),
        ]);
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
    SettingPage::new("────────────────")
        .resettable(false)
        .group(SettingGroup::new().item(SettingItem::render(|_, _, cx| {
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Settings added by plugins.")
        })))
}

/// The Sync and History pages, then every other plugin settings page.
fn plugin_pages(cx: &mut App) -> (Vec<SettingPage>, Vec<SettingPage>) {
    let mut sync = Vec::new();
    let mut history = Vec::new();
    let mut rest = Vec::new();
    for (manifest, tab) in PluginHost::global(cx).settings_tabs() {
        let title = tab.title().unwrap_or(manifest.name()).clone();
        let icon = tab
            .icon()
            .cloned()
            .unwrap_or_else(|| Icon::new(Lucide::Puzzle));
        let page = SettingPage::new(title)
            .icon(icon)
            .groups(tab.build(cx).iter().map(setting_group));
        match manifest.id().as_ref() {
            "sync" => sync.push(page),
            // History explains what sync writes, so it stays beside it
            // whatever order the core plugins register in.
            "history" => history.push(page),
            _ => rest.push(page),
        }
    }
    sync.extend(history);
    (sync, rest)
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
        SettingControl::Secret(value) => {
            let label = name.clone();
            SettingItem::new(
                name,
                SettingField::render(move |options, window, cx| {
                    secret_input(label.clone(), &value, options, window, cx)
                }),
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

/// The input for a secret, such as the sync token. Characters stay hidden
/// until the eye button shows them. The stored value is the real text either way.
fn secret_input(
    label: SharedString,
    value: &SettingValue<SharedString>,
    options: &RenderOptions,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let current = value.get(cx);
    let set_value = value.clone();
    let wide = !options.layout().is_horizontal();
    let state = window.use_keyed_state(
        SharedString::from(format!(
            "secret-input-{}-{}-{}",
            options.page_ix(),
            options.group_ix(),
            options.item_ix()
        )),
        cx,
        {
            let current = current.clone();
            move |window, cx| {
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .default_value(current)
                        .masked(true)
                });
                // Typing saves the real token. Showing or hiding it only
                // redraws the eye icon; the text itself does not change.
                let _events = cx.subscribe(&input, {
                    let set_value = set_value.clone();
                    move |_, input, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::Change) {
                            set_value.set(input.read(cx).value(), cx);
                        }
                    }
                });
                let masked = Rc::new(Cell::new(true));
                let _mask = cx.observe(&input, move |_, input, cx| {
                    let now = input.read(cx).presentation().is_masked();
                    if masked.get() != now {
                        masked.set(now);
                        cx.notify();
                    }
                });
                SecretInput {
                    input,
                    _events,
                    _mask,
                }
            }
        },
    );

    state.update(cx, |state, cx| {
        if state.input.read(cx).value() != current {
            state.input.update(cx, |input, cx| {
                input.set_value(current.clone(), window, cx);
            });
        }
    });
    let input = state.read(cx).input.clone();

    Input::new(&input)
        .aria_label(label)
        .mask_toggle()
        .disabled(options.is_disabled())
        .with_size(options.size())
        .map(|this| if wide { this.w_full() } else { this.w_64() })
        .into_any_element()
}

struct SecretInput {
    input: Entity<InputState>,
    _events: Subscription,
    _mask: Subscription,
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

fn editor_page(cx: &App) -> SettingPage {
    let mut views = vec![
        ("editing".into(), "Edit".into()),
        ("reading".into(), "Preview".into()),
    ];
    if AppSettings::global(cx).editor.split_view {
        views.insert(1, ("half".into(), "Split".into()));
    }
    SettingPage::new("Editor")
        .icon(IconName::FileText)
        .group(SettingGroup::new().items(vec![
            SettingItem::new(
                "Split view",
                SettingField::switch(
                    |cx: &App| AppSettings::global(cx).editor.split_view,
                    |value: bool, cx: &mut App| {
                        AppSettings::update(cx, |settings| {
                            settings.editor.split_view = value;
                            if !value && settings.editor.default_view == ViewMode::Half {
                                settings.editor.default_view = ViewMode::Editing;
                            }
                        });
                        cx.refresh_windows();
                    },
                ),
            )
            .description(
                "Shows the Markdown and the preview side by side. The button above a note then cycles edit, split, and preview.",
            ),
            SettingItem::new(
                "Default view for new tabs",
                SettingField::dropdown(
                    views,
                    |cx: &App| match AppSettings::global(cx).editor.opening_view() {
                        ViewMode::Half => "half".into(),
                        ViewMode::Reading => "reading".into(),
                        ViewMode::Editing => "editing".into(),
                    },
                    |value: SharedString, cx: &mut App| {
                        let view = match value.as_ref() {
                            "reading" => ViewMode::Reading,
                            "half" if AppSettings::global(cx).editor.split_view => ViewMode::Half,
                            _ => ViewMode::Editing,
                        };
                        AppSettings::update(cx, |settings| settings.editor.default_view = view);
                    },
                ),
            )
            .description("Edit shows the Markdown. Preview shows the rendered note."),
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
        ]))
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
            "Plugins installed in this vault. Notia looks for updates when it opens. Turn one on to load its code. Install more from Plugin store.",
        )
        .item(community_folder_item(
            host.command(RELOAD_COMMUNITY_PLUGINS).is_some(),
        ));
    for (manifest, _) in host.plugins() {
        if manifest.kind() != PluginKind::Core {
            continue;
        }
        let id = manifest.id().clone();
        let description = manifest.description().to_string();
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
        core = core.items(items);
    }
    community = community.item(installed_community_plugins(cx));
    SettingPage::new("Plugins")
        .icon(IconName::LayoutDashboard)
        .groups([core, community])
}

fn installed_community_plugins(cx: &App) -> SettingItem {
    let mut keywords: Vec<SharedString> = vec!["community".into(), "update".into()];
    for (manifest, _) in PluginHost::global(cx).plugins() {
        if manifest.kind() == PluginKind::Community {
            keywords.push(manifest.name().clone());
            keywords.push(manifest.id().clone());
        }
    }
    SettingItem::render(|options, window, cx| {
        let watched = crate::plugin_catalog::update_controller(cx);
        let _updates =
            window.use_keyed_state("notia-plugin-update-watch-plugins", cx, move |_, cx| {
                crate::plugin_catalog::PluginUpdateWatch::new(watched, cx)
            });
        let host = PluginHost::global(cx);
        let checking = crate::plugin_catalog::checking_plugin_updates(cx);
        let installing = crate::plugin_catalog::installing_plugin(cx);
        let rows: Vec<_> = host
            .plugins()
            .filter(|(manifest, _)| manifest.kind() == PluginKind::Community)
            .map(|(manifest, enabled)| {
                (
                    manifest.id().clone(),
                    manifest.name().clone(),
                    manifest.description().to_string(),
                    manifest.author().to_string(),
                    manifest.version().to_string(),
                    enabled,
                )
            })
            .collect();
        let errors: Vec<Option<String>> = rows
            .iter()
            .map(|(id, ..)| host.error(id).map(|error| error.to_string()))
            .collect();
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let mut list = v_flex().w_full().gap_4();
        if checking {
            list = list.child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("Checking for updates…"),
            );
        }
        for (row, error) in rows.into_iter().zip(errors) {
            let (id, name, description, author, version, enabled) = row;
            let details = [
                (!author.is_empty()).then(|| format!("by {author}")),
                (!version.is_empty()).then(|| format!("v{version}")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            let mut text = v_flex()
                .min_w_0()
                .flex_1()
                .gap_1()
                .child(div().text_sm().child(name.clone()));
            if !description.trim().is_empty() {
                text = text.child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(description.trim().to_string()),
                );
            }
            if !details.is_empty() {
                text = text.child(div().text_xs().text_color(muted).child(details));
            }
            if let Some(error) = error {
                let summary = error.lines().next().unwrap_or("").to_string();
                text = text.child(div().text_sm().text_color(danger).child(summary));
            }
            let available = crate::plugin_catalog::available_update(&id, cx);
            if let Some(update) = &available {
                text = text.child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(format!("Version {} is available.", update.available)),
                );
            }
            if let Some(error) = crate::plugin_catalog::update_error(&id, cx) {
                let summary = error.lines().next().unwrap_or("").to_string();
                text = text.child(div().text_sm().text_color(danger).child(summary));
            }
            let switch_id = id.clone();
            let update_id = id.clone();
            let this_install = installing.as_deref() == Some(id.as_ref());
            list = list.child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(text)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .when(available.is_some(), |row| {
                                row.child(
                                    Button::new(SharedString::from(format!(
                                        "update-plugin-{update_id}"
                                    )))
                                    .outline()
                                    .label(if this_install {
                                        "Updating…"
                                    } else {
                                        "Update"
                                    })
                                    .with_size(options.size())
                                    .disabled(installing.is_some())
                                    .on_click(
                                        move |_, _, cx| {
                                            crate::plugin_catalog::install_available(
                                                &update_id, cx,
                                            );
                                        },
                                    ),
                                )
                            })
                            .child(
                                Switch::new(SharedString::from(format!("community-enabled-{id}")))
                                    .checked(enabled)
                                    .with_size(options.size())
                                    .accessibility_label(format!("Turn on {name}"))
                                    .on_change(move |checked, _, cx| {
                                        PluginHost::set_enabled(&switch_id, *checked, cx);
                                    }),
                            ),
                    ),
            );
        }
        list
    })
    .keywords(keywords)
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
    let mut items = vec![SettingItem::render(|_, _, cx| {
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
    })];
    items.extend(crate::update::items());
    items.push(github_item());
    items.push(license_item());
    items.push(SettingItem::new(
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
    ));
    SettingPage::new("About")
        .icon(IconName::Info)
        .group(SettingGroup::new().items(items))
}

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

fn repository_url() -> &'static str {
    REPOSITORY.trim_end_matches('/')
}

fn github_item() -> SettingItem {
    let label = repository_url()
        .strip_prefix("https://")
        .unwrap_or(repository_url());
    SettingItem::new(
        "GitHub",
        SettingField::render(move |options, _, _| {
            Button::new("open-github")
                .outline()
                .icon(IconName::Github)
                .label(label)
                .with_size(options.size())
                .on_click(|_, _, cx| cx.open_url(repository_url()))
        }),
    )
    .keywords(["source", "repository"])
}

fn license_item() -> SettingItem {
    SettingItem::new(
        "License",
        SettingField::render(|options, _, _| {
            Button::new("open-license")
                .outline()
                .icon(IconName::FileText)
                .label(format!("{} License", env!("CARGO_PKG_LICENSE")))
                .with_size(options.size())
                .on_click(|_, _, cx| {
                    cx.open_url(&format!("{}/blob/master/LICENSE", repository_url()))
                })
        }),
    )
    .description(copyright_line())
    .keywords(["mit", "copyright"])
}

fn copyright_line() -> &'static str {
    include_str!("../../../LICENSE")
        .lines()
        .find(|line| line.starts_with("Copyright"))
        .unwrap_or("MIT License")
}

#[cfg(test)]
mod tests {
    use super::copyright_line;

    #[test]
    fn copyright_line_names_the_holder() {
        assert_eq!(copyright_line(), "Copyright (c) 2026 chakyiu");
    }
}
