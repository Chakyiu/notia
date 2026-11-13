use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use gpui_kit::{
    App, Global, SharedString, Task,
    component::{ActiveTheme as _, Theme, ThemeConfig, ThemeMode, ThemeRegistry},
    px,
};

use crate::{
    Appearance, EditorAppearance, EditorTheme, ThemePack, ThemeSource, appearance::DEFAULT_THEME_ID,
};

/// A theme file that could not be loaded, shown in the settings so the
/// author can fix it.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeLoadError {
    pub path: PathBuf,
    pub message: String,
}

/// Every installed theme plugin and the appearance preferences that pick
/// one of them. Changing either re-applies the theme to every window.
pub struct ThemeCatalog {
    packs: Vec<Rc<ThemePack>>,
    appearance: Appearance,
    /// A theme shown in place of the selected one without saving it.
    preview: Option<String>,
    /// Where the appearance is saved whenever it changes.
    appearance_file: Option<PathBuf>,
    folder: Option<PathBuf>,
    errors: Vec<ThemeLoadError>,
    /// Owns the folder watcher; dropping it stops hot reload.
    _watch: Option<Task<()>>,
}

impl Global for ThemeCatalog {}

impl ThemeCatalog {
    pub(crate) fn init(cx: &mut App) {
        cx.set_global(Self {
            packs: Vec::new(),
            appearance: Appearance::default(),
            preview: None,
            appearance_file: None,
            folder: None,
            errors: Vec::new(),
            _watch: None,
        });
        cx.set_global(EditorAppearance::default());
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub fn appearance(&self) -> &Appearance {
        &self.appearance
    }

    /// The vault folder themes are loaded from.
    pub fn folder(&self) -> Option<&Path> {
        self.folder.as_deref()
    }

    pub fn errors(&self) -> &[ThemeLoadError] {
        &self.errors
    }

    /// The installed themes, one per id, ordered by name. Where sources
    /// share an id, the vault folder wins over plugins and the bundle.
    pub fn packs(&self) -> Vec<Rc<ThemePack>> {
        let mut by_id: BTreeMap<&SharedString, &Rc<ThemePack>> = BTreeMap::new();
        for pack in &self.packs {
            match by_id.get(pack.id()) {
                Some(existing) if existing.source().rank() >= pack.source().rank() => {}
                _ => {
                    by_id.insert(pack.id(), pack);
                }
            }
        }
        let mut packs: Vec<Rc<ThemePack>> = by_id.into_values().cloned().collect();
        packs.sort_by_key(|pack| pack.name().to_lowercase());
        packs
    }

    pub fn pack(&self, id: &str) -> Option<Rc<ThemePack>> {
        self.packs
            .iter()
            .filter(|pack| pack.id().as_ref() == id)
            .max_by_key(|pack| pack.source().rank())
            .cloned()
    }

    /// The theme in use: the previewed one, the selected one, the default,
    /// or any installed theme, in that order.
    pub fn active_pack(&self) -> Option<Rc<ThemePack>> {
        self.preview
            .as_deref()
            .and_then(|id| self.pack(id))
            .or_else(|| self.pack(&self.appearance.theme))
            .or_else(|| self.pack(DEFAULT_THEME_ID))
            .or_else(|| self.packs.first().cloned())
    }

    /// Shows a theme without selecting it, or ends the preview with `None`.
    pub fn preview(theme_id: Option<&str>, cx: &mut App) {
        let preview = theme_id.map(str::to_string);
        if Self::global(cx).preview != preview {
            cx.global_mut::<Self>().preview = preview;
            Self::apply(cx);
        }
    }

    /// The mode the active theme is shown in, resolving `System`.
    pub fn active_mode(&self, cx: &App) -> ThemeMode {
        match self.appearance.scheme {
            crate::ColorScheme::Light => ThemeMode::Light,
            crate::ColorScheme::Dark => ThemeMode::Dark,
            crate::ColorScheme::System => cx.window_appearance().into(),
        }
    }

    /// Adds a theme, replacing one with the same id from the same source.
    pub fn register(pack: ThemePack, cx: &mut App) {
        let catalog = cx.global_mut::<Self>();
        catalog.packs.retain(|existing| {
            !(existing.id() == pack.id() && existing.source().same_kind(pack.source()))
        });
        catalog.packs.push(Rc::new(pack));
        Self::apply(cx);
    }

    /// Adds a theme on behalf of a code plugin.
    pub fn register_for_plugin(pack: ThemePack, plugin_id: SharedString, cx: &mut App) {
        Self::register(pack.with_source(ThemeSource::Plugin(plugin_id)), cx);
    }

    /// Removes every theme a code plugin contributed.
    pub fn unregister_plugin(plugin_id: &str, cx: &mut App) {
        let catalog = cx.global_mut::<Self>();
        let before = catalog.packs.len();
        catalog.packs.retain(
            |pack| !matches!(pack.source(), ThemeSource::Plugin(id) if id.as_ref() == plugin_id),
        );
        if catalog.packs.len() != before {
            Self::apply(cx);
        }
    }

    pub fn set_appearance(appearance: Appearance, cx: &mut App) {
        if Self::global(cx).appearance == appearance {
            return;
        }
        let catalog = cx.global_mut::<Self>();
        catalog.appearance = appearance;
        if let Some(path) = &catalog.appearance_file {
            let saved = serde_json::to_string_pretty(&catalog.appearance)
                .map_err(anyhow::Error::from)
                .and_then(|json| {
                    if let Some(dir) = path.parent() {
                        fs::create_dir_all(dir)?;
                    }
                    Ok(fs::write(path, json)?)
                });
            if let Err(error) = saved {
                log::error!("could not save {}: {error:#}", path.display());
            }
        }
        Self::apply(cx);
    }

    /// Restores the appearance saved at `path` and saves every later change
    /// there.
    pub fn load_appearance(path: PathBuf, cx: &mut App) {
        let appearance = match fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_else(|error| {
                log::warn!("ignoring the invalid {}: {error}", path.display());
                Appearance::default()
            }),
            Err(_) => Appearance::default(),
        };
        let catalog = cx.global_mut::<Self>();
        catalog.appearance = appearance;
        catalog.appearance_file = Some(path);
        Self::apply(cx);
    }

