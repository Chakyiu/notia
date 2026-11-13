//! Pasted images: stored in the vault folder chosen in settings and inserted
//! as a Markdown image.

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use gpui_kit::{ClipboardEntry, ClipboardItem, Image, ImageFormat, ImageSource, SharedUri};
use notia_plugin::core::{Vault, VaultPath};

const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "tif", "tiff", "ico",
];

/// Markdown to insert for the images on the clipboard, or `None` when the
/// clipboard has no image. A failed save is returned as an error so the
/// paste is still consumed instead of inserting the image as text.
pub(crate) fn pasted_images(
    vault: &Vault,
    folder: &str,
    item: &ClipboardItem,
) -> Result<Option<String>, String> {
    let mut links = Vec::new();
    let mut failed = None;
    for entry in item.entries() {
        let saved = match entry {
            ClipboardEntry::Image(image) if !image.bytes().is_empty() => {
                save_image(vault, folder, image).map(Some)
            }
            ClipboardEntry::ExternalPaths(paths) => {
                for path in paths.paths() {
                    if !is_image_file(path) {
                        continue;
                    }
                    match copy_image(vault, folder, path) {
                        Ok(stored) => links.push(markdown_image(&stored)),
                        Err(error) => failed = Some(error.to_string()),
                    }
                }
                Ok(None)
            }
            _ => Ok(None),
        };
        match saved {
            Ok(Some(path)) => links.push(markdown_image(&path)),
            Ok(None) => {}
            Err(error) => failed = Some(error.to_string()),
        }
    }
    if links.is_empty() {
        match failed {
            Some(error) => Err(error),
            None => Ok(None),
        }
    } else {
        Ok(Some(links.join("\n")))
    }
}

/// Resolves a Markdown image URL to a file. Vault-relative paths, including
/// ones written by [`pasted_images`], are read from `root`. Web and data URLs
/// stay as they are.
pub(crate) fn image_in_vault(root: PathBuf, uri: &SharedUri) -> ImageSource {
    match vault_file(&root, uri.as_ref()) {
        Some(path) => ImageSource::from(path),
        None => ImageSource::from(uri.to_string()),
    }
}

/// The file a vault-relative image URL names, or `None` for a web or data URL.
fn vault_file(root: &Path, uri: &str) -> Option<PathBuf> {
    if uri.starts_with("http:")
        || uri.starts_with("https:")
        || uri.starts_with("data:")
        || uri.starts_with("notia:")
    {
        return None;
    }
    let relative = percent_decode(uri.trim_start_matches("./"));
    Some(root.join(relative.trim_start_matches('/')))
}

fn save_image(vault: &Vault, folder: &str, image: &Image) -> io::Result<VaultPath> {
    let stem = format!("Pasted image {}", timestamp());
    let path = vault.unique_path(
        &attachment_folder(folder),
        &stem,
        Some(extension(image.format())),
    );
    vault.write_bytes(&path, image.bytes())?;
    Ok(path)
}

/// `folder` is relative to the vault. Empty means the vault root. `..` and
/// empty segments are dropped, so the folder cannot leave the vault.
fn attachment_folder(folder: &str) -> VaultPath {
    VaultPath::new(folder.trim().trim_matches('/'))
}

fn copy_image(vault: &Vault, folder: &str, source: &Path) -> io::Result<VaultPath> {
    if let Some(existing) = vault.relative_path(source) {
        return Ok(existing);
    }
    let bytes = fs::read(source)?;
    let name = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| format!("Pasted image {}", timestamp()));
    let extension = source
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("png");
    let path = vault.unique_path(&attachment_folder(folder), &name, Some(extension));
    vault.write_bytes(&path, &bytes)?;
    Ok(path)
}

fn markdown_image(path: &VaultPath) -> String {
    format!("![]({})", path.as_str().replace(' ', "%20"))
}

fn extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
        ImageFormat::Ico => "ico",
        ImageFormat::Pnm => "pnm",
    }
}

fn is_image_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

fn timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0);
    format!("{seconds}")
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut ix = 0;
    while ix < bytes.len() {
        if bytes[ix] == b'%' && ix + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[ix + 1..ix + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                ix += 3;
                continue;
            }
        }
        out.push(bytes[ix]);
        ix += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> Vault {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "notia-attachments-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Vault::open(dir).unwrap()
    }

    #[test]
    fn stores_a_pasted_image_in_the_attachments_folder() {
        let vault = vault();
        let image = Image::from_bytes(ImageFormat::Png, b"png-bytes".to_vec());
        let markdown = pasted_images(&vault, "media/images", &ClipboardItem::new_image(&image))
            .unwrap()
            .unwrap();
        assert!(markdown.starts_with("![](media/images/Pasted%20image%20"));
        assert!(markdown.ends_with(".png)"));
        let stored = vault.abs_path(&VaultPath::new(
            markdown
                .trim_start_matches("![](")
                .trim_end_matches(')')
                .replace("%20", " "),
        ));
        assert_eq!(fs::read(&stored).unwrap(), b"png-bytes");
        assert!(vault.abs_path(&VaultPath::new("media/images")).is_dir());
        let at_root = pasted_images(&vault, "", &ClipboardItem::new_image(&image))
            .unwrap()
            .unwrap();
        assert!(!at_root.contains('/'));
        let contained = pasted_images(&vault, "../outside", &ClipboardItem::new_image(&image))
            .unwrap()
            .unwrap();
        assert!(contained.starts_with("![](outside/"));
        let _ = fs::remove_dir_all(vault.root());
    }

    #[test]
    fn text_on_the_clipboard_is_not_an_image() {
        let vault = vault();
        let item = ClipboardItem::new_string("hello".into());
        assert_eq!(pasted_images(&vault, "attachments", &item).unwrap(), None);
        let _ = fs::remove_dir_all(vault.root());
    }

    #[test]
    fn resolves_vault_images_and_leaves_web_images() {
        let root = PathBuf::from("/vault");
        assert_eq!(
            vault_file(&root, "attachments/Pasted%20image.png"),
            Some(root.join("attachments/Pasted image.png"))
        );
        assert_eq!(vault_file(&root, "https://example.com/a.png"), None);
    }
}
