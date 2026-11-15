use std::{
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

use gpui_kit::{AnyView, App, Entity, Render, SharedString, Window, component::Icon};
use notia_core::{EntryKind, VaultPath};
use serde::{Deserialize, Serialize};

pub(crate) type Handler = Rc<dyn Fn(&mut Window, &mut App)>;
type ViewBuilder = Rc<dyn Fn(&mut Window, &mut App) -> AnyView>;
type Predicate = Rc<dyn Fn(&App) -> bool>;
type NavigationIconFn = dyn Fn(&VaultPath, EntryKind, bool, &App) -> Option<Icon>;

fn next_instance() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Prefixes a contribution id with its plugin id, so that two plugins can
/// use the same local ids.
pub(crate) fn qualify(id: &SharedString, plugin_id: &str) -> SharedString {
    if id.contains(':') {
        id.clone()
    } else {
        format!("{plugin_id}:{id}").into()
    }
}

/// An action listed in the command palette and optionally bound to a key.
#[derive(Clone)]
pub struct Command {
    id: SharedString,
    name: SharedString,
    icon: Option<Icon>,
    keybinding: Option<SharedString>,
    available: Option<Predicate>,
    handler: Handler,
}

impl Command {
    pub fn new(
        id: impl Into<SharedString>,
        name: impl Into<SharedString>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            icon: None,
            keybinding: None,
            available: None,
            handler: Rc::new(handler),
        }
    }

    pub fn with_icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Binds the command to keystrokes such as `secondary-shift-d`, where
    /// `secondary` is Command on macOS and Control elsewhere.
    pub fn with_keybinding(mut self, keystrokes: impl Into<SharedString>) -> Self {
        self.keybinding = Some(keystrokes.into());
        self
    }

    /// Lists the command only while `predicate` holds, for example only
    /// while a note is open.
    pub fn available_when(mut self, predicate: impl Fn(&App) -> bool + 'static) -> Self {
        self.available = Some(Rc::new(predicate));
        self
    }

    /// The qualified id, `plugin-id:command-id`.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn name(&self) -> &SharedString {
        &self.name
    }

    pub fn icon(&self) -> Option<&Icon> {
        self.icon.as_ref()
    }

    pub fn keybinding(&self) -> Option<&SharedString> {
        self.keybinding.as_ref()
    }

    pub fn is_available(&self, cx: &App) -> bool {
        self.available
            .as_ref()
            .is_none_or(|available| available(cx))
    }

    pub fn run(&self, window: &mut Window, cx: &mut App) {
        (self.handler)(window, cx)
    }

    pub(crate) fn qualified(mut self, plugin_id: &str) -> Self {
        self.id = qualify(&self.id, plugin_id);
        self
    }
}

/// A button on the ribbon, the strip of icons along the left edge.
#[derive(Clone)]
pub struct RibbonItem {
    id: SharedString,
    icon: Icon,
    tooltip: SharedString,
    order: i32,
    handler: Handler,
}

impl RibbonItem {
    pub fn new(
        id: impl Into<SharedString>,
        icon: impl Into<Icon>,
        tooltip: impl Into<SharedString>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            icon: icon.into(),
            tooltip: tooltip.into(),
            order: 0,
            handler: Rc::new(handler),
        }
    }

    /// Lower orders sit higher on the ribbon.
    pub fn with_order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn icon(&self) -> &Icon {
        &self.icon
    }

    pub fn tooltip(&self) -> &SharedString {
        &self.tooltip
    }

    pub fn order(&self) -> i32 {
        self.order
    }

    pub fn run(&self, window: &mut Window, cx: &mut App) {
        (self.handler)(window, cx)
    }

    pub(crate) fn qualified(mut self, plugin_id: &str) -> Self {
        self.id = qualify(&self.id, plugin_id);
        self
    }
}

/// Where a panel is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelPlacement {
    Left,
    Right,
    /// A bar above the open note, rather than a sidebar.
    Editor,
}

/// A view docked in a sidebar, selected by its icon in the sidebar header.
#[derive(Clone)]
pub struct Panel {
    id: SharedString,
    instance: u64,
    title: SharedString,
    icon: Icon,
    placement: PanelPlacement,
    order: i32,
    build: ViewBuilder,
}

