use std::{
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use anyhow::{Context as _, ensure};
use gpui_kit::{
    Pixels, SharedString,
    component::{ThemeConfig, ThemeMode},
    px,
};
use serde::Deserialize;

/// Who a theme plugin is and where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeManifest {
    pub id: SharedString,
    pub name: SharedString,
    pub version: Option<SharedString>,
    pub author: Option<SharedString>,
    pub description: Option<SharedString>,
    pub url: Option<SharedString>,
}

/// Where a theme was loaded from. When several sources provide the same
/// theme id, a vault folder overrides a plugin, which overrides the bundle.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThemeSource {
    Bundled,
    Plugin(SharedString),
    Folder(PathBuf),
}

impl ThemeSource {
    pub(crate) fn rank(&self) -> u8 {
        match self {
            Self::Bundled => 0,
            Self::Plugin(_) => 1,
            Self::Folder(_) => 2,
        }
    }

    pub(crate) fn same_kind(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Plugin(a), Self::Plugin(b)) => a == b,
            _ => self.rank() == other.rank(),
        }
    }
}

/// The reading and writing typography a theme chooses for notes.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorTheme {
    /// The note font. `None` uses the interface font.
    pub font_family: Option<SharedString>,
    pub font_size: Pixels,
    /// Line height as a multiple of the font size.
    pub line_height: f32,
    /// The readable line length.
    pub max_width: Pixels,
}

