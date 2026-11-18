//! Pages of plugin options in the settings dialog.
//!
//! A plugin describes its page as sections of [`Setting`]s, each a name and
//! a control that reads and changes a value. The settings dialog draws them
//! like its own pages, so a plugin needs nothing from the dialog itself:
//!
//! ```ignore
//! ctx.add_settings_tab(SettingsTab::new("options", |_| {
//!     vec![SettingsSection::new().setting(
//!         Setting::toggle(
//!             "Show seconds",
//!             |cx| Clock::global(cx).seconds,
//!             |seconds, cx| Clock::update(cx, |clock| clock.seconds = seconds),
//!         )
//!         .with_description("In the status bar."),
//!     )]
//! }));
//! ```

use std::{ops::RangeInclusive, rc::Rc};

use gpui_kit::{AnyElement, App, SharedString, Window, component::Icon};

use crate::contribution::qualify;

type Build = Rc<dyn Fn(&mut App) -> Vec<SettingsSection>>;
type Render = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;
type Get<T> = Rc<dyn Fn(&App) -> T>;
type Set<T> = Rc<dyn Fn(T, &mut App)>;

/// A page of a plugin's options in the settings dialog, listed while the
/// plugin is enabled. Its sections are built again whenever the dialog
/// draws, so they can show the current state.
#[derive(Clone)]
pub struct SettingsTab {
    id: SharedString,
    title: Option<SharedString>,
    icon: Option<Icon>,
    build: Build,
}

impl SettingsTab {
    pub fn new(
        id: impl Into<SharedString>,
        build: impl Fn(&mut App) -> Vec<SettingsSection> + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            title: None,
            icon: None,
            build: Rc::new(build),
        }
    }

    /// The page's name in the dialog. Defaults to the plugin's name.
    pub fn with_title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// The page's icon in the dialog's sidebar.
    pub fn with_icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// The qualified id, `plugin-id:tab-id`.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn title(&self) -> Option<&SharedString> {
        self.title.as_ref()
    }

    pub fn icon(&self) -> Option<&Icon> {
        self.icon.as_ref()
    }

    pub fn build(&self, cx: &mut App) -> Vec<SettingsSection> {
        (self.build)(cx)
    }

    pub(crate) fn qualified(mut self, plugin_id: &str) -> Self {
        self.id = qualify(&self.id, plugin_id);
        self
    }
}

/// Settings under an optional heading. The dialog leaves out a section
/// without settings.
#[derive(Clone, Default)]
pub struct SettingsSection {
    title: Option<SharedString>,
    description: Option<SharedString>,
    settings: Vec<Setting>,
}

impl SettingsSection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn setting(mut self, setting: Setting) -> Self {
        self.settings.push(setting);
        self
    }

    pub fn settings(mut self, settings: impl IntoIterator<Item = Setting>) -> Self {
        self.settings.extend(settings);
        self
    }

    pub fn title(&self) -> Option<&SharedString> {
        self.title.as_ref()
    }

    pub fn description(&self) -> Option<&SharedString> {
        self.description.as_ref()
    }

    pub fn items(&self) -> &[Setting] {
        &self.settings
    }
}

/// One row of a settings page: a name, and the control beside it.
#[derive(Clone)]
pub struct Setting {
    name: SharedString,
    description: Option<SharedString>,
    control: SettingControl,
}

impl Setting {
    /// A switch.
    pub fn toggle(
        name: impl Into<SharedString>,
        get: impl Fn(&App) -> bool + 'static,
        set: impl Fn(bool, &mut App) + 'static,
    ) -> Self {
        Self::new(name, SettingControl::Toggle(SettingValue::new(get, set)))
    }

    /// A text field. `set` runs as the text changes.
    pub fn text(
        name: impl Into<SharedString>,
        get: impl Fn(&App) -> SharedString + 'static,
        set: impl Fn(SharedString, &mut App) + 'static,
    ) -> Self {
        Self::new(name, SettingControl::Text(SettingValue::new(get, set)))
    }

