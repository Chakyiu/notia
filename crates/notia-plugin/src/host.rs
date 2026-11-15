use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    rc::Rc,
};

use gpui_kit::{
    App, Global, KeyBinding, KeyBindingContextPredicate, SharedString, Subscription, Task,
    component::Icon,
};
use notia_core::{EntryKind, VaultPath};
use notia_theme::ThemeCatalog;

use crate::{
    Command, NavigationIcon, Panel, Plugin, PluginContext, PluginManifest, RibbonItem, SettingsTab,
    StatusItem, TabBarItem,
    actions::{RunCommand, WORKSPACE_CONTEXT},
    context::DeactivateHook,
};

/// What an enabled plugin registered.
struct Activation {
    commands: Vec<Command>,
    ribbon_items: Vec<RibbonItem>,
    panels: Vec<Panel>,
    status_items: Vec<StatusItem>,
    tab_bar_items: Vec<TabBarItem>,
    navigation_icons: Vec<NavigationIcon>,
    settings_tabs: Vec<SettingsTab>,
    _subscriptions: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
    deactivate_hooks: Vec<DeactivateHook>,
}

impl Activation {
    fn extend(&mut self, other: Activation) {
        self.commands.extend(other.commands);
        self.ribbon_items.extend(other.ribbon_items);
        self.panels.extend(other.panels);
        self.status_items.extend(other.status_items);
        self.tab_bar_items.extend(other.tab_bar_items);
        self.navigation_icons.extend(other.navigation_icons);
        self.settings_tabs.extend(other.settings_tabs);
        self._subscriptions.extend(other._subscriptions);
        self._tasks.extend(other._tasks);
        self.deactivate_hooks.extend(other.deactivate_hooks);
    }
}

struct Registration {
    plugin: Rc<dyn Plugin>,
    manifest: PluginManifest,
    activation: Option<Activation>,
    error: Option<SharedString>,
}

/// Every registered plugin, which of them are enabled, and the contributions
/// of the enabled ones. Observe this global to follow contribution changes.
pub struct PluginHost {
    registrations: Vec<Registration>,
    /// The enabled state the user chose, by plugin id.
    choices: BTreeMap<String, bool>,
    config_path: PathBuf,
    /// Keybindings already installed. GPUI cannot remove a single binding,
    /// so a disabled plugin's bindings stay and run nothing.
    bound_keys: HashSet<(SharedString, SharedString)>,
}

impl Global for PluginHost {}

