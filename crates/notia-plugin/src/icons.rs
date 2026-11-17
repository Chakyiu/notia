//! The glyph the file navigation draws for a note, a folder, or an attachment.

use gpui_kit::assets::IconName;
use notia_core::{EntryKind, VaultPath};

/// The Lucide glyph for an entry. Folders change when they are open.
/// Attachments follow the file extension.
pub fn entry_icon(path: &VaultPath, kind: EntryKind, expanded: bool) -> IconName {
    match kind {
        EntryKind::Folder if expanded => IconName::FolderOpen,
        EntryKind::Folder => IconName::FolderClosed,
        EntryKind::Note if path.is_kanban() => IconName::Kanban,
        EntryKind::Note if path.is_canvas() => IconName::Frame,
        EntryKind::Note => IconName::FileText,
        EntryKind::Attachment => attachment_icon(path.extension()),
    }
}

fn attachment_icon(extension: Option<&str>) -> IconName {
    let extension = extension.map(|ext| ext.to_ascii_lowercase());
    match extension.as_deref() {
        Some(
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" | "avif" | "heic"
            | "tif" | "tiff",
        ) => IconName::FileImage,
        Some("mp3" | "wav" | "ogg" | "flac" | "m4a" | "aac" | "aiff") => IconName::FileMusic,
        Some("mp4" | "mov" | "webm" | "mkv" | "avi" | "m4v") => IconName::FileVideoCamera,
        Some("zip" | "tar" | "gz" | "tgz" | "7z" | "rar" | "bz2" | "xz") => IconName::FileArchive,
        Some("csv" | "tsv" | "xls" | "xlsx" | "ods" | "numbers") => IconName::FileSpreadsheet,
        Some("pdf" | "doc" | "docx" | "rtf" | "odt" | "pages") => IconName::FileType,
        Some(
            "rs" | "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "py" | "go" | "css" | "scss"
            | "html" | "htm" | "json" | "toml" | "yaml" | "yml" | "sh" | "bash" | "zsh" | "c" | "h"
            | "cpp" | "hpp" | "java" | "rb" | "swift" | "kt" | "xml" | "sql" | "vue" | "svelte"
            | "lua" | "php" | "cs",
        ) => IconName::FileCode,
        Some("txt" | "text" | "log" | "md" | "markdown") => IconName::FileText,
        _ => IconName::File,
    }
}
