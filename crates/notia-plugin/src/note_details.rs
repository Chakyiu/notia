//! The details shown while the pointer rests on a note.

use std::time::SystemTime;

use chrono::{DateTime, Local};
use gpui_kit::component::{ActiveTheme as _, h_flex, tooltip::Tooltip, v_flex};
use gpui_kit::{
    AnyView, App, FontWeight, Hsla, IntoElement, ParentElement, Styled, Window, div, px,
};
use notia_core::VaultPath;

use crate::Notia;

const UNKNOWN_TIME: &str = "Unknown";

/// Builds the hover tooltip for `path`: its full name, then when the file was
/// created and last modified. Times are read when the tooltip opens.
pub fn note_tooltip(path: VaultPath) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |window, cx| {
        let details = NoteDetails::load(&path, cx);
        Tooltip::element(move |_, cx| details.clone().into_element(cx))
            .py_1()
            .items_start()
            .build(window, cx)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NoteDetails {
    /// Vault-relative path, including the extension the file list hides.
    name: String,
    created: String,
    modified: String,
}

impl NoteDetails {
    fn load(path: &VaultPath, cx: &App) -> Self {
        let model = Notia::vault(cx);
        let vault = model.read(cx);
        let vault = vault.vault();
        Self::from_times(path.as_str(), vault.created(path), vault.modified(path))
    }

    fn from_times(
        name: impl Into<String>,
        created: Option<SystemTime>,
        modified: Option<SystemTime>,
    ) -> Self {
        Self {
            name: name.into(),
            created: format_timestamp(created),
            modified: format_timestamp(modified),
        }
    }

    fn into_element(self, cx: &App) -> impl IntoElement + use<> {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .flex_shrink_0()
            .min_w(px(200.))
            .gap_1()
            .items_start()
            .child(
                div()
                    .whitespace_nowrap()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .child(self.name),
            )
            .child(time_row("Created", self.created, muted))
            .child(time_row("Modified", self.modified, muted))
    }
}

fn time_row(label: &'static str, value: String, muted: Hsla) -> impl IntoElement {
    h_flex()
        .w_full()
        .justify_between()
        .gap_4()
        .items_center()
        .whitespace_nowrap()
        .text_xs()
        .child(div().text_color(muted).child(label))
        .child(value)
}

fn format_timestamp(time: Option<SystemTime>) -> String {
    let Some(time) = time else {
        return UNKNOWN_TIME.to_string();
    };
    DateTime::<Local>::from(time)
        .format("%b %-d, %Y, %-I:%M %p")
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use chrono::{DateTime, Local};

    use super::{NoteDetails, format_timestamp};

    #[test]
    fn missing_time_reads_as_unknown() {
        assert_eq!(format_timestamp(None), "Unknown");
    }

    #[test]
    fn formats_the_local_clock_time() {
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let text = format_timestamp(Some(time));
        let local = DateTime::<Local>::from(time);
        assert!(text.contains(&local.format("%Y").to_string()));
        assert!(text.contains(&local.format("%-I").to_string()));
        assert!(text.contains(&local.format("%M").to_string()));
        assert!(text.ends_with(&local.format("%p").to_string()));
    }

    #[test]
    fn details_keep_the_full_path() {
        let details = NoteDetails::from_times("Daily/Standup.md", None, None);
        assert_eq!(details.name, "Daily/Standup.md");
        assert_eq!(details.created, "Unknown");
        assert_eq!(details.modified, "Unknown");
    }
}
