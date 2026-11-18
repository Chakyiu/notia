//! The command palette and the quick switcher.

use std::{cell::Cell, collections::HashSet, rc::Rc};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
    command::{Command, CommandItem},
    h_flex,
    kbd::Kbd,
};
use gpui_kit::*;
use notia_plugin::{
    AppSettings, OpenOptions, PaletteCommand, PluginHost,
    actions::{
        CloseTab, EditMode, HalfEditMode, ManageVaults, NewCanvas, NewKanban, NewNote, NextTab,
        OpenCommandPalette, OpenQuickSwitcher, OpenSettings, PreviewMode, PreviousTab, SaveNote,
        ToggleLeftSidebar, ToggleReadingView, ToggleRightSidebar, WORKSPACE_CONTEXT,
    },
    core::{EntryKind, VaultPath, fuzzy_score},
    entry_icon, hotkeys,
};

use crate::Workspace;

/// A row of the quick switcher.
#[derive(Clone, Debug)]
pub(crate) enum SwitcherMatch {
    Note(VaultPath),
    Create(String),
}

/// Registers built-in commands and binds their default shortcuts.
pub(crate) fn install(cx: &mut App) {
    hotkeys::set_core(
        vec![
            PaletteCommand::core::<OpenCommandPalette>(
                "Open command palette",
                Some("secondary-p"),
                Some(Icon::new(IconName::SquareTerminal)),
            ),
            PaletteCommand::core::<NewNote>(
                "Create new note",
                Some("secondary-n"),
                Some(Icon::new(IconName::Plus)),
            ),
            PaletteCommand::core::<NewCanvas>(
                "Create new canvas",
                None,
                Some(Icon::new(Lucide::Frame)),
            ),
            PaletteCommand::core::<NewKanban>(
                "Create new kanban",
                None,
                Some(Icon::new(Lucide::Kanban)),
            ),
            PaletteCommand::core::<OpenQuickSwitcher>(
                "Open quick switcher",
                Some("secondary-o"),
                Some(Icon::new(IconName::Search)),
            ),
            PaletteCommand::core::<EditMode>("Edit mode", None, Some(Icon::new(Lucide::Pencil))),
            PaletteCommand::core::<HalfEditMode>(
                "Split view",
                None,
                Some(Icon::new(Lucide::Columns2)),
            ),
            PaletteCommand::core::<PreviewMode>("Preview mode", None, Some(Icon::new(Lucide::Eye))),
            PaletteCommand::core::<ToggleReadingView>(
                "Toggle edit and preview",
                Some("secondary-e"),
                Some(Icon::new(IconName::BookOpen)),
            ),
            PaletteCommand::core::<ToggleLeftSidebar>(
                "Toggle left sidebar",
                Some("secondary-\\"),
                Some(Icon::new(IconName::PanelLeft)),
            ),
            PaletteCommand::core::<ToggleRightSidebar>(
                "Toggle right sidebar",
                Some("secondary-shift-\\"),
                Some(Icon::new(IconName::PanelRight)),
            ),
            PaletteCommand::core::<CloseTab>(
                "Close current tab",
                Some("secondary-w"),
                Some(Icon::new(IconName::Close)),
            ),
            PaletteCommand::core::<SaveNote>(
                "Save current note",
                Some("secondary-s"),
                Some(Icon::new(IconName::FileText)),
            ),
            PaletteCommand::core::<OpenSettings>(
                "Open settings",
                Some("secondary-,"),
                Some(Icon::new(IconName::Settings)),
            ),
            PaletteCommand::core::<ManageVaults>(
                "Manage vault",
                None,
                Some(Icon::new(IconName::FolderOpen)),
            ),
            PaletteCommand::core::<NextTab>(
                "Next tab",
                Some("ctrl-tab"),
                Some(Icon::new(Lucide::ArrowRight)),
            ),
            PaletteCommand::core::<PreviousTab>(
                "Previous tab",
                Some("ctrl-shift-tab"),
                Some(Icon::new(Lucide::ArrowLeft)),
            ),
        ],
        cx,
    );
    hotkeys::bind_core_defaults(cx);
    hotkeys::install_capture(cx);
    hotkeys::sync(cx);
}

