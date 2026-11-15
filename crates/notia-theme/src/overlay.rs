//! Color replacements laid over the active theme.
//!
//! A plugin names theme colors, the same keys as a theme file, and they
//! replace those colors of whichever theme is showing. Removing the
//! replacement brings the theme's own color back.

use std::{
    collections::{BTreeMap, HashSet},
    sync::OnceLock,
};

use gpui_kit::component::{
    ThemeConfig, ThemeConfigColors,
    highlighter::{HighlightThemeStyle, SyntaxColors},
};
use serde_json::{Map, Value};

/// Applies `overrides` onto `config`. Keys are theme color names, such as
/// `primary.background`, or `highlight.` followed by a highlight key, such
/// as `highlight.syntax.title.color`. A key or a color Notia does not know
/// is left out and named in the returned warnings. The rest are applied.
pub(crate) fn apply(config: &mut ThemeConfig, overrides: &BTreeMap<String, String>) -> Vec<String> {
    let mut warnings = Vec::new();
    for (key, value) in overrides {
        if let Err(warning) = apply_key(config, key, value) {
            warnings.push(warning);
        }
    }
    warnings
}

/// The overrides that name a real color, and a warning for each that does not.
pub(crate) fn normalize(
    overrides: BTreeMap<String, String>,
) -> (BTreeMap<String, String>, Vec<String>) {
    let mut config = ThemeConfig::default();
    let mut valid = BTreeMap::new();
    let mut warnings = Vec::new();
    for (key, value) in overrides {
        match apply_key(&mut config, &key, &value) {
            Ok(()) => {
                valid.insert(key, value);
            }
            Err(warning) => warnings.push(warning),
        }
    }
    (valid, warnings)
}

fn apply_key(config: &mut ThemeConfig, key: &str, value: &str) -> Result<(), String> {
    let key = key.trim();
    let value = value.trim();
    if key.is_empty() {
        return Err("a theme color needs a name".to_string());
    }
    if let Some(rest) = key.strip_prefix("highlight.") {
        apply_highlight(config, rest, value)
    } else {
        apply_interface_color(config, key, value)
    }
}

fn apply_interface_color(config: &mut ThemeConfig, key: &str, value: &str) -> Result<(), String> {
    if !color_keys().contains(key) {
        return Err(format!("unknown color “{key}”"));
    }
    hex_color(value)?;
    let mut colors = serde_json::to_value(&config.colors).map_err(|error| error.to_string())?;
    colors[key] = Value::String(value.to_string());
    config.colors = serde_json::from_value(colors)
        .map_err(|error| format!("could not apply “{key}”: {error}"))?;
    Ok(())
}

fn apply_highlight(config: &mut ThemeConfig, key: &str, value: &str) -> Result<(), String> {
    let mut highlight = highlight_map(config);
    if key == "syntax" {
        return Err("name a syntax color, such as highlight.syntax.title.color".to_string());
    }
    if flat_highlight_keys().contains(key) {
        hex_color(value)?;
        highlight.insert(key.to_string(), Value::String(value.to_string()));
    } else if let Some((name, field)) = syntax_assignment(key) {
        let syntax = object_entry(&mut highlight, "syntax");
        let style = style_entry(syntax, &name);
        match field.as_str() {
            "color" => {
                hex_color(value)?;
                style.insert(field, Value::String(value.to_string()));
            }
            "font_style" => {
                let style_name = value.to_ascii_lowercase();
                if !matches!(style_name.as_str(), "normal" | "italic" | "underline") {
                    return Err(format!("“{value}” is not a font style"));
                }
                style.insert(field, Value::String(style_name));
            }
            "font_weight" => {
                let weight: u16 = value
                    .parse()
                    .ok()
                    .filter(|weight| {
                        matches!(weight, 100 | 200 | 300 | 400 | 500 | 600 | 700 | 800 | 900)
                    })
                    .ok_or_else(|| format!("“{value}” is not a font weight"))?;
                style.insert(field, Value::from(weight));
            }
            _ => return Err(format!("unknown highlight color “{key}”")),
        }
    } else {
        return Err(format!("unknown highlight color “{key}”"));
    }
    config.highlight = Some(
        serde_json::from_value(Value::Object(highlight))
            .map_err(|error| format!("could not apply “highlight.{key}”: {error}"))?,
    );
    Ok(())
}

fn highlight_map(config: &ThemeConfig) -> Map<String, Value> {
    let value = match &config.highlight {
        Some(style) => serde_json::to_value(style).ok(),
        None => serde_json::to_value(HighlightThemeStyle::default()).ok(),
    };
    value
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

fn object_entry<'a>(map: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    let entry = map.entry(key).or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    entry.as_object_mut().expect("the entry is an object")
}

