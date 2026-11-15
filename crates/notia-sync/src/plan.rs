//! Three-way comparison of this computer, the server, and the last sync.

use std::collections::{BTreeMap, BTreeSet};

use crate::{paths::conflict_path, snapshot::FileRev};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Download,
    Upload,
    DeleteLocal,
    DeleteRemote,
    Conflict,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub kind: ChangeKind,
    pub local: Option<FileRev>,
    pub remote: Option<FileRev>,
    pub conflict_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub changes: Vec<Change>,
    /// Paths that already match. The caller stores this revision as the base.
    pub aligned: Vec<(String, FileRev)>,
    /// Base entries whose file is gone on both sides.
    pub forget: Vec<String>,
}

/// Decide what to transfer.
///
/// `held` paths are left completely alone, including their base entry. A
/// missing base means the file was never synced from this computer: a file
/// that exists on only one side is copied, and a file that differs on both
/// sides is a conflict.
pub fn plan(
    local: &BTreeMap<String, FileRev>,
    remote: &BTreeMap<String, FileRev>,
    base: &BTreeMap<String, FileRev>,
    held: &BTreeSet<String>,
    stamp: &str,
) -> Plan {
    let mut paths = BTreeSet::new();
    paths.extend(local.keys().cloned());
    paths.extend(remote.keys().cloned());
    paths.extend(base.keys().cloned());

    let mut taken: BTreeSet<String> = local.keys().chain(remote.keys()).cloned().collect();
    let mut plan = Plan {
        changes: Vec::new(),
        aligned: Vec::new(),
        forget: Vec::new(),
    };

    for path in paths {
        if held.contains(&path) {
            continue;
        }
        let local_rev = local.get(&path);
        let remote_rev = remote.get(&path);
        let base_hash = base.get(&path).map(|rev| rev.sha256.as_str());
        match (local_rev, remote_rev) {
            (Some(local_rev), Some(remote_rev)) if local_rev.sha256 == remote_rev.sha256 => {
                plan.aligned.push((path, local_rev.clone()));
            }
            (Some(local_rev), Some(remote_rev)) => {
                if base_hash == Some(local_rev.sha256.as_str()) {
                    plan.changes.push(change(
                        path,
                        ChangeKind::Download,
                        Some(local_rev),
                        Some(remote_rev),
                        None,
                    ));
                } else if base_hash == Some(remote_rev.sha256.as_str()) {
                    plan.changes.push(change(
                        path,
                        ChangeKind::Upload,
                        Some(local_rev),
                        Some(remote_rev),
                        None,
                    ));
                } else {
                    let conflict = conflict_path(&path, stamp, &mut taken);
                    plan.changes.push(change(
                        path,
                        ChangeKind::Conflict,
                        Some(local_rev),
                        Some(remote_rev),
                        Some(conflict),
                    ));
                }
            }
            (Some(local_rev), None) => {
                let kind = if base_hash == Some(local_rev.sha256.as_str()) {
                    ChangeKind::DeleteLocal
                } else {
                    ChangeKind::Upload
                };
                plan.changes
                    .push(change(path, kind, Some(local_rev), None, None));
            }
            (None, Some(remote_rev)) => {
                let kind = if base_hash == Some(remote_rev.sha256.as_str()) {
                    ChangeKind::DeleteRemote
                } else {
                    ChangeKind::Download
                };
                plan.changes
                    .push(change(path, kind, None, Some(remote_rev), None));
            }
            (None, None) => plan.forget.push(path),
        }
    }
    plan
}

