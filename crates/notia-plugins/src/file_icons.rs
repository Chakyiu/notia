//! Icons for the file navigation: a folder, a note, or a glyph that follows
//! the extension of an attachment.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::{App, Styled as _};
use notia_plugin::{
    NavigationIcon, Plugin, PluginContext, PluginManifest,
    core::{EntryKind, VaultPath},
};

pub struct FileIconsPlugin;

impl Plugin for FileIconsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new("file-icons", "File icons")
            .with_description(
                "Shows a folder or file icon beside each item in the file navigation.",
            )
            // Icons belong to Theme enhancer until someone turns this on.
            .enabled_by_default(false)
    }

    fn activate(&self, ctx: &mut PluginContext, _: &mut App) {
        // Asked after plugins that decorate particular files. Those return
        // None for everything else, and this icon is used.
        ctx.add_navigation_icon(NavigationIcon::new("icons", icon_for).with_order(100));
    }
}

fn icon_for(path: &VaultPath, kind: EntryKind, expanded: bool, cx: &App) -> Option<Icon> {
    let theme = cx.theme();
    let color = match kind {
        EntryKind::Folder => theme.yellow,
        EntryKind::Note => theme.sidebar_foreground,
        EntryKind::Attachment => theme.muted_foreground,
    };
    Some(
        Icon::new(glyph(path, kind, expanded))
            .small()
            .text_color(color),
    )
}

/// The Lucide glyph for an entry. Folders change when they are open.
fn glyph(path: &VaultPath, kind: EntryKind, expanded: bool) -> Lucide {
    notia_plugin::entry_icon(path, kind, expanded)
}

#[cfg(test)]
mod tests {
    use super::{Lucide, glyph};
    use notia_plugin::core::{EntryKind, VaultPath};

    #[test]
    fn folders_notes_and_attachments_use_different_icons() {
        assert_eq!(
            glyph(&VaultPath::new("Ideas"), EntryKind::Folder, false),
            Lucide::FolderClosed
        );
        assert_eq!(
            glyph(&VaultPath::new("Ideas"), EntryKind::Folder, true),
            Lucide::FolderOpen
        );
        assert_eq!(
            glyph(&VaultPath::new("Welcome.md"), EntryKind::Note, false),
            Lucide::FileText
        );
        assert_eq!(
            glyph(&VaultPath::new("shot.PNG"), EntryKind::Attachment, false),
            Lucide::FileImage
        );
        assert_eq!(
            glyph(&VaultPath::new("clip.mp3"), EntryKind::Attachment, false),
            Lucide::FileMusic
        );
        assert_eq!(
            glyph(&VaultPath::new("main.rs"), EntryKind::Attachment, false),
            Lucide::FileCode
        );
        assert_eq!(
            glyph(&VaultPath::new("data.bin"), EntryKind::Attachment, false),
            Lucide::File
        );
    }
}
