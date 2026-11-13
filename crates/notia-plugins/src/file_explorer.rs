use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex,
    input::{Input, InputState},
    list::ListItem,
    menu::PopupMenuItem,
    tree::{TreeEvent, TreeItem, TreeState, tree},
    v_flex,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use notia_plugin::{
    AppSettings, Command, Notia, OpenOptions, Panel, PanelPlacement, Plugin, PluginContext,
    PluginHost, PluginManifest, VaultEvent,
    core::{EntryKind, FileEntry, VaultPath},
    workspace,
};

const PANEL_ID: &str = "file-explorer:files";

pub struct FileExplorerPlugin;

impl Plugin for FileExplorerPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new("file-explorer", "Files").with_description(
            "Browse the vault and open, create, rename or delete notes and folders.",
        )
    }

    fn activate(&self, ctx: &mut PluginContext, _: &mut App) {
        ctx.add_panel(
            Panel::new(
                "files",
                "Files",
                IconName::FolderClosed,
                PanelPlacement::Left,
                |window, cx| cx.new(|cx| FileExplorer::new(window, cx)),
            )
            .with_order(0),
        );
        ctx.add_command(
            Command::new(
                "reveal",
                "Reveal active file in navigation",
                |window, cx| {
                    workspace::show_panel(PANEL_ID, window, cx);
                },
            )
            .with_icon(IconName::FolderOpen)
            .available_when(|cx| Notia::workspace(cx).read(cx).active_path().is_some()),
        );
    }
}

/// The folder tree of the vault.
pub struct FileExplorer {
    tree: Entity<TreeState>,
    /// What each tree item is, by item id (its vault path).
    kinds: Rc<HashMap<SharedString, EntryKind>>,
    expanded: HashSet<SharedString>,
    active: Option<VaultPath>,
    _subscriptions: Vec<Subscription>,
}

