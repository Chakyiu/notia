use std::{
    cell::Cell,
    collections::{BTreeMap, HashMap, HashSet},
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
    core::{EntryKind, FileEntry, FileTree, VaultPath},
    workspace,
};
use serde::{Deserialize, Serialize};

const PANEL_ID: &str = "file-explorer:files";
const ORDER_PLUGIN_ID: &str = "file-explorer";

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
    /// The row under a drag, and whether the drop goes before it, after it, or into it.
    drop_target: Option<(SharedString, DropPlace)>,
    _subscriptions: Vec<Subscription>,
}

impl FileExplorer {
    fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        let vault = Notia::vault(cx);
        let workspace = Notia::workspace(cx);
        let tree = cx.new(|cx| TreeState::new(cx));
        let subscriptions = vec![
            cx.subscribe(&vault, |this, _, event, cx| match event {
                VaultEvent::Renamed { from, to } => {
                    this.retarget_order(from, to, cx);
                    this.rebuild(cx);
                }
                VaultEvent::TreeChanged => this.rebuild(cx),
                _ => {}
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
            drop_target: None,
            _subscriptions: subscriptions,
        };
        explorer.rebuild(cx);
        explorer
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let mut entries = Notia::vault(cx).read(cx).tree().entries().to_vec();
        order_entries(&mut entries, "", &FileOrder::load(cx).folders);
        let mut kinds = HashMap::new();
        let items = Self::items(&entries, &self.expanded, &mut kinds);
        self.kinds = Rc::new(kinds);
        self.tree.update(cx, |tree, cx| tree.set_items(items, cx));
        self.reveal_active(cx);
        cx.notify();
    }

    /// Moves `path` among its siblings. `delta` is `-1` for up and `1` for down.
    /// Files and folders share one order; a folder that has never been arranged
    /// starts from the order it is showing now.
    fn move_by(&mut self, path: VaultPath, delta: isize, cx: &mut Context<Self>) {
        let tree = Notia::vault(cx).read(cx).tree().clone();
        let key = parent_key(&path);
        let name = path.file_name().to_string();
        let mut order = FileOrder::load(cx);
        let mut names = order
            .folders
            .get(&key)
            .cloned()
            .unwrap_or_else(|| sibling_names(&tree, &path));
        if !names.iter().any(|existing| existing == &name) {
            names.push(name.clone());
        }
        let Some(index) = names.iter().position(|existing| existing == &name) else {
            return;
        };
        let target = index as isize + delta;
        if target < 0 || target >= names.len() as isize {
            return;
        }
        names.swap(index, target as usize);
        order.folders.insert(key, names);
        order.save(cx);
        self.rebuild(cx);
    }

    /// Moves `dragged` into `folder` and opens that folder.
    fn drop_into(
        &mut self,
        dragged: VaultPath,
        folder: VaultPath,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drop_target = None;
        if folder.starts_with(&dragged) || dragged.parent().is_some_and(|parent| parent == folder) {
            cx.notify();
            return;
        }
        if let Err(error) =
            Notia::vault(cx).update(cx, |vault, cx| vault.move_into(&dragged, &folder, cx))
        {
            window.push_notification(format!("Could not move: {error}"), cx);
            cx.notify();
            return;
        }
        if !folder.is_root() {
            self.expanded.insert(folder.to_string().into());
        }
        self.rebuild(cx);
    }

    /// Drops `dragged` above or below `target`. A different parent moves the
    /// file out of its folder and into the target's folder first.
    fn drop_at(
        &mut self,
        dragged: VaultPath,
        target: VaultPath,
        after: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drop_target = None;
        if target.starts_with(&dragged) {
            cx.notify();
            return;
        }
        let mut placed = dragged.clone();
        if parent_key(&dragged) != parent_key(&target) {
            let destination = target.parent().unwrap_or_else(VaultPath::root);
            if destination.starts_with(&dragged) {
                cx.notify();
                return;
            }
            match Notia::vault(cx)
                .update(cx, |vault, cx| vault.move_into(&dragged, &destination, cx))
            {
                Ok(path) => placed = path,
                Err(error) => {
                    window.push_notification(format!("Could not move: {error}"), cx);
                    cx.notify();
                    return;
                }
            }
        }
        let tree = Notia::vault(cx).read(cx).tree().clone();
        let mut order = FileOrder::load(cx);
        let key = parent_key(&placed);
        let mut names = displayed_sibling_names(tree.entries(), &placed, &order.folders);
        place_name(&mut names, placed.file_name(), target.file_name(), after);
        order.folders.insert(key, names);
        order.save(cx);
        self.rebuild(cx);
    }

    fn retarget_order(&mut self, from: &VaultPath, to: &VaultPath, cx: &mut Context<Self>) {
        let mut order = FileOrder::load(cx);
        retarget_names(&mut order.folders, from, to);
        order.save(cx);
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
        edge: Option<DropPlace>,
        explorer: WeakEntity<Self>,
        cx: &App,
    ) -> ListItem {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let marker = theme.drag_border;
        let into_folder = theme.drop_target;
        let id = entry.item().id.clone();
        let path = VaultPath::new(&id);
        let indent = px(10. + 14. * entry.depth() as f32);
        let chevron = match kind {
            EntryKind::Folder if entry.is_expanded() => Some(IconName::ChevronDown),
            EntryKind::Folder => Some(IconName::ChevronRight),
            _ => None,
        };
        let icon = PluginHost::global(cx).navigation_icon(&path, kind, entry.is_expanded(), cx);
        let drag = ReorderDrag {
            path: path.clone(),
            label: entry.item().label.clone(),
        };
        let drop_path = path.clone();
        let drop_owner = explorer.clone();
        let move_id = id.clone();
        let move_path = path.clone();
        let move_folder = kind == EntryKind::Folder;
        let move_owner = explorer;
        ListItem::new(id.clone())
            .py_0p5()
            .pl(indent)
            .text_sm()
            .rounded(theme.radius)
            .on_drag(drag, |drag, _, _, cx| {
                let label = drag.label.clone();
                cx.new(|_| FileDragGhost { label })
            })
            .on_drag_move::<ReorderDrag>(move |event, _, cx| {
                if !event.bounds.contains(&event.event.position) {
                    return;
                }
                let place = {
                    let drag = event.drag(cx);
                    let offset = (event.event.position.y - event.bounds.top()).as_f32();
                    resolve_drop(
                        &drag.path,
                        &move_path,
                        move_folder,
                        offset,
                        event.bounds.size.height.as_f32(),
                    )
                };
                let _ = move_owner.update(cx, |this, cx| {
                    let next = place.map(|place| (move_id.clone(), place));
                    if this.drop_target != next {
                        this.drop_target = next;
                        cx.notify();
                    }
                });
            })
            .drag_over::<ReorderDrag>({
                let target = path.clone();
                let folder = kind == EntryKind::Folder;
                move |style, drag, _, cx| {
                    if accepts_drop(&drag.path, &target, folder) {
                        style.bg(cx.theme().list_hover)
                    } else {
                        style
                    }
                }
            })
            .on_drop(move |drag: &ReorderDrag, window, cx| {
                let _ = drop_owner.update(cx, |this, cx| {
                    let place = this.drop_target.as_ref().and_then(|(id, place)| {
                        (id.as_ref() == drop_path.as_str()).then_some(*place)
                    });
                    match place {
                        Some(DropPlace::Inside) => {
                            this.drop_into(drag.path.clone(), drop_path.clone(), window, cx);
                        }
                        Some(place) => {
                            this.drop_at(
                                drag.path.clone(),
                                drop_path.clone(),
                                place == DropPlace::After,
                                window,
                                cx,
                            );
                        }
                        None => {
                            this.drop_target = None;
                            cx.notify();
                        }
                    }
                });
            })
            .when(edge == Some(DropPlace::Inside), |item| item.bg(into_folder))
            .child(
                div()
                    .w_full()
                    .relative()
                    .child(
                        h_flex()
                            .gap_1()
                            .child(div().w_4().flex_none().children(
                                chevron.map(|icon| Icon::new(icon).xsmall().text_color(muted)),
                            ))
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
                    .when(edge == Some(DropPlace::Inside), |row| {
                        row.child(
                            div()
                                .absolute()
                                .inset_0()
                                .rounded(theme.radius)
                                .border_1()
                                .border_color(marker),
                        )
                    })
                    .when_some(
                        edge.filter(|place| matches!(place, DropPlace::Before | DropPlace::After)),
                        |row, place| {
                            row.child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .h(px(2.))
                                    .bg(marker)
                                    .when(place == DropPlace::After, |bar| bar.bottom(px(-5.)))
                                    .when(place == DropPlace::Before, |bar| bar.top(px(-5.))),
                            )
                        },
                    ),
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
        if !cx.has_active_drag() {
            self.drop_target = None;
        }
        let kinds = self.kinds.clone();
        let menu_kinds = self.kinds.clone();
        let explorer = cx.weak_entity();
        let row_owner = explorer.clone();
        let drop_target = self.drop_target.clone();

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
            let edge = drop_target
                .as_ref()
                .and_then(|(target_id, place)| (*target_id == entry.item().id).then_some(*place));
            Self::render_row(entry, kind, edge, row_owner.clone(), cx)
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
                (path.clone(), path.clone(), path.clone(), path.clone());
            let (up_path, down_path) = (path.clone(), path);
            let (rename_owner, delete_owner, up_owner, down_owner) = (
                explorer.clone(),
                explorer.clone(),
                explorer.clone(),
                explorer,
            );
            menu.separator()
                .item(PopupMenuItem::new("Move up").on_click(move |_, _, cx| {
                    _ = up_owner.update(cx, |this, cx| this.move_by(up_path.clone(), -1, cx));
                }))
                .item(PopupMenuItem::new("Move down").on_click(move |_, _, cx| {
                    _ = down_owner.update(cx, |this, cx| this.move_by(down_path.clone(), 1, cx));
                }))
                .separator()
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

/// The order chosen for each folder. An empty parent path is the vault root.
/// Child names are file names, so a note keeps its `.md`. Files and folders
/// share the list.
#[derive(Clone, Default, Serialize, Deserialize)]
struct FileOrder {
    #[serde(default)]
    folders: BTreeMap<String, Vec<String>>,
}

impl FileOrder {
    fn load(cx: &App) -> Self {
        Notia::load_plugin_data(ORDER_PLUGIN_ID, cx)
    }

    fn save(&self, cx: &App) {
        Notia::save_plugin_data(ORDER_PLUGIN_ID, self, cx);
    }
}

fn parent_key(path: &VaultPath) -> String {
    path.parent()
        .filter(|parent| !parent.is_root())
        .map(|parent| parent.as_str().to_string())
        .unwrap_or_default()
}

/// A row being dragged. `label` is what the explorer shows, without `.md`.
#[derive(Clone)]
struct ReorderDrag {
    path: VaultPath,
    label: SharedString,
}

struct FileDragGhost {
    label: SharedString,
}

impl Render for FileDragGhost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px_2()
            .py_1()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .text_sm()
            .child(self.label.clone())
    }
}

fn sibling_names(tree: &FileTree, path: &VaultPath) -> Vec<String> {
    let siblings = match path.parent() {
        Some(parent) if !parent.is_root() => tree
            .find(&parent)
            .map(|entry| entry.children.as_slice())
            .unwrap_or(&[]),
        _ => tree.entries(),
    };
    siblings.iter().map(|entry| entry.name.clone()).collect()
}

/// The names currently shown beside `path`, after the saved order is applied.
fn displayed_sibling_names(
    entries: &[FileEntry],
    path: &VaultPath,
    order: &BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    let mut entries = entries.to_vec();
    order_entries(&mut entries, "", order);
    let siblings = match path.parent() {
        Some(parent) if !parent.is_root() => find_entry(&entries, &parent)
            .map(|entry| entry.children.as_slice())
            .unwrap_or(&[]),
        _ => entries.as_slice(),
    };
    siblings.iter().map(|entry| entry.name.clone()).collect()
}

fn find_entry<'a>(entries: &'a [FileEntry], path: &VaultPath) -> Option<&'a FileEntry> {
    for entry in entries {
        if entry.path == *path {
            return Some(entry);
        }
        if entry.is_folder()
            && path.starts_with(&entry.path)
            && let Some(found) = find_entry(&entry.children, path)
        {
            return Some(found);
        }
    }
    None
}