/// A palette row with an icon, a label and the command's keybinding.
fn command_row(label: SharedString, icon: Option<Icon>, action: Box<dyn Action>) -> CommandItem {
    let hint_action = action.boxed_clone();
    let row_label = label.clone();
    CommandItem::new()
        .label(label)
        .action(action)
        .child(move |window, cx| {
            let muted = cx.theme().muted_foreground;
            h_flex()
                .w_full()
                .gap_2()
                .child(
                    icon.clone()
                        .unwrap_or_else(|| Icon::new(IconName::SquareTerminal))
                        .small()
                        .text_color(muted),
                )
                .child(div().flex_1().truncate().child(row_label.clone()))
                .children(Kbd::binding_for_action(
                    hint_action.as_ref(),
                    Some(WORKSPACE_CONTEXT),
                    window,
                ))
        })
}

impl Workspace {
    fn command_items(&self, cx: &App) -> Vec<CommandItem> {
        let mut rows: Vec<(SharedString, Option<Icon>, Box<dyn Action>)> = Vec::new();
        for command in hotkeys::commands(cx) {
            if !hotkeys::is_listed(&command.id, cx) {
                continue;
            }
            if !AppSettings::global(cx).editor.split_view
                && command.id.as_ref() == HalfEditMode::name_for_type()
            {
                continue;
            }
            if PluginHost::global(cx)
                .command(&command.id)
                .is_some_and(|plugin_command| !plugin_command.is_available(cx))
            {
                continue;
            }
            let label = if command.group.as_ref() == "Notia" {
                command.name.clone()
            } else {
                format!("{}: {}", command.group, command.name).into()
            };
            rows.push((label, command.icon.clone(), command.action()));
        }
        rows.sort_by_key(|row| row.0.to_lowercase());
        rows.into_iter()
            .map(|(label, icon, action)| command_row(label, icon, action))
            .collect()
    }

    pub(crate) fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !hotkeys::is_enabled(cx) {
            window.push_notification(
                "The command palette is turned off. Enable it in Settings → Command palette.",
                cx,
            );
            return;
        }
        if self.palette_open {
            self.palette
                .update(cx, |palette, cx| palette.focus(window, cx));
            return;
        }
        self.palette_open = true;
        self.palette
            .update(cx, |palette, cx| palette.set_query("", window, cx));

