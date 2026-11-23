//! Opens common image attachments in a scrollable in-app viewer.

extern crate gpui_kit as gpui;

mod ui;
mod viewer;

use gpui::{App, AppContext as _};
use notia_plugin::{FileOpener, Plugin, PluginContext, PluginManifest, core::VaultPath};

use viewer::ImageViewer;

pub struct ImageReaderPlugin;

impl Plugin for ImageReaderPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new("image-reader", "Image Reader")
            .with_description("Opens PNG, JPEG, GIF, WebP, and other image files from the vault.")
    }

    fn activate(&self, ctx: &mut PluginContext, _: &mut App) {
        ctx.add_file_opener(FileOpener::new("image", |path: &VaultPath, _window, cx| {
            if !path.extension().is_some_and(viewer::is_image_extension) {
                return None;
            }
            Some(cx.new(|cx| ImageViewer::new(path.clone(), cx)).into())
        }));
    }
}

#[cfg(test)]
mod tests {
    use gpui::ImageFormat;

    use crate::viewer::{image_format, is_image_extension};

    #[test]
    fn maps_known_extensions() {
        assert!(matches!(
            image_format(std::path::Path::new("a.PNG")).unwrap(),
            ImageFormat::Png
        ));
        assert!(matches!(
            image_format(std::path::Path::new("b.JpEg")).unwrap(),
            ImageFormat::Jpeg
        ));
        assert!(image_format(std::path::Path::new("c.txt")).is_err());
    }

    #[test]
    fn recognizes_image_extensions() {
        assert!(is_image_extension("webp"));
        assert!(!is_image_extension("pdf"));
    }
}
