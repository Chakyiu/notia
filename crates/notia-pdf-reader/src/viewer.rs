use std::sync::Arc;

use gpui_kit::component::{ActiveTheme as _, scroll::ScrollableElement as _, v_flex};
use gpui_kit::{img, *};
use notia_plugin::{Notia, core::VaultPath};
use pdf_oxide::PdfDocument;
use pdf_oxide::rendering::{RenderOptions, render_page};

use crate::ui;

enum LoadState {
    Loading,
    Ready(Vec<Arc<Image>>),
    Failed(SharedString),
}

pub struct PdfViewer {
    _path: VaultPath,
    state: LoadState,
    _task: Task<()>,
}

impl PdfViewer {
    pub fn new(path: VaultPath, cx: &mut Context<Self>) -> Self {
        let vault = Notia::vault(cx);
        let abs = vault.read(cx).vault().abs_path(&path);
        let state = LoadState::Loading;
        let path_label = path.clone();
        let task = cx.spawn(async move |this, cx| {
            let file = abs.clone();
            let result = smol::unblock(move || load_pages(&file)).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(images) => this.state = LoadState::Ready(images),
                    Err(error) => {
                        log::error!("could not open {}: {error:#}", path_label.as_str());
                        this.state =
                            LoadState::Failed(format!("Could not open this PDF: {error}").into());
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

fn load_pages(path: &std::path::Path) -> anyhow::Result<Vec<Arc<Image>>> {
    let mut doc = PdfDocument::open(path)?;
    let page_count = doc.page_count()?;
    let options = RenderOptions::with_dpi(144);
    let mut pages = Vec::with_capacity(page_count);
    for index in 0..page_count {
        let rendered = render_page(&mut doc, index, &options)?;
        pages.push(Arc::new(Image::from_bytes(
            ImageFormat::Png,
            rendered.as_bytes().to_vec(),
        )));
    }
    Ok(pages)
}

impl Render for PdfViewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let body = match &self.state {
            LoadState::Loading => ui::empty_message("Loading PDF…", muted).into_any_element(),
            LoadState::Failed(message) => {
                ui::empty_message(message.as_ref(), muted).into_any_element()
            }
            LoadState::Ready(pages) if pages.is_empty() => {
                ui::empty_message("This PDF has no pages.", muted).into_any_element()
            }
            LoadState::Ready(pages) => v_flex()
                .gap_4()
                .p_4()
                .children(pages.iter().enumerate().map(|(ix, image)| {
                    img(ImageSource::Image(image.clone()))
                        .w_full()
                        .max_w_full()
                        .id(("pdf-page", ix))
                }))
                .into_any_element(),
        };
        v_flex()
            .id("pdf-viewer")
            .size_full()
            .bg(theme.background)
            .child(body)
            .overflow_y_scrollbar()
    }
}
