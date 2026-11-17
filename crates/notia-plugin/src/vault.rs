use std::{io, path::Component, time::Duration};

use gpui_kit::{Context, EventEmitter, Task};
use notia_core::{FileTree, IndexChanges, IndexedNote, Vault, VaultIndex, VaultPath};

/// What changed in the vault.
#[derive(Clone, Debug, PartialEq)]
pub enum VaultEvent {
    /// Files or folders were created, renamed, moved or deleted.
    TreeChanged,
    /// These notes were re-indexed after their content changed.
    NotesChanged(Vec<VaultPath>),
    /// Notia renamed or moved an entry.
    Renamed { from: VaultPath, to: VaultPath },
    /// Notia moved an entry to the trash.
    Trashed(VaultPath),
}

/// The open vault: the folder tree, the link index, and the file operations
/// that keep both up to date. Changes made outside Notia are picked up by a
/// file watcher and an incremental rescan.
pub struct VaultModel {
    vault: Vault,
    tree: FileTree,
    index: VaultIndex,
    loaded: bool,
    scan: Option<Task<()>>,
    rescan_requested: bool,
    _watch: Option<Task<()>>,
}

impl EventEmitter<VaultEvent> for VaultModel {}

impl VaultModel {
    pub(crate) fn new(vault: Vault, cx: &mut Context<Self>) -> Self {
        let mut model = Self {
            vault,
            tree: FileTree::default(),
            index: VaultIndex::default(),
            loaded: false,
            scan: None,
            rescan_requested: false,
            _watch: None,
        };
        model.rescan(cx);
        model.watch(cx);
        model
    }

    pub fn vault(&self) -> &Vault {
        &self.vault
    }

    pub fn name(&self) -> String {
        self.vault.name()
    }

    pub fn tree(&self) -> &FileTree {
        &self.tree
    }

    pub fn index(&self) -> &VaultIndex {
        &self.index
    }