/// Where a dragged row lands relative to the row under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DropPlace {
    Before,
    After,
    /// The pointer is over a folder, so the dragged entry moves inside it.
    Inside,
}

/// The drop for a pointer `offset` pixels below the top of a row of `height`.
/// The top and bottom of any row place the file beside it, which moves the
/// file out when that row lives in another folder. The middle of a folder
/// moves the file inside.
fn resolve_drop(
    dragged: &VaultPath,
    target: &VaultPath,
    folder: bool,
    offset: f32,
    height: f32,
) -> Option<DropPlace> {
    if height <= 0. || target.starts_with(dragged) {
        return None;
    }
    if folder {
        let edge = height * 0.25;
        if offset < edge {
            return Some(DropPlace::Before);
        }
        if offset > height - edge {
            return Some(DropPlace::After);
        }
        if dragged.parent().is_some_and(|parent| parent == *target) {
            return None;
        }
        return Some(DropPlace::Inside);
    }
    if offset >= height / 2. {
        Some(DropPlace::After)
    } else {
        Some(DropPlace::Before)
    }
}

fn accepts_drop(dragged: &VaultPath, target: &VaultPath, _folder: bool) -> bool {
    !target.starts_with(dragged)
}

/// Keeps saved names aligned after a rename or a move into another folder.
fn retarget_names(folders: &mut BTreeMap<String, Vec<String>>, from: &VaultPath, to: &VaultPath) {
    let from_key = parent_key(from);
    let to_key = parent_key(to);
    let old_name = from.file_name().to_string();
    let new_name = to.file_name().to_string();
    if from_key == to_key {
        if let Some(names) = folders.get_mut(&from_key) {
            if let Some(index) = names.iter().position(|name| name == &old_name) {
                names[index] = new_name;
            }
        }
    } else {
        if let Some(names) = folders.get_mut(&from_key) {
            names.retain(|name| name != &old_name);
        }
        if let Some(names) = folders.get_mut(&to_key) {
            if !names.iter().any(|name| name == &new_name) {
                names.push(new_name);
            }
        }
    }
    let old_prefix = from.as_str();
    if old_prefix.is_empty() {
        return;
    }
    let new_prefix = to.as_str();
    let nested: Vec<String> = folders
        .keys()
        .filter(|key| key.as_str() == old_prefix || key.starts_with(&format!("{old_prefix}/")))
        .cloned()
        .collect();
    for key in nested {
        let rest = &key[old_prefix.len()..];
        if let Some(names) = folders.remove(&key) {
            folders.insert(format!("{new_prefix}{rest}"), names);
        }
    }
}