impl FileExplorer {
    fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        let vault = Notia::vault(cx);
        let workspace = Notia::workspace(cx);
        let tree = cx.new(|cx| TreeState::new(cx));
        let subscriptions = vec![
            cx.subscribe(&vault, |this, _, event, cx| {
                if matches!(event, VaultEvent::TreeChanged | VaultEvent::Renamed { .. }) {
                    this.rebuild(cx);
                }
            }),
            cx.subscribe(&tree, |this, _, event: &TreeEvent, _| match event {
                TreeEvent::Expanded(id) => {
                    this.expanded.insert(id.clone());
                }
                TreeEvent::Collapsed(id) => {
                    this.expanded.remove(id);
                }
            }),
            cx.observe(&workspace, |this, workspace, cx| {
                let active = workspace.read(cx).active_path().cloned();
                if active != this.active {
                    this.active = active;
                    this.reveal_active(cx);
                }
            }),
        ];
        let mut explorer = Self {
            tree,
            kinds: Rc::default(),
            expanded: HashSet::new(),
            active: workspace.read(cx).active_path().cloned(),
            _subscriptions: subscriptions,
        };
        explorer.rebuild(cx);
        explorer
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let entries = Notia::vault(cx).read(cx).tree().entries().to_vec();
        let mut kinds = HashMap::new();
        let items = Self::items(&entries, &self.expanded, &mut kinds);
        self.kinds = Rc::new(kinds);
        self.tree.update(cx, |tree, cx| tree.set_items(items, cx));
        self.reveal_active(cx);
        cx.notify();
    }

    fn items(
        entries: &[FileEntry],
        expanded: &HashSet<SharedString>,
        kinds: &mut HashMap<SharedString, EntryKind>,
    ) -> Vec<TreeItem> {
        entries
            .iter()
            .map(|entry| {
                let id: SharedString = entry.path.to_string().into();
                kinds.insert(id.clone(), entry.kind);
                TreeItem::new(id.clone(), entry.display_name().to_string())
                    .expanded(expanded.contains(&id))
                    .children(Self::items(&entry.children, expanded, kinds))
            })
            .collect()
    }

    fn reveal_active(&mut self, cx: &mut Context<Self>) {
        let Some(path) = &self.active else {
            self.tree
                .update(cx, |tree, cx| tree.set_selected_index(None, cx));
            return;
        };
        let id: SharedString = path.to_string().into();
        self.tree.update(cx, |tree, cx| {
            tree.reveal_item(&id, ScrollStrategy::Center, cx);
            let ix = tree.index_of(&id);
            tree.set_selected_index(ix, cx);
        });
    }

    fn collapse_all(&mut self, cx: &mut Context<Self>) {
        self.expanded.clear();
        self.rebuild(cx);
    }

    fn new_folder(&mut self, parent: VaultPath, window: &mut Window, cx: &mut Context<Self>) {
        match Notia::vault(cx).update(cx, |vault, cx| vault.create_folder(&parent, "Untitled", cx))
        {
            Ok(folder) => {
                self.expanded.insert(parent.to_string().into());
                self.rename(folder, window, cx);
            }
            Err(error) => {
                window.push_notification(format!("Could not create a folder: {error}"), cx)
            }
        }
    }

    fn rename(&mut self, path: VaultPath, window: &mut Window, cx: &mut Context<Self>) {
        let current = if path.is_markdown() {
            path.file_stem()
        } else {
            path.file_name()
        }
        .to_string();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(current));
        let submit = {
            let input = input.clone();
            Rc::new(move |window: &mut Window, cx: &mut App| {
                let name = input.read(cx).value().trim().to_string();
                if name.is_empty() {
                    return false;
                }
                match Notia::vault(cx).update(cx, |vault, cx| vault.rename(&path, &name, cx)) {
                    Ok(_) => true,
                    Err(error) => {
                        window.push_notification(format!("Could not rename: {error}"), cx);
                        false
                    }
                }
            })
        };
        let focus_on_mount = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if focus_on_mount.replace(false) {
                input.update(cx, |input, cx| {
                    input.focus(window, cx);
                    input.select_all(window, cx);
                });
            }
            let submit = submit.clone();
            dialog
                .title("Rename")
                .w(px(420.))
                .child(Input::new(&input))
                .on_ok(move |_, window, cx| submit(window, cx))
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("rename-cancel").outline().label("Cancel")),
                        )
                        .child(
                            DialogAction::new()
                                .child(Button::new("rename-confirm").primary().label("Rename")),
                        ),
                )
        });
    }

    fn delete(&mut self, path: VaultPath, window: &mut Window, cx: &mut Context<Self>) {
        let name = if path.is_markdown() {
            path.file_stem()
        } else {
            path.file_name()
        };
        let title = SharedString::from(format!("Delete “{name}”?"));
        let trash = Rc::new(move |window: &mut Window, cx: &mut App| {
            if let Err(error) = Notia::vault(cx).update(cx, |vault, cx| vault.trash(&path, cx)) {
                window.push_notification(format!("Could not delete: {error}"), cx);
            }
        });
        if !AppSettings::global(cx).files.confirm_trash {
            trash(window, cx);
            return;
        }
        window.open_alert_dialog(cx, move |alert, _, _| {
            let trash = trash.clone();
            alert
                .title(title.clone())
                .description("It moves to the .trash folder of the vault.")
                .confirm()
                .ok_text("Delete")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    trash(window, cx);
                    true
                })
        });
    }

    fn render_row(
        entry: &gpui_kit::component::tree::TreeEntry,
        kind: EntryKind,
        cx: &App,
    ) -> ListItem {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let id = entry.item().id.clone();
        let path = VaultPath::new(&id);
        let indent = px(10. + 14. * entry.depth() as f32);
        let chevron = match kind {
            EntryKind::Folder if entry.is_expanded() => Some(IconName::ChevronDown),
            EntryKind::Folder => Some(IconName::ChevronRight),
            _ => None,
        };
        let icon = PluginHost::global(cx).navigation_icon(&path, kind, entry.is_expanded(), cx);
        ListItem::new(id.clone())
            .py_0p5()
            .pl(indent)
            .text_sm()
            .rounded(theme.radius)
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        div().w_4().flex_none().children(
                            chevron.map(|icon| Icon::new(icon).xsmall().text_color(muted)),
                        ),
                    )
                    .when_some(icon, |row, icon| row.child(icon.flex_none()))
                    .child(div().truncate().child(entry.item().label.clone()))
                    .when(kind == EntryKind::Attachment, |row| {
                        row.child(
                            div()
                                .flex_none()
                                .px_1()
                                .rounded_sm()
                                .text_xs()
                                .text_color(muted)
                                .bg(theme.muted)
                                .child(path.extension().unwrap_or_default().to_uppercase()),
                        )
                    }),
            )
            .when(kind != EntryKind::Folder, |item| {
                item.on_click(move |event, window, cx| {
                    let options = if event.modifiers().secondary(cx) {
                        OpenOptions::new().in_new_tab()
                    } else {
                        OpenOptions::new()
                    };
                    workspace::open_note(path.clone(), options, window, cx);
                })
            })
    }
}

