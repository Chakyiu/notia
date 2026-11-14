//! The core plugin that finds community plugins and keeps them in sync with
//! the plugins folder.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use gpui_kit::{
    App, Global, SharedString, Task,
    assets::IconName as Lucide,
    component::{Icon, notification::Notification},
};
use notia_plugin::{Command, Notia, Plugin, PluginContext, PluginHost, PluginKind, PluginManifest};

use crate::{
    TYPE_DEFINITIONS,
    manifest::{self, BrokenPlugin, Fingerprint, PluginInfo},
    runtime::{JsRuntime, notify},
};

pub(crate) const LOADER_ID: &str = "community-plugins";

/// Lists the plugins in `.notia/plugins` and loads the enabled ones.
pub struct CommunityPluginsPlugin;

impl Plugin for CommunityPluginsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(LOADER_ID, "Community plugins").with_description(
            "Lists the JavaScript plugins in the vault's .notia/plugins folder. A plugin's code runs only while it is enabled.",
        )
    }

    fn activate(&self, ctx: &mut PluginContext, cx: &mut App) {
        let dir = Notia::vault(cx)
            .read(cx)
            .vault()
            .config_dir()
            .join("plugins");
        if let Err(error) = fs::create_dir_all(&dir) {
            log::warn!("could not create {}: {error}", dir.display());
        }
        let (watcher, task) = watch(&dir, cx).unzip();
        if let Some(task) = task {
            ctx.keep_task(task);
        }
        cx.set_global(Installed {
            dir: dir.clone(),
            plugins: BTreeMap::new(),
            watcher,
            linked: BTreeSet::new(),
        });
        // Reading the manifests waits for the app to finish starting.
        cx.defer(reload);

        ctx.add_command(
            Command::new("reload", "Reload community plugins", |_, cx| reload(cx))
                .with_icon(Icon::new(Lucide::RefreshCw)),
        );
        ctx.add_command(
            Command::new(
                "open-folder",
                "Open the community plugins folder",
                move |_, cx| open_folder(&dir, cx),
            )
            .with_icon(Icon::new(Lucide::FolderOpen)),
        );
        ctx.on_deactivate(|cx| {
            if let Some(installed) = cx.try_global::<Installed>() {
                let ids: Vec<SharedString> = installed.plugins.keys().cloned().collect();
                for id in ids {
                    PluginHost::unregister(&id, cx);
                }
            }
            cx.remove_global::<Installed>();
        });
    }
}

/// The community plugins registered with the plugin host.
struct Installed {
    dir: PathBuf,
    plugins: BTreeMap<SharedString, Fingerprint>,
    watcher: Option<notify::RecommendedWatcher>,
    /// Where the plugin folders that are links lead. A watch on `dir` does
    /// not see changes there on macOS, so each is watched too.
    linked: BTreeSet<PathBuf>,
}

impl Global for Installed {}

/// Registers new plugins, removes deleted ones and re-registers changed
/// ones, which reloads the code of those that are enabled.
pub(crate) fn reload(cx: &mut App) {
    let Some(dir) = cx
        .try_global::<Installed>()
        .map(|installed| installed.dir.clone())
    else {
        return;
    };
    let mut previous = std::mem::take(&mut cx.global_mut::<Installed>().plugins);
    let mut current = BTreeMap::new();
    let discovered = manifest::discover(&dir);
    watch_linked(discovered.iter().map(|found| found.folder.as_path()), cx);
    for found in discovered {
        let id = found.id().clone();
        if current.contains_key(&id) {
            log::warn!("ignoring a second community plugin with the id “{id}”");
            continue;
        }
        match previous.remove(&id) {
            Some(fingerprint) if fingerprint == found.fingerprint => {
                current.insert(id, fingerprint);
                continue;
            }
            Some(_) => PluginHost::unregister(&id, cx),
            None => {
                let taken = PluginHost::global(cx)
                    .plugins()
                    .any(|(manifest, _)| *manifest.id() == id);
                if taken {
                    log::warn!("ignoring the community plugin “{id}”: the id is taken");
                    notify(
                        Notification::warning(format!(
                            "The community plugin “{id}” was not loaded: another plugin has this id."
                        )),
                        cx,
                    );
                    continue;
                }
            }
        }
        match found.plugin {
            Ok(plugin) => PluginHost::register(JsPlugin(Rc::new(plugin)), cx),
            Err(broken) => {
                let error = broken.error.clone();
                PluginHost::register(Broken(broken), cx);
                PluginHost::set_error(&id, Some(error), cx);
            }
        }
        current.insert(id, found.fingerprint);
    }
    for id in previous.into_keys() {
        PluginHost::unregister(&id, cx);
    }
    if cx.has_global::<Installed>() {
        cx.global_mut::<Installed>().plugins = current;
    }
}