impl Panel {
    /// `build` creates the panel's view the first time a window shows it.
    pub fn new<V: Render>(
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        icon: impl Into<Icon>,
        placement: PanelPlacement,
        build: impl Fn(&mut Window, &mut App) -> Entity<V> + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            instance: next_instance(),
            title: title.into(),
            icon: icon.into(),
            placement,
            order: 0,
            build: Rc::new(move |window, cx| build(window, cx).into()),
        }
    }

    /// Unique to this panel and its clones. A plugin that is reloaded
    /// contributes a panel with the same id but another instance, whose view
    /// must be built anew.
    pub fn instance(&self) -> u64 {
        self.instance
    }

    /// Lower orders come first in the sidebar header.
    pub fn with_order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn title(&self) -> &SharedString {
        &self.title
    }

    pub fn icon(&self) -> &Icon {
        &self.icon
    }

    pub fn placement(&self) -> PanelPlacement {
        self.placement
    }

    pub fn order(&self) -> i32 {
        self.order
    }

    pub fn build(&self, window: &mut Window, cx: &mut App) -> AnyView {
        (self.build)(window, cx)
    }

    pub(crate) fn qualified(mut self, plugin_id: &str) -> Self {
        self.id = qualify(&self.id, plugin_id);
        self
    }
}

/// A view in the status bar at the bottom right of the window.
#[derive(Clone)]
pub struct StatusItem {
    id: SharedString,
    instance: u64,
    order: i32,
    build: ViewBuilder,
}

impl StatusItem {
    pub fn new<V: Render>(
        id: impl Into<SharedString>,
        build: impl Fn(&mut Window, &mut App) -> Entity<V> + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            instance: next_instance(),
            order: 0,
            build: Rc::new(move |window, cx| build(window, cx).into()),
        }
    }

    /// Unique to this item and its clones; see [`Panel::instance`].
    pub fn instance(&self) -> u64 {
        self.instance
    }

    /// Lower orders sit further left.
    pub fn with_order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn order(&self) -> i32 {
        self.order
    }

    pub fn build(&self, window: &mut Window, cx: &mut App) -> AnyView {
        (self.build)(window, cx)
    }

    pub(crate) fn qualified(mut self, plugin_id: &str) -> Self {
        self.id = qualify(&self.id, plugin_id);
        self
    }
}

/// A view at the right of the note tab bar. It can draw nothing, so a plugin
/// can offer the item and let the user hide it.
#[derive(Clone)]
pub struct TabBarItem {
    id: SharedString,
    instance: u64,
    order: i32,
    build: ViewBuilder,
}

impl TabBarItem {
    pub fn new<V: Render>(
        id: impl Into<SharedString>,
        build: impl Fn(&mut Window, &mut App) -> Entity<V> + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            instance: next_instance(),
            order: 0,
            build: Rc::new(move |window, cx| build(window, cx).into()),
        }
    }

    /// Unique to this item and its clones; see [`Panel::instance`].
    pub fn instance(&self) -> u64 {
        self.instance
    }

    /// Lower orders sit further left.
    pub fn with_order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn order(&self) -> i32 {
        self.order
    }

    pub fn build(&self, window: &mut Window, cx: &mut App) -> AnyView {
        (self.build)(window, cx)
    }

    pub(crate) fn qualified(mut self, plugin_id: &str) -> Self {
        self.id = qualify(&self.id, plugin_id);
        self
    }
}

/// An icon for one entry of the file navigation.
///
/// Several plugins may contribute one. They are asked from lowest
/// [`order`](Self::order) to highest, and the first icon is drawn, so a
/// plugin can decorate the files it knows and leave the rest alone.
#[derive(Clone)]
pub struct NavigationIcon {
    id: SharedString,
    order: i32,
    resolve: Rc<NavigationIconFn>,
}

impl NavigationIcon {
    /// `resolve` is called for each visible entry. `expanded` is true when
    /// a folder is open. Return `None` to leave the entry to another plugin.
    pub fn new(
        id: impl Into<SharedString>,
        resolve: impl Fn(&VaultPath, EntryKind, bool, &App) -> Option<Icon> + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            order: 0,
            resolve: Rc::new(resolve),
        }
    }

    /// Lower orders are asked first.
    pub fn with_order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn order(&self) -> i32 {
        self.order
    }

    pub(crate) fn resolve(
        &self,
        path: &VaultPath,
        kind: EntryKind,
        expanded: bool,
        cx: &App,
    ) -> Option<Icon> {
        (self.resolve)(path, kind, expanded, cx)
    }

    pub(crate) fn qualified(mut self, plugin_id: &str) -> Self {
        self.id = qualify(&self.id, plugin_id);
        self
    }
}