    /// A text field that hides its characters. A button in the field shows
    /// the value, and hides it again. `set` runs as the text changes, with
    /// the real value, including while it is hidden.
    pub fn secret(
        name: impl Into<SharedString>,
        get: impl Fn(&App) -> SharedString + 'static,
        set: impl Fn(SharedString, &mut App) + 'static,
    ) -> Self {
        Self::new(name, SettingControl::Secret(SettingValue::new(get, set)))
    }

    /// A number field that keeps the value within `range`, and steps by
    /// `step` with the arrow keys.
    pub fn number(
        name: impl Into<SharedString>,
        range: RangeInclusive<f64>,
        step: f64,
        get: impl Fn(&App) -> f64 + 'static,
        set: impl Fn(f64, &mut App) + 'static,
    ) -> Self {
        Self::new(
            name,
            SettingControl::Number {
                value: SettingValue::new(get, set),
                range,
                step,
            },
        )
    }

    /// A menu of `(value, label)` options.
    pub fn choice<V, L>(
        name: impl Into<SharedString>,
        options: impl IntoIterator<Item = (V, L)>,
        get: impl Fn(&App) -> SharedString + 'static,
        set: impl Fn(SharedString, &mut App) + 'static,
    ) -> Self
    where
        V: Into<SharedString>,
        L: Into<SharedString>,
    {
        Self::new(
            name,
            SettingControl::Choice {
                value: SettingValue::new(get, set),
                options: options
                    .into_iter()
                    .map(|(value, label)| (value.into(), label.into()))
                    .collect(),
            },
        )
    }

    /// A setting whose control is any element, such as buttons.
    pub fn custom(
        name: impl Into<SharedString>,
        render: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        Self::new(name, SettingControl::Custom(Rc::new(render)))
    }

    /// An element across the page, without a name, such as an explanation.
    pub fn row(render: impl Fn(&mut Window, &mut App) -> AnyElement + 'static) -> Self {
        Self::custom(SharedString::default(), render)
    }

    fn new(name: impl Into<SharedString>, control: SettingControl) -> Self {
        Self {
            name: name.into(),
            description: None,
            control,
        }
    }

    /// Text under the name.
    pub fn with_description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Empty for a [`Setting::row`].
    pub fn name(&self) -> &SharedString {
        &self.name
    }

    pub fn description(&self) -> Option<&SharedString> {
        self.description.as_ref()
    }

    pub fn control(&self) -> &SettingControl {
        &self.control
    }
}

/// What a [`Setting`] shows beside its name.
#[derive(Clone)]
pub enum SettingControl {
    Toggle(SettingValue<bool>),
    Text(SettingValue<SharedString>),
    /// Like [`Text`](Self::Text), drawn with the characters hidden until shown.
    Secret(SettingValue<SharedString>),
    Number {
        value: SettingValue<f64>,
        range: RangeInclusive<f64>,
        step: f64,
    },
    Choice {
        value: SettingValue<SharedString>,
        /// `(value, label)` pairs.
        options: Vec<(SharedString, SharedString)>,
    },
    Custom(Render),
}

/// How a control reads the value it shows, and changes it.
pub struct SettingValue<T> {
    get: Get<T>,
    set: Set<T>,
}

impl<T> SettingValue<T> {
    pub fn new(get: impl Fn(&App) -> T + 'static, set: impl Fn(T, &mut App) + 'static) -> Self {
        Self {
            get: Rc::new(get),
            set: Rc::new(set),
        }
    }

    pub fn get(&self, cx: &App) -> T {
        (self.get)(cx)
    }

    pub fn set(&self, value: T, cx: &mut App) {
        (self.set)(value, cx)
    }
}

impl<T> Clone for SettingValue<T> {
    fn clone(&self) -> Self {
        Self {
            get: self.get.clone(),
            set: self.set.clone(),
        }
    }
}