/// Reloads plugins when their files change.
fn watch(dir: &Path, cx: &mut App) -> Option<(notify::RecommendedWatcher, Task<()>)> {
    let (tx, rx) = smol::channel::bounded::<()>(1);
    let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else {
            return;
        };
        let relevant = !event.kind.is_access()
            && event
                .paths
                .iter()
                .any(|path| !manifest::is_data_file(path) && !path.ends_with("notia.d.ts"));
        if relevant {
            // A full channel already holds a pending reload.
            let _ = tx.try_send(());
        }
    });
    let mut watcher = watcher
        .inspect_err(|error| log::error!("plugin hot reload is unavailable: {error}"))
        .ok()?;
    notify::Watcher::watch(&mut watcher, dir, notify::RecursiveMode::Recursive)
        .inspect_err(|error| log::error!("could not watch {}: {error}", dir.display()))
        .ok()?;

    let task = cx.spawn(async move |cx| {
        while rx.recv().await.is_ok() {
            // Editors and bundlers write in bursts; let the burst settle.
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            while rx.try_recv().is_ok() {}
            cx.update(reload);
        }
    });
    Some((watcher, task))
}

/// Watches where the plugin folders that are links lead, and stops watching
/// where removed links led.
fn watch_linked<'a>(folders: impl Iterator<Item = &'a Path>, cx: &mut App) {
    let Installed {
        dir,
        watcher,
        linked,
        ..
    } = cx.global_mut::<Installed>();
    let Some(watcher) = watcher else {
        return;
    };
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.clone());
    let targets: BTreeSet<PathBuf> = folders
        .filter_map(|folder| folder.canonicalize().ok())
        .filter(|target| !target.starts_with(&dir))
        .collect();
    for gone in linked.difference(&targets) {
        _ = notify::Watcher::unwatch(watcher, gone);
    }
    for target in targets.difference(linked) {
        if let Err(error) =
            notify::Watcher::watch(watcher, target, notify::RecursiveMode::Recursive)
        {
            log::warn!("could not watch {}: {error}", target.display());
        }
    }
    *linked = targets;
}

/// Opens the plugins folder, with the type definitions of the plugin API
/// for writing plugins in it.
fn open_folder(dir: &Path, cx: &mut App) {
    let types = dir.join("notia.d.ts");
    let result = fs::create_dir_all(dir).and_then(|()| {
        if fs::read_to_string(&types).ok().as_deref() == Some(TYPE_DEFINITIONS) {
            Ok(())
        } else {
            fs::write(&types, TYPE_DEFINITIONS)
        }
    });
    if let Err(error) = result {
        log::warn!("could not set up {}: {error}", dir.display());
    }
    cx.open_with_system(dir);
}

/// A community plugin. Registering it reads nothing but its manifest; its
/// code is loaded when it is enabled.
struct JsPlugin(Rc<PluginInfo>);

impl Plugin for JsPlugin {
    fn manifest(&self) -> PluginManifest {
        let plugin = &self.0;
        PluginManifest::new(plugin.id.clone(), plugin.name.clone())
            .with_description(plugin.description.clone())
            .with_version(plugin.version.clone())
            .with_author(plugin.author.clone())
            .with_kind(PluginKind::Community)
            .enabled_by_default(false)
    }

    fn activate(&self, ctx: &mut PluginContext, cx: &mut App) {
        let plugin = &self.0;
        match JsRuntime::load(plugin.clone(), cx) {
            Ok(js) => js.attach(ctx, cx),
            Err(error) => {
                log::error!("could not load {} ({}): {error}", plugin.name, plugin.id);
                let summary = error.lines().next().unwrap_or_default();
                notify(
                    Notification::error(format!("Could not load {}: {summary}", plugin.name)),
                    cx,
                );
                PluginHost::set_error(&plugin.id, Some(error.into()), cx);
            }
        }
    }
}

/// A plugin folder that cannot be loaded, listed so that the settings can
/// say why.
struct Broken(BrokenPlugin);

impl Plugin for Broken {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(self.0.id.clone(), self.0.id.clone())
            .with_description("This plugin cannot be loaded.")
            .with_version("")
            .with_author("")
            .with_kind(PluginKind::Community)
            .enabled_by_default(false)
    }

    fn activate(&self, _: &mut PluginContext, cx: &mut App) {
        PluginHost::set_error(&self.0.id, Some(self.0.error.clone()), cx);
    }
}
