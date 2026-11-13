use gpui_kit::SharedString;

/// Whether a plugin ships with Notia or was installed by the user.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PluginKind {
    #[default]
    Core,
    Community,
}

/// Who a plugin is. The id must be unique and stable: it keys the plugin's
/// enabled state and qualifies the ids of everything it contributes.
#[derive(Clone, Debug)]
pub struct PluginManifest {
    id: SharedString,
    name: SharedString,
    description: SharedString,
    version: SharedString,
    author: SharedString,
    kind: PluginKind,
    enabled_by_default: bool,
}

impl PluginManifest {
    pub fn new(id: impl Into<SharedString>, name: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: SharedString::default(),
            version: env!("CARGO_PKG_VERSION").into(),
            author: "Notia".into(),
            kind: PluginKind::Core,
            enabled_by_default: true,
        }
    }

    pub fn with_description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = description.into();
        self
    }

    pub fn with_version(mut self, version: impl Into<SharedString>) -> Self {
        self.version = version.into();
        self
    }

    pub fn with_author(mut self, author: impl Into<SharedString>) -> Self {
        self.author = author.into();
        self
    }

    pub fn with_kind(mut self, kind: PluginKind) -> Self {
        self.kind = kind;
        self
    }

    /// Whether a vault that never toggled the plugin has it enabled.
    pub fn enabled_by_default(mut self, enabled: bool) -> Self {
        self.enabled_by_default = enabled;
        self
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn name(&self) -> &SharedString {
        &self.name
    }

    pub fn description(&self) -> &SharedString {
        &self.description
    }

    pub fn version(&self) -> &SharedString {
        &self.version
    }

    pub fn author(&self) -> &SharedString {
        &self.author
    }

    pub fn kind(&self) -> PluginKind {
        self.kind
    }

    pub fn is_enabled_by_default(&self) -> bool {
        self.enabled_by_default
    }
}