    pub fn update_appearance(cx: &mut App, edit: impl FnOnce(&mut Appearance)) {
        let mut appearance = Self::global(cx).appearance.clone();
        edit(&mut appearance);
        Self::set_appearance(appearance, cx);
    }

    /// Installs the active theme into GPUI Component and resolves the note
    /// typography. Call it again when the system appearance changes.
    pub fn apply(cx: &mut App) {
        let catalog = Self::global(cx);
        let appearance = catalog.appearance.clone();
        let mode = catalog.active_mode(cx);
        let pack = catalog.active_pack();

        let registry = ThemeRegistry::global(cx);
        let default_light = registry.default_light_theme().clone();
        let default_dark = registry.default_dark_theme().clone();
        let (light, dark, editor) = match &pack {
            Some(pack) => (
                pack.exact_variant(ThemeMode::Light)
                    .map_or(default_light, |variant| variant.config().clone()),
                pack.exact_variant(ThemeMode::Dark)
                    .map_or(default_dark, |variant| variant.config().clone()),
                pack.variant(mode).editor().clone(),
            ),
            None => (default_light, default_dark, EditorTheme::default()),
        };
        let active = match &pack {
            Some(pack) => pack.variant(mode).config().clone(),
            None if mode.is_dark() => dark.clone(),
            None => light.clone(),
        };
        // Fresh allocations make `Theme::update` see a newly installed theme
        // and not reload the previous one over the overrides below.
        let fresh = |config: &Rc<ThemeConfig>| Rc::new((**config).clone());
        let (light, dark, active) = (fresh(&light), fresh(&dark), fresh(&active));
        let defaults = Theme::default();

        Theme::update(cx, |theme| {
            // A theme only sets what it names, so start every theme from the
            // defaults rather than from whatever the previous one set.
            theme.font_family = defaults.font_family.clone();
            theme.mono_font_family = defaults.mono_font_family.clone();
            theme.mono_font_size = defaults.mono_font_size;
            theme.radius = defaults.radius;
            theme.radius_lg = defaults.radius_lg;
            theme.shadow = defaults.shadow;
            theme.light_theme = light;
            theme.dark_theme = dark;
            theme.apply_config(&active);
            theme.font_size = px(appearance.interface_font_size.clamp(10., 24.));
        });

        let interface_font = cx.theme().font_family.clone();
        cx.set_global(EditorAppearance::resolve(
            &editor,
            &appearance,
            &interface_font,
        ));
        cx.refresh_windows();
    }

    /// Loads the themes in `dir` and reloads them whenever a file in it
    /// changes. Replaces any folder watched before.
    pub fn watch_folder(dir: PathBuf, cx: &mut App) {
        if let Err(error) = fs::create_dir_all(&dir) {
            log::warn!(
                "could not create the themes folder {}: {error}",
                dir.display()
            );
        }
        cx.global_mut::<Self>().folder = Some(dir.clone());
        Self::reload_folder(cx);

        let (tx, rx) = smol::channel::bounded::<()>(1);
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if event.is_ok_and(|event| !event.kind.is_access()) {
                // A full channel already holds a pending reload.
                let _ = tx.try_send(());
            }
        });
        let mut watcher = match watcher {
            Ok(watcher) => watcher,
            Err(error) => {
                log::error!("theme hot reload is unavailable: {error}");
                return;
            }
        };
        if let Err(error) =
            notify::Watcher::watch(&mut watcher, &dir, notify::RecursiveMode::Recursive)
        {
            log::error!("could not watch {}: {error}", dir.display());
            return;
        }

        let task = cx.spawn(async move |cx| {
            let _watcher = watcher;
            while rx.recv().await.is_ok() {
                // Editors save in bursts; let the burst settle first.
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                while rx.try_recv().is_ok() {}
                cx.update(Self::reload_folder);
            }
        });
        cx.global_mut::<Self>()._watch = Some(task);
    }

    /// Reloads the themes of the watched folder.
    pub fn reload_folder(cx: &mut App) {
        let Some(dir) = Self::global(cx).folder.clone() else {
            return;
        };
        let (packs, errors) = ThemePack::load_folder(&dir);
        let catalog = cx.global_mut::<Self>();
        catalog
            .packs
            .retain(|pack| !matches!(pack.source(), ThemeSource::Folder(_)));
        catalog.packs.extend(packs.into_iter().map(Rc::new));
        catalog.errors = errors
            .into_iter()
            .map(|(path, error)| {
                log::warn!("could not load the theme {}: {error:#}", path.display());
                ThemeLoadError {
                    path,
                    message: format!("{error:#}"),
                }
            })
            .collect();
        Self::apply(cx);
    }
}
