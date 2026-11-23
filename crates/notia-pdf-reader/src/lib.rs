//! Opens `.pdf` attachments in a scrollable in-app viewer.

extern crate gpui_kit as gpui;

mod ui;
mod viewer;

use gpui::{App, AppContext as _};
use notia_plugin::{
    FileOpener, Plugin, PluginContext, PluginManifest, core::VaultPath,
};

use viewer::PdfViewer;

pub struct PdfReaderPlugin;

impl Plugin for PdfReaderPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new("pdf-reader", "PDF Reader")
            .with_description("Opens PDF files from the vault with scrollable pages.")
    }

    fn activate(&self, ctx: &mut PluginContext, _: &mut App) {
        ctx.add_file_opener(FileOpener::new(
            "pdf",
            |path: &VaultPath, _window, cx| {
                if !path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
                {
                    return None;
                }
                Some(cx.new(|cx| PdfViewer::new(path.clone(), cx)).into())
            },
        ));
    }
}