/// Moves `moving` to sit just before `anchor`, or just after it when `after`.
fn place_name(names: &mut Vec<String>, moving: &str, anchor: &str, after: bool) {
    if moving == anchor {
        return;
    }
    names.retain(|name| name != moving);
    let mut index = names
        .iter()
        .position(|name| name == anchor)
        .unwrap_or(names.len());
    if after {
        index += 1;
    }
    if index > names.len() {
        index = names.len();
    }
    names.insert(index, moving.to_string());
}

/// Arranges `entries` by the saved names, then does the same inside folders.
/// A name that was never placed stays after the ones that were, in the order
/// it already had.
fn order_entries(entries: &mut [FileEntry], parent: &str, order: &BTreeMap<String, Vec<String>>) {
    if let Some(names) = order.get(parent) {
        entries.sort_by(|left, right| {
            let rank = |entry: &FileEntry| {
                names
                    .iter()
                    .position(|name| name == &entry.name)
                    .unwrap_or(usize::MAX)
            };
            rank(left).cmp(&rank(right))
        });
    }
    for entry in entries.iter_mut() {
        if entry.is_folder() {
            let child = if parent.is_empty() {
                entry.name.clone()
            } else {
                format!("{parent}/{}", entry.name)
            };
            order_entries(&mut entry.children, &child, order);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DropPlace, displayed_sibling_names, order_entries, place_name, resolve_drop, retarget_names,
    };
    use notia_plugin::core::{EntryKind, FileEntry, VaultPath};
    use std::collections::BTreeMap;

    fn entry(name: &str, kind: EntryKind) -> FileEntry {
        FileEntry {
            path: VaultPath::new(name),
            name: name.to_string(),
            kind,
            children: Vec::new(),
        }
    }

    #[test]
    fn a_saved_order_mixes_files_and_folders() {
        let mut entries = vec![
            entry("A", EntryKind::Folder),
            entry("b.md", EntryKind::Note),
            entry("c.md", EntryKind::Note),
        ];
        let order = BTreeMap::from([(
            String::new(),
            vec!["c.md".into(), "A".into(), "b.md".into()],
        )]);
        order_entries(&mut entries, "", &order);
        let names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["c.md", "A", "b.md"]);
    }

    #[test]
    fn an_unlisted_file_stays_after_the_arranged_ones() {
        let mut entries = vec![
            entry("a.md", EntryKind::Note),
            entry("z.md", EntryKind::Note),
        ];
        let order = BTreeMap::from([(String::new(), vec!["z.md".into()])]);
        order_entries(&mut entries, "", &order);
        let names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["z.md", "a.md"]);
    }

    #[test]
    fn a_drop_moves_a_name_before_or_after_its_neighbor() {
        let mut names = vec!["a.md".into(), "B".into(), "c.md".into()];
        place_name(&mut names, "c.md", "a.md", false);
        assert_eq!(
            names.iter().map(String::as_str).collect::<Vec<_>>(),
            ["c.md", "a.md", "B"]
        );

        let mut names = vec!["a.md".into(), "B".into(), "c.md".into()];
        place_name(&mut names, "a.md", "c.md", true);
        assert_eq!(
            names.iter().map(String::as_str).collect::<Vec<_>>(),
            ["B", "c.md", "a.md"]
        );
    }

    #[test]
    fn placing_a_name_on_itself_leaves_the_list() {
        let mut names = vec!["a.md".into(), "b.md".into()];
        place_name(&mut names, "a.md", "a.md", true);
        assert_eq!(
            names.iter().map(String::as_str).collect::<Vec<_>>(),
            ["a.md", "b.md"]
        );
    }

    #[test]
    fn a_drop_uses_the_order_on_screen() {
        let mut folder = entry("Notes", EntryKind::Folder);
        folder.path = VaultPath::new("Notes");
        folder.children = vec![
            FileEntry {
                path: VaultPath::new("Notes/a.md"),
                name: "a.md".into(),
                kind: EntryKind::Note,
                children: Vec::new(),
            },
            FileEntry {
                path: VaultPath::new("Notes/b.md"),
                name: "b.md".into(),
                kind: EntryKind::Note,
                children: Vec::new(),
            },
        ];
        let order = BTreeMap::from([("Notes".into(), vec!["b.md".into(), "a.md".into()])]);
        let names = displayed_sibling_names(&[folder], &VaultPath::new("Notes/a.md"), &order);
        assert_eq!(
            names.iter().map(String::as_str).collect::<Vec<_>>(),
            ["b.md", "a.md"]
        );
    }

    #[test]
    fn the_middle_of_a_folder_receives_the_file() {
        let file = VaultPath::new("Inbox/a.md");
        let folder = VaultPath::new("Notes");
        assert_eq!(
            resolve_drop(&file, &folder, true, 12., 24.),
            Some(DropPlace::Inside)
        );
    }

    #[test]
    fn a_file_can_be_dragged_out_beside_another_row() {
        let file = VaultPath::new("Notes/a.md");
        let outside = VaultPath::new("b.md");
        let folder = VaultPath::new("Notes");
        assert_eq!(
            resolve_drop(&file, &outside, false, 1., 24.),
            Some(DropPlace::Before)
        );
        assert_eq!(
            resolve_drop(&file, &outside, false, 20., 24.),
            Some(DropPlace::After)
        );
        assert_eq!(
            resolve_drop(&file, &folder, true, 1., 24.),
            Some(DropPlace::Before)
        );
        assert_eq!(resolve_drop(&file, &folder, true, 12., 24.), None);
    }

    #[test]
    fn the_edge_of_a_neighboring_folder_reorders() {
        let file = VaultPath::new("a.md");
        let folder = VaultPath::new("Notes");
        assert_eq!(
            resolve_drop(&file, &folder, true, 1., 24.),
            Some(DropPlace::Before)
        );
        assert_eq!(
            resolve_drop(&file, &folder, true, 22., 24.),
            Some(DropPlace::After)
        );
        assert_eq!(
            resolve_drop(&file, &folder, true, 12., 24.),
            Some(DropPlace::Inside)
        );
    }

    #[test]
    fn a_folder_cannot_be_dropped_into_itself() {
        let folder = VaultPath::new("Notes");
        let child = VaultPath::new("Notes/a.md");
        assert_eq!(resolve_drop(&folder, &folder, true, 12., 24.), None);
        assert_eq!(resolve_drop(&folder, &child, false, 12., 24.), None);
        assert_eq!(resolve_drop(&child, &folder, true, 12., 24.), None);
    }

    #[test]
    fn moving_into_a_folder_leaves_the_old_list() {
        let mut folders = BTreeMap::from([
            (String::new(), vec!["a.md".into(), "Notes".into()]),
            ("Notes".into(), vec!["b.md".into()]),
            ("Archive".into(), vec!["c.md".into()]),
        ]);
        retarget_names(
            &mut folders,
            &VaultPath::new("a.md"),
            &VaultPath::new("Archive/a.md"),
        );
        assert_eq!(
            folders.get("").map(|names| names.as_slice()),
            Some(["Notes".to_string()].as_slice())
        );
        assert_eq!(
            folders.get("Archive").map(|names| names.as_slice()),
            Some(["c.md".to_string(), "a.md".into()].as_slice())
        );

        retarget_names(
            &mut folders,
            &VaultPath::new("Notes"),
            &VaultPath::new("Archive/Notes"),
        );
        assert_eq!(folders.get("Notes"), None);
        assert_eq!(
            folders.get("Archive/Notes").map(|names| names.as_slice()),
            Some(["b.md".to_string()].as_slice())
        );
        assert_eq!(
            folders.get("Archive").map(|names| names.as_slice()),
            Some(["c.md".to_string(), "a.md".into(), "Notes".into()].as_slice())
        );
    }
}