fn style_entry<'a>(syntax: &'a mut Map<String, Value>, name: &str) -> &'a mut Map<String, Value> {
    let entry = syntax.entry(name).or_insert_with(empty_style);
    if !entry.is_object() {
        *entry = empty_style();
    }
    let style = entry.as_object_mut().expect("the style is an object");
    style.entry("color").or_insert(Value::Null);
    style.entry("font_style").or_insert(Value::Null);
    style.entry("font_weight").or_insert(Value::Null);
    style
}

fn empty_style() -> Value {
    serde_json::json!({
        "color": null,
        "font_style": null,
        "font_weight": null,
    })
}

/// `syntax.title` and `syntax.title.color` both set the title color.
fn syntax_assignment(key: &str) -> Option<(String, String)> {
    let path = key.strip_prefix("syntax.")?;
    for field in ["font_weight", "font_style", "color"] {
        let suffix = format!(".{field}");
        if let Some(name) = path.strip_suffix(&suffix)
            && syntax_keys().contains(name)
        {
            return Some((name.to_string(), field.to_string()));
        }
    }
    syntax_keys()
        .contains(path)
        .then(|| (path.to_string(), "color".to_string()))
}

fn hex_color(value: &str) -> Result<(), String> {
    gpui_kit::Rgba::try_from(value)
        .map(|_| ())
        .map_err(|_| format!("“{value}” is not a #hex color"))
}

fn color_keys() -> &'static HashSet<String> {
    static KEYS: OnceLock<HashSet<String>> = OnceLock::new();
    KEYS.get_or_init(|| json_keys(&ThemeConfigColors::default()))
}

fn flat_highlight_keys() -> &'static HashSet<String> {
    static KEYS: OnceLock<HashSet<String>> = OnceLock::new();
    KEYS.get_or_init(|| {
        let value = serde_json::to_value(HighlightThemeStyle::default()).unwrap_or_default();
        value
            .as_object()
            .map(|object| {
                object
                    .iter()
                    .filter(|(_, value)| !value.is_object())
                    .map(|(key, _)| key.clone())
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn syntax_keys() -> &'static HashSet<String> {
    static KEYS: OnceLock<HashSet<String>> = OnceLock::new();
    KEYS.get_or_init(|| json_keys(&SyntaxColors::default()))
}

fn json_keys(value: &impl serde::Serialize) -> HashSet<String> {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::try_parse_color;

    fn overrides(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn replaces_named_colors_and_keeps_the_rest() {
        let mut config = ThemeConfig::default();
        config.colors.background = Some("#111111".into());
        config.colors.foreground = Some("#eeeeee".into());
        let warnings = apply(
            &mut config,
            &overrides(&[
                ("background", "#112233"),
                ("primary.background", "#abcdef"),
                ("not-a-color", "#112233"),
                ("foreground", "nope"),
            ]),
        );
        assert_eq!(config.colors.background.as_deref(), Some("#112233"));
        assert_eq!(config.colors.foreground.as_deref(), Some("#eeeeee"));
        assert_eq!(config.colors.primary.as_deref(), Some("#abcdef"));
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("not-a-color"))
        );
        assert!(warnings.iter().any(|warning| warning.contains("nope")));
    }

    #[test]
    fn replaces_highlight_colors() {
        let mut config = ThemeConfig::default();
        let warnings = apply(
            &mut config,
            &overrides(&[
                ("highlight.editor.background", "#112233"),
                ("highlight.syntax.title.color", "#445566"),
                ("highlight.syntax.emphasis.font_style", "italic"),
                ("highlight.syntax.missing", "#445566"),
            ]),
        );
        let highlight = config.highlight.expect("highlight was set");
        assert_eq!(
            highlight.editor_background,
            Some(try_parse_color("#112233").unwrap())
        );
        let title = serde_json::to_value(&highlight).unwrap();
        let title_color = title["syntax"]["title"]["color"].as_str().unwrap();
        assert_eq!(
            try_parse_color(title_color).unwrap(),
            try_parse_color("#445566").unwrap()
        );
        assert_eq!(title["syntax"]["emphasis"]["font_style"], "italic");
        assert!(warnings.iter().any(|warning| warning.contains("missing")));
    }

    #[test]
    fn normalize_drops_unknown_keys() {
        let (valid, warnings) = normalize(overrides(&[("background", "#abc"), ("nope", "#abc")]));
        assert_eq!(valid.get("background").map(String::as_str), Some("#abc"));
        assert!(!valid.contains_key("nope"));
        assert_eq!(warnings.len(), 1);
    }
}