fn change(
    path: String,
    kind: ChangeKind,
    local: Option<&FileRev>,
    remote: Option<&FileRev>,
    conflict_path: Option<String>,
) -> Change {
    Change {
        path,
        kind,
        local: local.cloned(),
        remote: remote.cloned(),
        conflict_path,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::snapshot::FileRev;

    fn rev(hash: &str) -> FileRev {
        FileRev {
            sha256: hash.into(),
            size: hash.len() as u64,
            mtime_ms: 1,
        }
    }

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, FileRev> {
        pairs
            .iter()
            .map(|(path, hash)| ((*path).to_string(), rev(hash)))
            .collect()
    }

    fn kinds(plan: &Plan) -> Vec<(&str, ChangeKind)> {
        plan.changes
            .iter()
            .map(|change| (change.path.as_str(), change.kind))
            .collect()
    }

    fn decide(local: &[(&str, &str)], remote: &[(&str, &str)], base: &[(&str, &str)]) -> Plan {
        plan(&map(local), &map(remote), &map(base), &BTreeSet::new(), "T")
    }

    #[test]
    fn first_sync_copies_whichever_side_has_the_file() {
        let plan = decide(&[("A.md", "a")], &[("B.md", "b")], &[]);
        assert_eq!(
            kinds(&plan),
            vec![("A.md", ChangeKind::Upload), ("B.md", ChangeKind::Download),]
        );
    }

    #[test]
    fn matching_files_are_aligned_without_a_base() {
        let plan = decide(&[("A.md", "a")], &[("A.md", "a")], &[]);
        assert!(plan.changes.is_empty());
        assert_eq!(plan.aligned.len(), 1);
    }

    #[test]
    fn edits_follow_the_side_that_changed() {
        let upload = decide(&[("A.md", "new")], &[("A.md", "old")], &[("A.md", "old")]);
        assert_eq!(kinds(&upload), vec![("A.md", ChangeKind::Upload)]);

        let download = decide(&[("A.md", "old")], &[("A.md", "new")], &[("A.md", "old")]);
        assert_eq!(kinds(&download), vec![("A.md", ChangeKind::Download)]);
    }

    #[test]
    fn both_sides_changing_keeps_a_conflict_copy() {
        let plan = decide(
            &[("Note.md", "local")],
            &[("Note.md", "remote")],
            &[("Note.md", "base")],
        );
        assert_eq!(plan.changes.len(), 1);
        let change = &plan.changes[0];
        assert_eq!(change.kind, ChangeKind::Conflict);
        assert_eq!(
            change.conflict_path.as_deref(),
            Some("Note.sync-conflict-T.md")
        );
    }

    #[test]
    fn a_delete_of_an_unchanged_file_propagates() {
        let remote_delete = decide(&[("A.md", "a")], &[], &[("A.md", "a")]);
        assert_eq!(
            kinds(&remote_delete),
            vec![("A.md", ChangeKind::DeleteLocal)]
        );

        let local_delete = decide(&[], &[("A.md", "a")], &[("A.md", "a")]);
        assert_eq!(
            kinds(&local_delete),
            vec![("A.md", ChangeKind::DeleteRemote)]
        );
    }

    #[test]
    fn an_edit_wins_over_a_delete_on_the_other_side() {
        let edited = decide(&[("A.md", "new")], &[], &[("A.md", "old")]);
        assert_eq!(kinds(&edited), vec![("A.md", ChangeKind::Upload)]);

        let restored = decide(&[], &[("A.md", "new")], &[("A.md", "old")]);
        assert_eq!(kinds(&restored), vec![("A.md", ChangeKind::Download)]);
    }

    #[test]
    fn a_held_file_is_not_deleted_or_forgotten() {
        let mut held = BTreeSet::new();
        held.insert("big.bin".into());
        let plan = plan(
            &BTreeMap::new(),
            &map(&[("big.bin", "old")]),
            &map(&[("big.bin", "old")]),
            &held,
            "T",
        );
        assert!(plan.changes.is_empty());
        assert!(plan.forget.is_empty());
    }

    #[test]
    fn a_base_entry_for_a_file_gone_on_both_sides_is_forgotten() {
        let plan = decide(&[], &[], &[("A.md", "a")]);
        assert_eq!(plan.forget, vec!["A.md".to_string()]);
    }
}
