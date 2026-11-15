use gpui_kit::{App, SharedString, Subscription, Task};
use notia_theme::{ThemePack, ThemeSource};

use crate::{Command, NavigationIcon, Panel, RibbonItem, SettingsTab, StatusItem, TabBarItem};

pub(crate) type DeactivateHook = Box<dyn FnOnce(&mut App)>;

/// What a plugin registers while it is enabled. The host removes all of it,
/// drops the kept subscriptions and tasks, and runs the deactivation hooks
/// when the plugin is disabled.
pub struct PluginContext {
    plugin_id: SharedString,
    pub(crate) commands: Vec<Command>,
    pub(crate) ribbon_items: Vec<RibbonItem>,
    pub(crate) panels: Vec<Panel>,
    pub(crate) status_items: Vec<StatusItem>,
    pub(crate) tab_bar_items: Vec<TabBarItem>,
    pub(crate) navigation_icons: Vec<NavigationIcon>,
    pub(crate) settings_tabs: Vec<SettingsTab>,
    pub(crate) themes: Vec<ThemePack>,
    pub(crate) subscriptions: Vec<Subscription>,
    pub(crate) tasks: Vec<Task<()>>,
    pub(crate) deactivate_hooks: Vec<DeactivateHook>,
}

impl PluginContext {
    pub(crate) fn new(plugin_id: SharedString) -> Self {
        Self {
            plugin_id,
            commands: Vec::new(),
            ribbon_items: Vec::new(),
            panels: Vec::new(),
            status_items: Vec::new(),
            tab_bar_items: Vec::new(),
            navigation_icons: Vec::new(),
            settings_tabs: Vec::new(),
            themes: Vec::new(),
            subscriptions: Vec::new(),
            tasks: Vec::new(),
            deactivate_hooks: Vec::new(),
        }
    }

    pub fn plugin_id(&self) -> &SharedString {
        &self.plugin_id
    }

    pub fn add_command(&mut self, command: Command) {
        self.commands.push(command.qualified(&self.plugin_id));
    }

    pub fn add_ribbon_item(&mut self, item: RibbonItem) {
        self.ribbon_items.push(item.qualified(&self.plugin_id));
    }

    pub fn add_panel(&mut self, panel: Panel) {
        self.panels.push(panel.qualified(&self.plugin_id));
    }

    pub fn add_status_item(&mut self, item: StatusItem) {
        self.status_items.push(item.qualified(&self.plugin_id));
    }

    /// Adds a view at the right of the note tab bar.
    pub fn add_tab_bar_item(&mut self, item: TabBarItem) {
        self.tab_bar_items.push(item.qualified(&self.plugin_id));
    }

    /// Contributes icons for the file navigation. Plugins are asked from
    /// lowest order to highest; the first icon is drawn.
    pub fn add_navigation_icon(&mut self, icon: NavigationIcon) {
        self.navigation_icons.push(icon.qualified(&self.plugin_id));
    }

    /// Adds a page of the plugin's options to the settings dialog.
    pub fn add_settings_tab(&mut self, tab: SettingsTab) {
        self.settings_tabs.push(tab.qualified(&self.plugin_id));
    }

    /// Contributes a theme plugin. It appears in the appearance settings
    /// while this plugin is enabled.
    pub fn add_theme(&mut self, theme: ThemePack) {
        self.themes.push(theme);
    }

    /// Parses a theme file (see [`notia_theme`] for the format) and
    /// contributes it.
    pub fn add_theme_json(&mut self, json: &str) -> anyhow::Result<()> {
        let source = ThemeSource::Plugin(self.plugin_id.clone());
        self.themes.push(ThemePack::from_json(json, source)?);
        Ok(())
    }

    /// Keeps a subscription alive until the plugin is disabled.
    pub fn keep_subscription(&mut self, subscription: Subscription) {
        self.subscriptions.push(subscription);
    }

    /// Keeps a task running until the plugin is disabled.
    pub fn keep_task(&mut self, task: Task<()>) {
        self.tasks.push(task);
    }

    /// Runs `hook` when the plugin is disabled, after its contributions are
    /// removed.
    pub fn on_deactivate(&mut self, hook: impl FnOnce(&mut App) + 'static) {
        self.deactivate_hooks.push(Box::new(hook));
    }
}