impl Default for EditorTheme {
    fn default() -> Self {
        Self {
            font_family: None,
            font_size: px(16.),
            line_height: 1.6,
            max_width: px(720.),
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct EditorThemeFile {
    #[serde(rename = "editor.font.family")]
    font_family: Option<String>,
    #[serde(rename = "editor.font.size")]
    font_size: Option<f32>,
    #[serde(rename = "editor.line_height")]
    line_height: Option<f32>,
    #[serde(rename = "editor.max_width")]
    max_width: Option<f32>,
}

impl From<EditorThemeFile> for EditorTheme {
    fn from(file: EditorThemeFile) -> Self {
        let defaults = Self::default();
        Self {
            font_family: file
                .font_family
                .filter(|f| !f.trim().is_empty())
                .map(Into::into),
            font_size: file
                .font_size
                .map_or(defaults.font_size, |size| px(size.clamp(8., 40.))),
            line_height: file
                .line_height
                .map_or(defaults.line_height, |h| h.clamp(1., 3.)),
            max_width: file
                .max_width
                .map_or(defaults.max_width, |w| px(w.clamp(320., 2400.))),
        }
    }
}

/// One appearance of a theme: its light or its dark half.
#[derive(Clone, Debug)]
pub struct ThemeVariant {
    config: Rc<ThemeConfig>,
    editor: EditorTheme,
}

impl ThemeVariant {
    pub fn mode(&self) -> ThemeMode {
        self.config.mode
    }

    /// The GPUI Component theme this variant installs.
    pub fn config(&self) -> &Rc<ThemeConfig> {
        &self.config
    }

    pub fn editor(&self) -> &EditorTheme {
        &self.editor
    }
}

/// A theme plugin: a manifest with a light variant, a dark variant, or both.
#[derive(Clone, Debug)]
pub struct ThemePack {
    manifest: ThemeManifest,
    source: ThemeSource,
    light: Option<ThemeVariant>,
    dark: Option<ThemeVariant>,
}

#[derive(Deserialize)]
struct ThemePackFile {
    id: Option<String>,
    name: String,
    version: Option<String>,
    author: Option<String>,
    description: Option<String>,
    url: Option<String>,
    themes: Vec<serde_json::Value>,
}

impl ThemePack {
    pub fn from_json(json: &str, source: ThemeSource) -> anyhow::Result<Self> {
        let file: ThemePackFile = serde_json::from_str(json).context("invalid theme file")?;
        let name = file.name.trim().to_string();
        ensure!(!name.is_empty(), "the theme has no name");
        let id = file
            .id
            .map(|id| slug(&id))
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| slug(&name));

        let mut pack = Self {
            manifest: ThemeManifest {
                id: id.into(),
                name: name.clone().into(),
                version: file.version.map(Into::into),
                author: file.author.map(Into::into),
                description: file.description.map(Into::into),
                url: file.url.map(Into::into),
            },
            source,
            light: None,
            dark: None,
        };

        for (ix, mut value) in file.themes.into_iter().enumerate() {
            let editor = match value
                .as_object_mut()
                .and_then(|object| object.remove("notia"))
            {
                Some(extras) => serde_json::from_value::<EditorThemeFile>(extras)
                    .with_context(|| format!("invalid `notia` settings in variant {}", ix + 1))?,
                None => EditorThemeFile::default(),
            };
            let mut config: ThemeConfig = serde_json::from_value(value)
                .with_context(|| format!("invalid variant {}", ix + 1))?;
            if config.name.trim().is_empty() {
                let mode = if config.mode.is_dark() {
                    "Dark"
                } else {
                    "Light"
                };
                config.name = format!("{name} {mode}").into();
            }
            let variant = ThemeVariant {
                config: Rc::new(config),
                editor: editor.into(),
            };
            let slot = if variant.mode().is_dark() {
                &mut pack.dark
            } else {
                &mut pack.light
            };
            if slot.is_none() {
                *slot = Some(variant);
            }
        }
        ensure!(
            pack.light.is_some() || pack.dark.is_some(),
            "the theme has no light or dark variant"
        );
        Ok(pack)
    }

    /// Loads every theme in `dir`: `name/theme.json` folders and loose
    /// `name.json` files. Files that fail to load are reported, not skipped
    /// silently.
    pub fn load_folder(dir: &Path) -> (Vec<ThemePack>, Vec<(PathBuf, anyhow::Error)>) {
        let mut packs = Vec::new();
        let mut errors = Vec::new();
        let Ok(entries) = fs::read_dir(dir) else {
            return (packs, errors);
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter_map(|path| {
                if path.is_dir() {
                    Some(path.join("theme.json")).filter(|file| file.is_file())
                } else {
                    path.extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
                        .then_some(path)
                }
            })
            .collect();
        files.sort();

        for file in files {
            let source = ThemeSource::Folder(file.clone());
            match fs::read_to_string(&file)
                .map_err(anyhow::Error::from)
                .and_then(|json| Self::from_json(&json, source))
            {
                Ok(pack) => packs.push(pack),
                Err(error) => errors.push((file, error)),
            }
        }
        (packs, errors)
    }

    pub fn manifest(&self) -> &ThemeManifest {
        &self.manifest
    }

    pub fn id(&self) -> &SharedString {
        &self.manifest.id
    }

    pub fn name(&self) -> &SharedString {
        &self.manifest.name
    }

    pub fn source(&self) -> &ThemeSource {
        &self.source
    }

    pub(crate) fn with_source(mut self, source: ThemeSource) -> Self {
        self.source = source;
        self
    }

    pub fn has_mode(&self, mode: ThemeMode) -> bool {
        self.exact_variant(mode).is_some()
    }

    pub fn exact_variant(&self, mode: ThemeMode) -> Option<&ThemeVariant> {
        if mode.is_dark() {
            self.dark.as_ref()
        } else {
            self.light.as_ref()
        }
    }

    /// The variant for `mode`, or the theme's only variant when it does not
    /// have one for that mode.
    pub fn variant(&self, mode: ThemeMode) -> &ThemeVariant {
        self.exact_variant(mode)
            .or(self.light.as_ref())
            .or(self.dark.as_ref())
            .expect("a theme pack always has a variant")
    }
}

/// A file-name-safe identifier: lowercase letters, digits and dashes.
fn slug(text: &str) -> String {
    let mut slug = String::with_capacity(text.len());
    for ch in text.trim().chars() {
        if ch.is_alphanumeric() {
            slug.extend(ch.to_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACK: &str = r##"{
        "name": "Paper Ink",
        "author": "Tester",
        "themes": [
            { "mode": "dark", "colors": { "background": "#101010" } },
            {
                "name": "Paper",
                "mode": "light",
                "colors": { "background": "#fdfcf8" },
                "notia": { "editor.font.size": 18, "editor.line_height": 9 }
            }
        ]
    }"##;

    #[test]
    fn parses_variants_and_extras() {
        let pack = ThemePack::from_json(PACK, ThemeSource::Bundled).unwrap();
        assert_eq!(pack.id().as_ref(), "paper-ink");
        assert_eq!(
            pack.variant(ThemeMode::Dark).config().name.as_ref(),
            "Paper Ink Dark"
        );
        let light = pack.variant(ThemeMode::Light);
        assert_eq!(light.config().name.as_ref(), "Paper");
        assert_eq!(light.editor().font_size, px(18.));
        assert_eq!(light.editor().line_height, 3.);
        assert_eq!(
            pack.variant(ThemeMode::Dark).editor(),
            &EditorTheme::default()
        );
    }

    #[test]
    fn falls_back_to_the_only_variant() {
        let pack = ThemePack::from_json(
            r#"{ "id": "Solo Theme!", "name": "Solo", "themes": [{ "mode": "light" }] }"#,
            ThemeSource::Bundled,
        )
        .unwrap();
        assert_eq!(pack.id().as_ref(), "solo-theme");
        assert!(!pack.has_mode(ThemeMode::Dark));
        assert_eq!(pack.variant(ThemeMode::Dark).mode(), ThemeMode::Light);
    }

    #[test]
    fn rejects_files_without_variants() {
        assert!(
            ThemePack::from_json(r#"{ "name": "Empty", "themes": [] }"#, ThemeSource::Bundled)
                .is_err()
        );
        assert!(ThemePack::from_json(r#"{ "themes": [] }"#, ThemeSource::Bundled).is_err());
    }
}
