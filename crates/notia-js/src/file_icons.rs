//! Folder and file icons a community plugin substitutes for the built-in ones.
//!
//! The built-in file icons are a fallback. A plugin that sets its own is
//! asked first, so a folder pack and icon colors replace the yellow folder
//! without hiding extension icons for attachments.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gpui_kit::{
    App, AssetSource as _, Hsla, Styled as _,
    component::{ActiveTheme as _, Icon, Sizable as _, try_parse_color},
};
use notia_plugin::{
    NavigationIcon,
    core::{EntryKind, VaultPath},
    entry_icon,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::runtime::{Contribution, JsRuntime};

/// Asked before the built-in file icons, which use order 100.
const ORDER: i32 = 40;

/// The icons a plugin is currently drawing, shared with the navigation
/// callback so changing a color does not register the callback again.
#[derive(Default)]
pub(crate) struct FileIconSlot {
    style: Rc<RefCell<Option<FileIconStyle>>>,
    added: Cell<bool>,
}

#[derive(Clone, PartialEq)]
struct FileIconStyle {
    folder: String,
    folder_open: String,
    folder_color: Option<Hsla>,
    note_color: Option<Hsla>,
    file_color: Option<Hsla>,
    folders: Vec<FolderIcon>,
}

#[derive(Clone, PartialEq)]
struct FolderIcon {
    path: String,
    icon: String,
    open: String,
    color: Option<Hsla>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileIconSpec {
    #[serde(default)]
    folder: String,
    #[serde(default)]
    folder_open: String,
    #[serde(default)]
    folder_color: Option<String>,
    #[serde(default)]
    note_color: Option<String>,
    #[serde(default)]
    file_color: Option<String>,
    #[serde(default)]
    folders: Vec<FolderSpec>,
}

#[derive(Deserialize)]
struct FolderSpec {
    #[serde(default)]
    path: String,
    #[serde(default)]
    icon: String,
    #[serde(default)]
    open: String,
    #[serde(default)]
    color: Option<String>,
}

impl JsRuntime {
    pub(crate) fn set_file_icons(&self, spec: Value, cx: &mut App) -> Result<Value, String> {
        if spec.is_null() {
            let state = self.state.borrow();
            let changed = state.file_icons.style.borrow().is_some();
            *state.file_icons.style.borrow_mut() = None;
            drop(state);
            if changed {
                cx.refresh_windows();
            }
            return Ok(json!({ "warnings": [] }));
        }
        let spec: FileIconSpec =
            serde_json::from_value(spec).map_err(|error| format!("file icons: {error}"))?;
        let (style, warnings) = style_from(spec);
        let (changed, install) = {
            let state = self.state.borrow();
            let slot = &state.file_icons;
            let changed = slot.style.borrow().as_ref() != Some(&style);
            *slot.style.borrow_mut() = Some(style);
            let install = !slot.added.get();
            if install {
                slot.added.set(true);
            }
            (changed, install.then(|| slot.style.clone()))
        };
        if let Some(style) = install {
            self.add(Contribution::NavigationIcon(
                NavigationIcon::new("icons", move |path, kind, expanded, cx| {
                    style
                        .borrow()
                        .as_ref()
                        .and_then(|style| render_icon(style, path, kind, expanded, cx))
                })
                .with_order(ORDER),
            ));
        }
        if changed {
            cx.refresh_windows();
        }
        Ok(json!({ "warnings": warnings }))
    }
}

fn style_from(spec: FileIconSpec) -> (FileIconStyle, Vec<String>) {
    let mut warnings = Vec::new();
    let mut folders = Vec::new();
    for rule in spec.folders {
        let path = rule.path.trim().trim_matches('/').to_string();
        if path.is_empty() {
            warnings.push("a folder icon needs a path".to_string());
            continue;
        }
        let (color, warning) = optional_color(rule.color.as_deref(), &format!("folder “{path}”"));
        if let Some(warning) = warning {
            warnings.push(warning);
        }
        folders.push(FolderIcon {
            path,
            icon: rule.icon.trim().to_string(),
            open: rule.open.trim().to_string(),
            color,
        });
    }
    let (folder_color, warning) = optional_color(spec.folder_color.as_deref(), "the folder icon");
    if let Some(warning) = warning {
        warnings.push(warning);
    }
    let (note_color, warning) = optional_color(spec.note_color.as_deref(), "the note icon");
    if let Some(warning) = warning {
        warnings.push(warning);
    }
    let (file_color, warning) = optional_color(spec.file_color.as_deref(), "the file icon");
    if let Some(warning) = warning {
        warnings.push(warning);
    }
    (
        FileIconStyle {
            folder: spec.folder.trim().to_string(),
            folder_open: spec.folder_open.trim().to_string(),
            folder_color,
            note_color,
            file_color,
            folders,
        },
        warnings,
    )
}

fn optional_color(value: Option<&str>, what: &str) -> (Option<Hsla>, Option<String>) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return (None, None);
    };
    match try_parse_color(value) {
        Ok(color) => (Some(color), None),
        Err(_) => (None, Some(format!("“{value}” is not a color for {what}"))),
    }
}