    /// Whether the first scan has finished.
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Re-reads the folder tree and every note that changed on disk, off
    /// the main thread.
    pub fn rescan(&mut self, cx: &mut Context<Self>) {
        if self.scan.is_some() {
            self.rescan_requested = true;
            return;
        }
        let vault = self.vault.clone();
        let known = self.index.snapshot();
        self.scan = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let tree = vault.scan()?;
                    let changes = VaultIndex::read_changes(&vault, &tree, &known);
                    io::Result::Ok((tree, changes))
                })
                .await;
            _ = this.update(cx, |this, cx| {
                this.scan = None;
                match result {
                    Ok((tree, changes)) => this.apply_scan(tree, changes, cx),
                    Err(error) => log::error!("could not scan the vault: {error}"),
                }
                if std::mem::take(&mut this.rescan_requested) {
                    this.rescan(cx);
                }
            });
        }));
    }

    fn apply_scan(&mut self, tree: FileTree, changes: IndexChanges, cx: &mut Context<Self>) {
        let first_load = !self.loaded;
        let tree_changed = first_load || tree != self.tree;
        let changed: Vec<VaultPath> = changes
            .updated()
            .chain(changes.removed())
            .cloned()
            .collect();
        self.tree = tree;
        self.index.apply(changes);
        self.loaded = true;
        if tree_changed {
            cx.emit(VaultEvent::TreeChanged);
        }
        if !changed.is_empty() {
            cx.emit(VaultEvent::NotesChanged(changed));
        }
        cx.notify();
    }

    fn watch(&mut self, cx: &mut Context<Self>) {
        let (tx, rx) = smol::channel::bounded::<()>(1);
        let root = self.vault.root().to_path_buf();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else {
                return;
            };
            if event.kind.is_access() {
                return;
            }
            // Settings, themes, the trash and temporary files live in hidden
            // entries; changes there are not vault content.
            let relevant = event.need_rescan()
                || event.paths.iter().any(|path| match path.strip_prefix(&root) {
                    Ok(relative) => !relative.components().any(|component| {
                        matches!(component, Component::Normal(name) if name.to_string_lossy().starts_with('.'))
                    }),
                    Err(_) => true,
                });
            if relevant {
                let _ = tx.try_send(());
            }
        });
        let mut watcher = match watcher {
            Ok(watcher) => watcher,
            Err(error) => {
                log::error!("vault changes made outside Notia will not show: {error}");
                return;
            }
        };
        if let Err(error) = notify::Watcher::watch(
            &mut watcher,
            self.vault.root(),
            notify::RecursiveMode::Recursive,
        ) {
            log::error!("could not watch the vault: {error}");
            return;
        }
        self._watch = Some(cx.spawn(async move |this, cx| {
            let _watcher = watcher;
            while rx.recv().await.is_ok() {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                while rx.try_recv().is_ok() {}
                if this.update(cx, |this, cx| this.rescan(cx)).is_err() {
                    break;
                }
            }
        }));
    }

    pub fn read_note(&self, path: &VaultPath) -> io::Result<String> {
        self.vault.read_note(path)
    }

    /// Writes a note and re-indexes it immediately, without waiting for the
    /// file watcher.
    pub fn save_note(
        &mut self,
        path: &VaultPath,
        text: &str,
        cx: &mut Context<Self>,
    ) -> io::Result<()> {
        let is_new = !self.vault.exists(path);
        self.vault.write_note(path, text)?;
        let modified = self.vault.modified(path);
        self.index
            .insert(IndexedNote::new(path.clone(), text, modified));
        cx.emit(VaultEvent::NotesChanged(vec![path.clone()]));
        if is_new {
            self.rescan(cx);
        }
        cx.notify();
        Ok(())
    }

    pub fn create_note(
        &mut self,
        folder: &VaultPath,
        title: &str,
        content: &str,
        cx: &mut Context<Self>,
    ) -> io::Result<VaultPath> {
        let path = self.vault.create_note(folder, title, content)?;
        let modified = self.vault.modified(&path);
        self.index
            .insert(IndexedNote::new(path.clone(), content, modified));
        self.rescan(cx);
        Ok(path)
    }

    /// Creates a Canvas note in `folder`.
    pub fn create_canvas(
        &mut self,
        folder: &VaultPath,
        title: &str,
        content: &str,
        cx: &mut Context<Self>,
    ) -> io::Result<VaultPath> {
        let path = self.vault.create_canvas(folder, title, content)?;
        let modified = self.vault.modified(&path);
        self.index
            .insert(IndexedNote::new(path.clone(), content, modified));
        self.rescan(cx);
        Ok(path)
    }

    /// Creates a Kanban note in `folder`.
    pub fn create_kanban(
        &mut self,
        folder: &VaultPath,
        title: &str,
        content: &str,
        cx: &mut Context<Self>,
    ) -> io::Result<VaultPath> {
        let path = self.vault.create_kanban(folder, title, content)?;
        let modified = self.vault.modified(&path);
        self.index
            .insert(IndexedNote::new(path.clone(), content, modified));
        self.rescan(cx);
        Ok(path)
    }

    pub fn create_folder(
        &mut self,
        parent: &VaultPath,
        name: &str,
        cx: &mut Context<Self>,
    ) -> io::Result<VaultPath> {
        let path = self.vault.create_folder(parent, name)?;
        self.rescan(cx);
        Ok(path)
    }

    pub fn rename(
        &mut self,
        path: &VaultPath,
        new_name: &str,
        cx: &mut Context<Self>,
    ) -> io::Result<VaultPath> {
        let to = self.vault.rename(path, new_name)?;
        self.moved(path, &to, cx);
        Ok(to)
    }

    pub fn move_into(
        &mut self,
        path: &VaultPath,
        folder: &VaultPath,
        cx: &mut Context<Self>,
    ) -> io::Result<VaultPath> {
        let to = self.vault.move_into(path, folder)?;
        self.moved(path, &to, cx);
        Ok(to)
    }

    pub fn trash(&mut self, path: &VaultPath, cx: &mut Context<Self>) -> io::Result<()> {
        self.vault.trash(path)?;
        let removed: Vec<VaultPath> = self
            .index
            .notes()
            .filter(|note| note.path.starts_with(path))
            .map(|note| note.path.clone())
            .collect();
        for note in &removed {
            self.index.remove(note);
        }
        cx.emit(VaultEvent::Trashed(path.clone()));
        if !removed.is_empty() {
            cx.emit(VaultEvent::NotesChanged(removed));
        }
        self.rescan(cx);
        Ok(())
    }

    /// Moves the index entries of a renamed or moved entry to its new path.
    fn moved(&mut self, from: &VaultPath, to: &VaultPath, cx: &mut Context<Self>) {
        let moved: Vec<IndexedNote> = self
            .index
            .notes()
            .filter(|note| note.path.starts_with(from))
            .cloned()
            .collect();
        for note in moved {
            self.index.remove(&note.path);
            if let Some(path) = note.path.rebase(from, to) {
                let modified = self.vault.modified(&path);
                self.index
                    .insert(IndexedNote::new(path, note.text, modified));
            }
        }
        cx.emit(VaultEvent::Renamed {
            from: from.clone(),
            to: to.clone(),
        });
        self.rescan(cx);
    }
}
