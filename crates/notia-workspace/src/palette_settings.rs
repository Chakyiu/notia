//! Settings for which commands the palette lists and which keys run them.

use gpui_kit::component::{
    IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    setting::{SettingField, SettingGroup, SettingItem, SettingPage},
    switch::Switch,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use notia_plugin::{PaletteCommand, hotkeys};

pub(crate) fn page(cx: &App) -> SettingPage {
    let mut groups = vec![
        SettingGroup::new()
            .title("Command palette")
            .description(
                "Choose which commands are listed. Click a shortcut, then press the new keys. Backspace clears a shortcut and Escape cancels.",
            )
            .item(
                SettingItem::new(
                    "Enable command palette",
                    SettingField::switch(
                        |cx: &App| hotkeys::is_enabled(cx),
                        |enabled: bool, cx: &mut App| hotkeys::set_enabled(enabled, cx),
                    )
                    .default_value(true),
                )
                .description("When this is off, the shortcut does not open the palette."),
            ),
    ];

    let mut by_group: Vec<(SharedString, Vec<PaletteCommand>)> = Vec::new();
    for command in hotkeys::commands(cx) {
        if let Some((_, commands)) = by_group
            .iter_mut()
            .find(|(group, _)| group == &command.group)
        {
            commands.push(command);
        } else {
            by_group.push((command.group.clone(), vec![command]));
        }
    }
    if let Some(index) = by_group
        .iter()
        .position(|(group, _)| group.as_ref() == "Notia")
    {
        let notia = by_group.remove(index);
        by_group.insert(0, notia);
    }

    for (group, mut commands) in by_group {
        commands.sort_by(|a, b| a.name.cmp(&b.name));
        let mut setting_group = SettingGroup::new().title(group);
        for command in commands {
            setting_group = setting_group.item(command_item(command, cx));
        }
        groups.push(setting_group);
    }

    SettingPage::new("Command palette")
        .icon(IconName::SquareTerminal)
        .groups(groups)
}

fn command_item(command: PaletteCommand, cx: &App) -> SettingItem {
    let id = command.id.clone();
    let description = match command.default_hotkey.as_deref() {
        Some(hotkey) => format!("Default {}", hotkeys::display_hotkey(hotkey, cx)),
        None => "No default shortcut.".to_string(),
    };
    let (shown_id, key_id, reset_id) = (id.clone(), id.clone(), id.clone());
    SettingItem::new(
        command.name.clone(),
        SettingField::render(move |_, _, cx| {
            let listed = hotkeys::is_listed(&shown_id, cx);
            let capturing = hotkeys::is_capturing(&key_id, cx);
            let label = if capturing {
                "Press shortcut…".to_string()
            } else {
                hotkeys::hotkey(&key_id, cx)
                    .map(|hotkey| hotkeys::display_hotkey(&hotkey, cx))
                    .unwrap_or_else(|| "None".to_string())
            };
            let (toggle_id, record_id, restore_id) =
                (shown_id.clone(), key_id.clone(), reset_id.clone());
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Switch::new(SharedString::from(format!("palette-show-{toggle_id}")))
                        .small()
                        .checked(listed)
                        .label("Show")
                        .tooltip("List this command in the palette")
                        .accessibility_label(format!(
                            "Show {} in the command palette",
                            command.name
                        ))
                        .on_change({
                            let id = toggle_id.clone();
                            move |shown, window, cx| {
                                hotkeys::set_listed(&id, *shown, cx);
                                window.refresh();
                            }
                        }),
                )
                .child(
                    div().min_w(px(132.)).child(
                        Button::new(SharedString::from(format!("palette-key-{record_id}")))
                            .outline()
                            .small()
                            .label(label)
                            .on_click({
                                let id = record_id.clone();
                                move |_, window, cx| {
                                    if hotkeys::is_capturing(&id, cx) {
                                        hotkeys::cancel_capture(cx);
                                    } else {
                                        hotkeys::start_capture(id.clone(), cx);
                                    }
                                    window.refresh();
                                }
                            }),
                    ),
                )
                .when(hotkeys::hotkey_is_custom(&restore_id, cx), |row| {
                    let id = restore_id.clone();
                    row.child(
                        Button::new(SharedString::from(format!("palette-reset-{id}")))
                            .ghost()
                            .small()
                            .label("Reset")
                            .tooltip("Use the default shortcut")
                            .on_click(move |_, window, cx| {
                                hotkeys::restore_hotkey(&id, cx);
                                window.refresh();
                            }),
                    )
                })
        })
        .on_reset(
            {
                let id = id.clone();
                move |cx| hotkeys::is_customized(&id, cx)
            },
            {
                let id = id.clone();
                move |window, cx| {
                    hotkeys::reset_command(&id, cx);
                    window.refresh();
                }
            },
        ),
    )
    .description(description)
    .keywords([id, "shortcut".into(), "hotkey".into(), "palette".into()])
}