impl PluginHost {
    pub(crate) fn init(config_path: PathBuf, cx: &mut App) {
        cx.set_global(Self {
            registrations: Vec::new(),
            choices: crate::read_json(&config_path),
            config_path,
            bound_keys: HashSet::new(),
        });
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// Adds a plugin and enables it when the vault has it enabled.
    pub fn register(plugin: impl Plugin, cx: &mut App) {
        let plugin: Rc<dyn Plugin> = Rc::new(plugin);
        let manifest = plugin.manifest();
        let host = cx.global_mut::<Self>();
        if host.position(manifest.id()).is_some() {
            log::warn!(
                "a plugin with the id “{}” is already registered",
                manifest.id()
            );
            return;
        }
        let enabled = host
            .choices
            .get(manifest.id().as_ref())
            .copied()
            .unwrap_or(manifest.is_enabled_by_default());
        let id = manifest.id().clone();
        host.registrations.push(Registration {
            plugin,
            manifest,
            activation: None,
            error: None,
        });
        if enabled {
            Self::activate(&id, cx);
        }
    }

    /// Disables and removes a plugin. Its enabled state is kept, so
    /// registering it again restores it.
    pub fn unregister(id: &str, cx: &mut App) {
        let host = Self::global(cx);
        let Some(id) = host
            .position(id)
            .map(|ix| host.registrations[ix].manifest.id().clone())
        else {
            return;
        };
        Self::deactivate(&id, cx);
        let host = cx.global_mut::<Self>();
        if let Some(ix) = host.position(&id) {
            host.registrations.remove(ix);
        }
    }

    /// Adds contributions to an enabled plugin after its activation, for
    /// plugins that register some of them later. Does nothing when the
    /// plugin is disabled.
    pub fn extend(id: &str, cx: &mut App, contribute: impl FnOnce(&mut PluginContext)) {
        let host = Self::global(cx);
        let Some(id) = host
            .position(id)
            .filter(|&ix| host.registrations[ix].activation.is_some())
            .map(|ix| host.registrations[ix].manifest.id().clone())
        else {
            return;
        };
        let mut context = PluginContext::new(id.clone());
        contribute(&mut context);
        let added = Self::install(&id, context, cx);
        let host = cx.global_mut::<Self>();
        if let Some(activation) = host
            .position(&id)
            .and_then(|ix| host.registrations[ix].activation.as_mut())
        {
            activation.extend(added);
        }
    }

    /// Why the plugin failed to load, if it did.
    pub fn error(&self, id: &str) -> Option<&SharedString> {
        self.position(id)
            .and_then(|ix| self.registrations[ix].error.as_ref())
    }

    /// Records why a plugin failed, which the settings show with it, or
    /// clears it with `None`. Enabling or disabling the plugin clears it too.
    pub fn set_error(id: &str, error: Option<SharedString>, cx: &mut App) {
        let host = cx.global_mut::<Self>();
        if let Some(ix) = host.position(id) {
            host.registrations[ix].error = error;
        }
    }

    /// Every plugin with whether it is enabled, in registration order.
    pub fn plugins(&self) -> impl Iterator<Item = (&PluginManifest, bool)> {
        self.registrations
            .iter()
            .map(|registration| (&registration.manifest, registration.activation.is_some()))
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        self.position(id)
            .is_some_and(|ix| self.registrations[ix].activation.is_some())
    }

    pub fn set_enabled(id: &str, enabled: bool, cx: &mut App) {
        let host = Self::global(cx);
        let Some(ix) = host.position(id) else {
            return;
        };
        let id = host.registrations[ix].manifest.id().clone();
        if host.is_enabled(&id) != enabled {
            if enabled {
                Self::activate(&id, cx);
            } else {
                Self::deactivate(&id, cx);
            }
        }
        let host = cx.global_mut::<Self>();
        host.choices.insert(id.to_string(), enabled);
        crate::write_json(&host.config_path, &host.choices);
    }

    fn position(&self, id: &str) -> Option<usize> {
        self.registrations
            .iter()
            .position(|registration| registration.manifest.id().as_ref() == id)
    }

    fn activate(id: &SharedString, cx: &mut App) {
        let host = cx.global_mut::<Self>();
        let Some(ix) = host.position(id) else {
            return;
        };
        host.registrations[ix].error = None;
        let plugin = host.registrations[ix].plugin.clone();

        // The host is not borrowed while the plugin runs, so the plugin may
        // read it or other globals freely.
        let mut context = PluginContext::new(id.clone());
        plugin.activate(&mut context, cx);
        let activation = Self::install(id, context, cx);

        {
            let host = cx.global_mut::<Self>();
            if let Some(ix) = host.position(id) {
                host.registrations[ix].activation = Some(activation);
            }
        }
        // Defaults are bound above. Saved shortcut changes have to follow
        // them, or a plugin enabled later would keep its original keys.
        crate::hotkeys::sync(cx);
    }

    /// Registers the themes and keybindings a plugin contributed, and keeps
    /// the rest.
    fn install(id: &SharedString, context: PluginContext, cx: &mut App) -> Activation {
        let PluginContext {
            commands,
            ribbon_items,
            panels,
            status_items,
            tab_bar_items,
            navigation_icons,
            settings_tabs,
            themes,
            subscriptions,
            tasks,
            deactivate_hooks,
            ..
        } = context;

        for theme in themes {
            ThemeCatalog::register_for_plugin(theme, id.clone(), cx);
        }
        Self::bind_keys(&commands, cx);
        Activation {
            commands,
            ribbon_items,
            panels,
            status_items,
            tab_bar_items,
            navigation_icons,
            settings_tabs,
            _subscriptions: subscriptions,
            _tasks: tasks,
            deactivate_hooks,
        }
    }

    fn deactivate(id: &SharedString, cx: &mut App) {
        let host = cx.global_mut::<Self>();
        let activation = host.position(id).and_then(|ix| {
            host.registrations[ix].error = None;
            host.registrations[ix].activation.take()
        });
        if let Some(activation) = activation {
            let hooks = activation.deactivate_hooks;
            for hook in hooks {
                hook(cx);
            }
        }
        ThemeCatalog::unregister_plugin(id, cx);
    }

    fn bind_keys(commands: &[Command], cx: &mut App) {
        let context = KeyBindingContextPredicate::parse(WORKSPACE_CONTEXT)
            .ok()
            .map(Rc::new);
        let mut bindings = Vec::new();
        for command in commands {
            let Some(keystrokes) = command.keybinding() else {
                continue;
            };
            let key = (keystrokes.clone(), command.id().clone());
            if Self::global(cx).bound_keys.contains(&key) {
                continue;
            }
            let action = Box::new(RunCommand {
                id: command.id().clone(),
            });
            match KeyBinding::load(keystrokes, action, context.clone(), false, None, cx) {
                Ok(binding) => {
                    bindings.push(binding);
                    cx.global_mut::<Self>().bound_keys.insert(key);
                }
                Err(error) => {
                    log::warn!(
                        "ignoring the keybinding “{keystrokes}” of {}: {error}",
                        command.id()
                    );
                }
            }
        }
        cx.bind_keys(bindings);
    }

    fn activations(&self) -> impl Iterator<Item = (&PluginManifest, &Activation)> {
        self.registrations.iter().filter_map(|registration| {
            registration
                .activation
                .as_ref()
                .map(|activation| (&registration.manifest, activation))
        })
    }

    /// The commands of the enabled plugins with the name of their plugin,
    /// sorted by plugin and command name.
    pub fn commands(&self) -> Vec<(SharedString, Command)> {
        let mut commands: Vec<(SharedString, Command)> = self
            .activations()
            .flat_map(|(manifest, activation)| {
                activation
                    .commands
                    .iter()
                    .map(|command| (manifest.name().clone(), command.clone()))
            })
            .collect();
        commands.sort_by(|a, b| (&a.0, a.1.name()).cmp(&(&b.0, b.1.name())));
        commands
    }

    pub fn command(&self, id: &str) -> Option<Command> {
        self.activations()
            .flat_map(|(_, activation)| activation.commands.iter())
            .find(|command| command.id().as_ref() == id)
            .cloned()
    }

    pub fn ribbon_items(&self) -> Vec<RibbonItem> {
        let mut items: Vec<RibbonItem> = self
            .activations()
            .flat_map(|(_, activation)| activation.ribbon_items.iter().cloned())
            .collect();
        items.sort_by(|a, b| (a.order(), a.id()).cmp(&(b.order(), b.id())));
        items
    }

    pub fn panels(&self) -> Vec<Panel> {
        let mut panels: Vec<Panel> = self
            .activations()
            .flat_map(|(_, activation)| activation.panels.iter().cloned())
            .collect();
        panels.sort_by(|a, b| (a.order(), a.id()).cmp(&(b.order(), b.id())));
        panels
    }

    /// The settings tabs of the enabled plugins with the plugin that added
    /// each, in the order the plugins were registered.
    pub fn settings_tabs(&self) -> Vec<(PluginManifest, SettingsTab)> {
        self.activations()
            .flat_map(|(manifest, activation)| {
                activation
                    .settings_tabs
                    .iter()
                    .map(|tab| (manifest.clone(), tab.clone()))
            })
            .collect()
    }

    pub fn status_items(&self) -> Vec<StatusItem> {
        let mut items: Vec<StatusItem> = self
            .activations()
            .flat_map(|(_, activation)| activation.status_items.iter().cloned())
            .collect();
        items.sort_by(|a, b| (a.order(), a.id()).cmp(&(b.order(), b.id())));
        items
    }

    /// Views at the right of the note tab bar, from lowest order to highest.
    pub fn tab_bar_items(&self) -> Vec<TabBarItem> {
        let mut items: Vec<TabBarItem> = self
            .activations()
            .flat_map(|(_, activation)| activation.tab_bar_items.iter().cloned())
            .collect();
        items.sort_by(|a, b| (a.order(), a.id()).cmp(&(b.order(), b.id())));
        items
    }

    /// The icon the enabled plugins chose for an entry of the file
    /// navigation. Plugins are asked from lowest order to highest, and the
    /// first icon is used. `expanded` is whether a folder is open.
    pub fn navigation_icon(
        &self,
        path: &VaultPath,
        kind: EntryKind,
        expanded: bool,
        cx: &App,
    ) -> Option<Icon> {
        let mut icons: Vec<&NavigationIcon> = self
            .activations()
            .flat_map(|(_, activation)| activation.navigation_icons.iter())
            .collect();
        icons.sort_by(|a, b| (a.order(), a.id()).cmp(&(b.order(), b.id())));
        icons
            .into_iter()
            .find_map(|icon| icon.resolve(path, kind, expanded, cx))
    }
}