        let palette = self.palette.clone();
        let owner = cx.weak_entity();
        let focus_on_mount = Rc::new(Cell::new(true));
        window.open_dialog(cx, move |dialog, _, _| {
            let palette = palette.clone();
            let (close_owner, content_owner) = (owner.clone(), owner.clone());
            let focus_on_mount = focus_on_mount.clone();
            dialog
                .close_button(false)
                .p_0()
                .w(px(600.))
                .on_close(move |_, _, cx| {
                    _ = close_owner.update(cx, |this, _| this.palette_open = false);
                })
                .content(move |content, window, cx| {
                    if focus_on_mount.replace(false) {
                        let palette = palette.clone();
                        window.defer(cx, move |window, cx| {
                            palette.update(cx, |palette, cx| palette.focus(window, cx));
                        });
                    }
                    let items = content_owner
                        .read_with(cx, |this, cx| this.command_items(cx))
                        .unwrap_or_default();
                    let confirm_owner = content_owner.clone();
                    content.child(
                        Command::new(&palette)
                            .bordered(false)
                            .placeholder("Type a command…")
                            .max_h(px(420.))
                            .items(items)
                            .on_confirm(move |_, window, cx| {
                                _ = confirm_owner.update(cx, |this, _| this.palette_open = false);
                                window.close_dialog(cx);
                            }),
                    )
                })
        });
    }

    pub(crate) fn update_switcher_matches(&mut self, query: &str, cx: &App) {
        let query = query.trim();
        let notes = self.vault.read(cx).tree().notes();
        self.switcher_matches = if query.is_empty() {
            let existing: HashSet<&VaultPath> = notes.iter().collect();
            let mut seen = HashSet::new();
            self.recent
                .iter()
                .filter(|path| existing.contains(path))
                .chain(notes.iter())
                .filter(|path| seen.insert((*path).clone()))
                .take(200)
                .map(|path| SwitcherMatch::Note(path.clone()))
                .collect()
        } else {
            let mut scored: Vec<(i64, VaultPath)> = notes
                .into_iter()
                .filter_map(|path| {
                    let by_name = fuzzy_score(query, path.file_stem());
                    let by_path = fuzzy_score(query, path.without_markdown_extension())
                        .map(|score| score - 5);
                    by_name.max(by_path).map(|score| (score, path))
                })
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            let exact = scored
                .iter()
                .any(|(_, path)| path.file_stem().eq_ignore_ascii_case(query));
            let mut matches: Vec<SwitcherMatch> = scored
                .into_iter()
                .take(200)
                .map(|(_, path)| SwitcherMatch::Note(path))
                .collect();
            if !exact {
                matches.push(SwitcherMatch::Create(query.to_string()));
            }
            matches
        };
    }

    fn switcher_items(&self) -> Vec<CommandItem> {
        self.switcher_matches
            .iter()
            .map(|choice| match choice {
                SwitcherMatch::Note(path) => {
                    let name: SharedString = path.file_stem().to_string().into();
                    let folder: SharedString = path
                        .parent()
                        .filter(|parent| !parent.is_root())
                        .map(|parent| parent.to_string())
                        .unwrap_or_default()
                        .into();
                    let label = name.clone();
                    let icon = entry_icon(path, EntryKind::Note, false);
                    CommandItem::new().label(label).child(move |_, cx| {
                        let muted = cx.theme().muted_foreground;
                        h_flex()
                            .w_full()
                            .gap_2()
                            .child(Icon::new(icon).small().text_color(muted))
                            .child(div().truncate().child(name.clone()))
                            .child(
                                div()
                                    .flex_1()
                                    .truncate()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(folder.clone()),
                            )
                    })
                }
                SwitcherMatch::Create(name) => CommandItem::new()
                    .label(format!("Create “{name}”"))
                    .icon(IconName::Plus),
            })
            .collect()
    }

    pub(crate) fn open_quick_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.switcher_open {
            self.switcher
                .update(cx, |switcher, cx| switcher.focus(window, cx));
            return;
        }
        self.switcher_open = true;
        self.update_switcher_matches("", cx);
        self.switcher
            .update(cx, |switcher, cx| switcher.set_query("", window, cx));

        let switcher = self.switcher.clone();
        let owner = cx.weak_entity();
        let focus_on_mount = Rc::new(Cell::new(true));
        window.open_dialog(cx, move |dialog, _, _| {
            let switcher = switcher.clone();
            let (close_owner, content_owner) = (owner.clone(), owner.clone());
            let focus_on_mount = focus_on_mount.clone();
            dialog
                .close_button(false)
                .p_0()
                .w(px(600.))
                .on_close(move |_, _, cx| {
                    _ = close_owner.update(cx, |this, _| this.switcher_open = false);
                })
                .content(move |content, window, cx| {
                    if focus_on_mount.replace(false) {
                        let switcher = switcher.clone();
                        window.defer(cx, move |window, cx| {
                            switcher.update(cx, |switcher, cx| switcher.focus(window, cx));
                        });
                    }
                    let items = content_owner
                        .read_with(cx, |this, _| this.switcher_items())
                        .unwrap_or_default();
                    let (query_owner, confirm_owner) =
                        (content_owner.clone(), content_owner.clone());
                    content.child(
                        Command::new(&switcher)
                            .bordered(false)
                            .filterable(false)
                            .placeholder("Find or create a note…")
                            .max_h(px(420.))
                            .items(items)
                            .on_query(move |query, window, cx| {
                                _ = query_owner
                                    .update(cx, |this, cx| this.update_switcher_matches(query, cx));
                                window.refresh();
                            })
                            .on_confirm(move |index, window, cx| {
                                let choice = confirm_owner
                                    .update(cx, |this, _| {
                                        this.switcher_open = false;
                                        this.switcher_matches.get(index.row).cloned()
                                    })
                                    .ok()
                                    .flatten();
                                window.close_dialog(cx);
                                if let Some(choice) = choice {
                                    let owner = confirm_owner.clone();
                                    window.defer(cx, move |window, cx| {
                                        _ = owner.update(cx, |this, cx| {
                                            this.confirm_switcher(choice, window, cx)
                                        });
                                    });
                                }
                            }),
                    )
                })
        });
    }

    fn confirm_switcher(
        &mut self,
        choice: SwitcherMatch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match choice {
            SwitcherMatch::Note(path) => self.open_note(path, OpenOptions::new(), window, cx),
            SwitcherMatch::Create(name) => match self.create_named_note(&name, cx) {
                Ok(path) => self.open_note(path, OpenOptions::new().in_new_tab(), window, cx),
                Err(error) => {
                    window.push_notification(format!("Could not create “{name}”: {error}"), cx)
                }
            },
        }
    }
}
