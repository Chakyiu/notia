use std::sync::Arc;

use gpui_kit::component::{ActiveTheme as _, scroll::ScrollableElement as _, v_flex};
use gpui_kit::{img, *};
use notia_plugin::{Notia, core::VaultPath};

use crate::ui;

const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "tif", "tiff", "ico",
];

enum LoadState {
    Loading,
    Ready(Arc<Image>),
    Failed(SharedString),
}

pub struct ImageViewer {
    _path: VaultPath,
    state: LoadState,
    _task: Task<()>,
}

impl ImageViewer {
    pub fn new(path: VaultPath, cx: &mut Context<Self>) -> Self {
        let vault = Notia::vault(cx);
        let abs = vault.read(cx).vault().abs_path(&path);
        let state = LoadState::Loading;
        let path_label = path.clone();
        let task = cx.spawn(async move |this, cx| {
            let file = abs.clone();
            let result = smol::unblock(move || load_image(&file)).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(image) => this.state = LoadState::Ready(image),
                    Err(error) => {
                        log::error!("could not open {}: {error:#}", path_label.as_str());
                        this.state =
                            LoadState::Failed(format!("Could not open this image: {error}").into());
                    }
                }
                cx.notify();
            });
        });
        Self {
            _path: path,
            state,
            _task: task,
        }
    }
}

fn load_image(path: &std::path::Path) -> anyhow::Result<Arc<Image>> {
    let format = image_format(path)?;
    let bytes = std::fs::read(path)?;
    Ok(Arc::new(Image::from_bytes(format, bytes)))
}

pub(crate) fn image_format(path: &std::path::Path) -> anyhow::Result<ImageFormat> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .ok_or_else(|| anyhow::anyhow!("missing file extension"))?;
    match ext.to_ascii_lowercase().as_str() {
        "png" => Ok(ImageFormat::Png),
        "jpg" | "jpeg" => Ok(ImageFormat::Jpeg),
        "gif" => Ok(ImageFormat::Gif),
        "webp" => Ok(ImageFormat::Webp),
        "svg" => Ok(ImageFormat::Svg),
        "bmp" => Ok(ImageFormat::Bmp),
        "tif" | "tiff" => Ok(ImageFormat::Tiff),
        "ico" => Ok(ImageFormat::Ico),
        other => anyhow::bail!("unsupported image extension: {other}"),
    }
}

pub(crate) fn is_image_extension(ext: &str) -> bool {
    IMAGE_EXTENSIONS
        .iter()
        .any(|known| ext.eq_ignore_ascii_case(known))
}

impl Render for ImageViewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let body = match &self.state {
            LoadState::Loading => ui::empty_message("Loading image…", muted).into_any_element(),
            LoadState::Failed(message) => {
                ui::empty_message(message.as_ref(), muted).into_any_element()
            }
            LoadState::Ready(image) => img(ImageSource::Image(image.clone()))
                .w_full()
                .max_w_full()
                .id("image-viewer-content")
                .into_any_element(),
        };
        v_flex()
            .id("image-viewer")
            .size_full()
            .bg(theme.background)
            .child(body)
            .overflow_y_scrollbar()
    }
}