fn render_icon(
    style: &FileIconStyle,
    path: &VaultPath,
    kind: EntryKind,
    expanded: bool,
    cx: &App,
) -> Option<Icon> {
    let theme = cx.theme();
    let (name, color) = match kind {
        EntryKind::Folder => {
            let rule = folder_rule(style, path);
            let custom = rule.and_then(|rule| {
                let name = if expanded && !rule.open.is_empty() {
                    &rule.open
                } else {
                    &rule.icon
                };
                (!name.is_empty()).then(|| name.as_str())
            });
            let fallback = if expanded && !style.folder_open.is_empty() {
                &style.folder_open
            } else if !expanded && !style.folder.is_empty() {
                &style.folder
            } else {
                ""
            };
            let color = rule
                .and_then(|rule| rule.color)
                .or(style.folder_color)
                .unwrap_or(theme.yellow);
            (
                custom.or(Some(fallback).filter(|name| !name.is_empty())),
                color,
            )
        }
        EntryKind::Note => (None, style.note_color.unwrap_or(theme.sidebar_foreground)),
        EntryKind::Attachment => (None, style.file_color.unwrap_or(theme.muted_foreground)),
    };
    let icon = name
        .and_then(named_icon)
        .unwrap_or_else(|| Icon::new(entry_icon(path, kind, expanded)));
    Some(icon.small().text_color(color))
}

fn folder_rule<'a>(style: &'a FileIconStyle, path: &VaultPath) -> Option<&'a FolderIcon> {
    let path = path.as_str().trim_matches('/');
    style.folders.iter().rev().find(|rule| rule.path == path)
}

fn named_icon(name: &str) -> Option<Icon> {
    let path = format!("icons/{name}.svg");
    gpui_kit::assets::AllAssets
        .load(&path)
        .ok()
        .flatten()
        .map(|_| Icon::empty().path(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_rule_matches_its_path() {
        let style = FileIconStyle {
            folder: "folder-closed".into(),
            folder_open: "folder-open".into(),
            folder_color: None,
            note_color: None,
            file_color: None,
            folders: vec![FolderIcon {
                path: "Ideas".into(),
                icon: "folder-heart".into(),
                open: String::new(),
                color: None,
            }],
        };
        assert_eq!(
            folder_rule(&style, &VaultPath::new("Ideas")).map(|rule| rule.icon.as_str()),
            Some("folder-heart")
        );
        assert!(folder_rule(&style, &VaultPath::new("Daily")).is_none());
        // The later rule for a path wins, which is what the CSS file does.
        let style = FileIconStyle {
            folders: vec![
                FolderIcon {
                    path: "Ideas".into(),
                    icon: "folder-heart".into(),
                    open: String::new(),
                    color: None,
                },
                FolderIcon {
                    path: "Ideas".into(),
                    icon: "folder-lock".into(),
                    open: String::new(),
                    color: None,
                },
            ],
            ..style
        };
        assert_eq!(
            folder_rule(&style, &VaultPath::new("/Ideas/")).map(|rule| rule.icon.as_str()),
            Some("folder-lock")
        );
    }

    #[test]
    fn unknown_icon_colors_are_warnings() {
        let (_style, warnings) = style_from(FileIconSpec {
            folder: "folder".into(),
            folder_open: String::new(),
            folder_color: Some("nope".into()),
            note_color: Some("#e0af68".into()),
            file_color: None,
            folders: vec![FolderSpec {
                path: "  ".into(),
                icon: String::new(),
                open: String::new(),
                color: None,
            }],
        });
        assert!(warnings.iter().any(|warning| warning.contains("nope")));
        assert!(warnings.iter().any(|warning| warning.contains("path")));
    }

    #[test]
    fn a_known_lucide_name_resolves() {
        assert!(named_icon("folder-heart").is_some());
        assert!(named_icon("not-an-icon").is_none());
    }
}