impl Render for FileExplorer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let kinds = self.kinds.clone();
        let menu_kinds = self.kinds.clone();
        let explorer = cx.weak_entity();

        let toolbar = h_flex()
            .justify_center()
            .gap_1()
            .py_1()
            .child(
                Button::new("explorer-new-note")
                    .icon(Icon::new(Lucide::FilePlus))
                    .ghost()
                    .small()
                    .tooltip("New note")
                    .on_click(|_, window, cx| workspace::new_note(None, window, cx)),
            )
            .child(
                Button::new("explorer-new-folder")
                    .icon(Icon::new(Lucide::FolderPlus))
                    .ghost()
                    .small()
                    .tooltip("New folder")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.new_folder(VaultPath::root(), window, cx)
                    })),
            )
            .child(
                Button::new("explorer-collapse")
                    .icon(Icon::new(Lucide::ChevronsDownUp))
                    .ghost()
                    .small()
                    .tooltip("Collapse all")
                    .on_click(cx.listener(|this, _, _, cx| this.collapse_all(cx))),
            );

        let tree = tree(&self.tree, move |_, entry, _, _, cx| {
            let kind = kinds
                .get(&entry.item().id)
                .copied()
                .unwrap_or(EntryKind::Note);
            Self::render_row(entry, kind, cx)
        })
        .context_menu(move |_, entry, menu, _, _| {
            let id = entry.item().id.clone();
            let path = VaultPath::new(&id);
            let kind = menu_kinds.get(&id).copied().unwrap_or(EntryKind::Note);
            let explorer = explorer.clone();
            let menu = if kind == EntryKind::Folder {
                let (note_folder, folder_parent, explorer) =
                    (path.clone(), path.clone(), explorer.clone());
                menu.item(
                    PopupMenuItem::new("New note")
                        .icon(Icon::new(Lucide::FilePlus))
                        .on_click(move |_, window, cx| {
                            workspace::new_note(Some(note_folder.clone()), window, cx)
                        }),
                )
                .item(
                    PopupMenuItem::new("New folder")
                        .icon(Icon::new(Lucide::FolderPlus))
                        .on_click(move |_, window, cx| {
                            _ = explorer.update(cx, |this, cx| {
                                this.new_folder(folder_parent.clone(), window, cx)
                            });
                        }),
                )
            } else {
                let path = path.clone();
                menu.item(
                    PopupMenuItem::new("Open in new tab").on_click(move |_, window, cx| {
                        workspace::open_note(
                            path.clone(),
                            OpenOptions::new().in_new_tab(),
                            window,
                            cx,
                        );
                    }),
                )
            };
            let (rename_path, delete_path, reveal_path, copy_path) =
                (path.clone(), path.clone(), path.clone(), path);
            let (rename_owner, delete_owner) = (explorer.clone(), explorer);
            menu.separator()
                .item(
                    PopupMenuItem::new("Rename…").on_click(move |_, window, cx| {
                        _ = rename_owner
                            .update(cx, |this, cx| this.rename(rename_path.clone(), window, cx));
                    }),
                )
                .item(
                    PopupMenuItem::new("Delete")
                        .icon(IconName::Delete)
                        .on_click(move |_, window, cx| {
                            _ = delete_owner.update(cx, |this, cx| {
                                this.delete(delete_path.clone(), window, cx)
                            });
                        }),
                )
                .separator()
                .item(
                    PopupMenuItem::new(if cfg!(target_os = "macos") {
                        "Reveal in Finder"
                    } else {
                        "Show in system explorer"
                    })
                    .on_click(move |_, _, cx| {
                        let path = Notia::vault(cx).read(cx).vault().abs_path(&reveal_path);
                        cx.reveal_path(&path);
                    }),
                )
                .item(
                    PopupMenuItem::new("Copy path")
                        .icon(IconName::Copy)
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_path.to_string()));
                        }),
                )
        });

        v_flex()
            .size_full()
            .child(toolbar)
            .child(div().flex_1().min_h_0().px_1().child(tree))
    }
}
