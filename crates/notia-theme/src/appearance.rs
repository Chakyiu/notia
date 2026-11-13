use gpui_kit::{App, Global, Pixels, SharedString, px};
use serde::{Deserialize, Serialize};

use crate::EditorTheme;

/// The theme every vault starts with.
pub const DEFAULT_THEME_ID: &str = "notia";

/// Whether to show a theme's light or dark variant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorScheme {
    Light,
    Dark,
    /// Follow the operating system.
    #[default]
    System,
}

/// The appearance preferences of a vault, persisted with its settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    /// The id of the active theme plugin.
    pub theme: String,
    pub scheme: ColorScheme,
    /// The interface's base font size. Every rem-based size scales with it,
    /// so it doubles as the interface zoom.
    pub interface_font_size: f32,
    /// Replaces the theme's note font size.
    pub editor_font_size: Option<f32>,
    /// Replaces the theme's note font.
    pub editor_font_family: Option<String>,
    /// Limits note text to the theme's readable line length.
    pub readable_line_length: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: DEFAULT_THEME_ID.to_string(),
            scheme: ColorScheme::System,
            interface_font_size: 15.,
            editor_font_size: None,
            editor_font_family: None,
            readable_line_length: true,
        }
    }
}

/// The note typography in effect: the active theme's choices with the
/// user's overrides applied. Notes read it when they render.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorAppearance {
    pub font_family: SharedString,
    pub font_size: Pixels,
    /// Line height as a multiple of the font size.
    pub line_height: f32,
    /// The readable line length, or `None` to use the full width.
    pub max_width: Option<Pixels>,
}

impl Global for EditorAppearance {}

impl Default for EditorAppearance {
    fn default() -> Self {
        let theme = EditorTheme::default();
        Self {
            font_family: ".SystemUIFont".into(),
            font_size: theme.font_size,
            line_height: theme.line_height,
            max_width: Some(theme.max_width),
        }
    }
}

impl EditorAppearance {
    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub(crate) fn resolve(
        theme: &EditorTheme,
        appearance: &Appearance,
        interface_font: &SharedString,
    ) -> Self {
        let font_family = appearance
            .editor_font_family
            .as_deref()
            .map(str::trim)
            .filter(|family| !family.is_empty())
            .map(SharedString::from)
            .or_else(|| theme.font_family.clone())
            .unwrap_or_else(|| interface_font.clone());
        Self {
            font_family,
            font_size: appearance
                .editor_font_size
                .map_or(theme.font_size, |size| px(size.clamp(10., 32.))),
            line_height: theme.line_height,
            max_width: appearance.readable_line_length.then_some(theme.max_width),
        }
    }
}
