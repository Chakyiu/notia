use std::{fmt, sync::Arc};

use serde::{Deserialize, Serialize};

/// A path inside a vault, relative to its root and separated by `/`, such as
/// `Daily/2026-11-13.md`. The empty path is the vault root.
///
/// Construction normalizes separators and drops empty, `.` and `..`
/// segments, so a `VaultPath` can never point outside its vault.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub struct VaultPath(Arc<str>);

impl VaultPath {
    pub fn new(path: impl AsRef<str>) -> Self {
        let normalized = path.as_ref().replace('\\', "/");
        let segments: Vec<&str> = normalized
            .split('/')
            .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
            .collect();
        Self(Arc::from(segments.join("/")))
    }

    pub fn root() -> Self {
        Self(Arc::from(""))
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The last segment, including its extension.
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or_default()
    }

    /// The last segment without its extension. A leading dot is part of the
    /// stem, so `.gitignore` has the stem `.gitignore`.
    pub fn file_stem(&self) -> &str {
        let name = self.file_name();
        match name.rfind('.') {
            Some(ix) if ix > 0 => &name[..ix],
            _ => name,
        }
    }

    pub fn extension(&self) -> Option<&str> {
        let name = self.file_name();
        name.rfind('.')
            .filter(|&ix| ix > 0)
            .map(|ix| &name[ix + 1..])
    }

    pub fn is_markdown(&self) -> bool {
        self.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
    }

    /// The whole path without the `.md` extension, the form wikilinks use.
    pub fn without_markdown_extension(&self) -> &str {
        if self.is_markdown() {
            &self.0[..self.0.len() - 3]
        } else {
            &self.0
        }
    }

    pub fn parent(&self) -> Option<VaultPath> {
        if self.is_root() {
            return None;
        }
        Some(match self.0.rfind('/') {
            Some(ix) => Self(Arc::from(&self.0[..ix])),
            None => Self::root(),
        })
    }

    pub fn join(&self, relative: &str) -> VaultPath {
        if self.is_root() {
            Self::new(relative)
        } else {
            Self::new(format!("{}/{relative}", self.0))
        }
    }

    /// Replaces the last segment.
    pub fn with_file_name(&self, name: &str) -> VaultPath {
        self.parent().unwrap_or_else(Self::root).join(name)
    }

    /// Whether `self` is `ancestor` or lies inside it.
    pub fn starts_with(&self, ancestor: &VaultPath) -> bool {
        ancestor.is_root()
            || *self.0 == *ancestor.0
            || (self.0.starts_with(&*ancestor.0)
                && self.0.as_bytes().get(ancestor.0.len()) == Some(&b'/'))
    }

    /// Moves `self` from under `from` to under `to`, for renamed folders.
    pub fn rebase(&self, from: &VaultPath, to: &VaultPath) -> Option<VaultPath> {
        if !self.starts_with(from) {
            return None;
        }
        let rest = self.0[from.0.len()..].trim_start_matches('/');
        Some(if rest.is_empty() {
            to.clone()
        } else {
            to.join(rest)
        })
    }
}

impl fmt::Display for VaultPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for VaultPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VaultPath({:?})", &*self.0)
    }
}

impl From<&str> for VaultPath {
    fn from(path: &str) -> Self {
        Self::new(path)
    }
}

impl From<String> for VaultPath {
    fn from(path: String) -> Self {
        Self::new(path)
    }
}

impl From<VaultPath> for String {
    fn from(path: VaultPath) -> Self {
        path.0.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_separators_and_rejects_traversal() {
        assert_eq!(VaultPath::new("/a//b\\c.md/").as_str(), "a/b/c.md");
        assert_eq!(VaultPath::new("../../etc/passwd").as_str(), "etc/passwd");
        assert_eq!(VaultPath::new("./a/./b").as_str(), "a/b");
        assert!(VaultPath::new("").is_root());
    }

    #[test]
    fn splits_names() {
        let path = VaultPath::new("Daily/2026-11-13.md");
        assert_eq!(path.file_name(), "2026-11-13.md");
        assert_eq!(path.file_stem(), "2026-11-13");
        assert_eq!(path.extension(), Some("md"));
        assert!(path.is_markdown());
        assert_eq!(path.without_markdown_extension(), "Daily/2026-11-13");
        assert_eq!(path.parent(), Some(VaultPath::new("Daily")));
        assert_eq!(VaultPath::new("a.md").parent(), Some(VaultPath::root()));
        assert_eq!(VaultPath::root().parent(), None);
        assert_eq!(VaultPath::new(".hidden").file_stem(), ".hidden");
    }

    #[test]
    fn ancestry_respects_segment_boundaries() {
        let note = VaultPath::new("Projects/Notia/plan.md");
        assert!(note.starts_with(&VaultPath::new("Projects")));
        assert!(note.starts_with(&VaultPath::root()));
        assert!(!note.starts_with(&VaultPath::new("Proj")));
        assert_eq!(
            note.rebase(&VaultPath::new("Projects"), &VaultPath::new("Archive")),
            Some(VaultPath::new("Archive/Notia/plan.md"))
        );
    }
}
