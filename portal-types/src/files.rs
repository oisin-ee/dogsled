//! Exact Git paths and the shared By Stage projection. No filesystem or process access.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// `bytes` retains identity only for names that are not UTF-8. `text` is their lossy display.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GitPath {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<Vec<u8>>,
}

impl GitPath {
    #[must_use]
    pub fn new(bytes: &[u8]) -> Self {
        match std::str::from_utf8(bytes) {
            Ok(text) => Self {
                text: text.into(),
                bytes: None,
            },
            Err(_) => Self {
                text: String::from_utf8_lossy(bytes).into_owned(),
                bytes: Some(bytes.to_vec()),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub path: GitPath,
    pub change: Change,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    Ordinary {
        index: char,
        worktree: char,
    },
    /// Includes both rename and copy records.
    Renamed {
        index: char,
        worktree: char,
        from: GitPath,
    },
    Conflict {
        code: [char; 2],
    },
    Untracked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Head {
    Branch(String),
    Detached { oid: String },
    Unborn { branch: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Files {
    pub root: String,
    pub head: Head,
    pub entries: Vec<Entry>,
    pub captured_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilesState {
    Pending,
    Current(Files),
    Stale { files: Files, error: String },
    NotRepository,
    TooMany,
    Unavailable(String),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Stages<'a> {
    pub conflicts: Vec<&'a Entry>,
    pub staged: Vec<&'a Entry>,
    pub unstaged: Vec<&'a Entry>,
    pub untracked: Vec<&'a Entry>,
    pub paths: usize,
}

/// A path can occur in both staged and unstaged, but contributes once to `paths`.
#[must_use]
pub fn by_stage(entries: &[Entry]) -> Stages<'_> {
    let mut stages = Stages::default();
    let mut paths = BTreeSet::new();
    for entry in entries {
        paths.insert(&entry.path);
        match entry.change {
            Change::Conflict { .. } => stages.conflicts.push(entry),
            Change::Untracked => stages.untracked.push(entry),
            Change::Ordinary { index, worktree }
            | Change::Renamed {
                index, worktree, ..
            } => {
                if index != '.' {
                    stages.staged.push(entry);
                }
                if worktree != '.' {
                    stages.unstaged.push(entry);
                }
            }
        }
    }
    for group in [
        &mut stages.conflicts,
        &mut stages.staged,
        &mut stages.unstaged,
        &mut stages.untracked,
    ] {
        group.sort_by(|a, b| a.path.cmp(&b.path));
    }
    stages.paths = paths.len();
    stages
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &[u8], change: Change) -> Entry {
        Entry {
            path: GitPath::new(name),
            change,
        }
    }

    #[test]
    fn projection_sorts_groups_and_counts_each_exact_path_once() {
        fn names(group: Vec<&Entry>) -> Vec<&str> {
            group.into_iter().map(|e| e.path.text.as_str()).collect()
        }
        let entries = vec![
            entry(
                b"z",
                Change::Ordinary {
                    index: 'M',
                    worktree: 'M',
                },
            ),
            entry(b"b", Change::Untracked),
            entry(
                b"a",
                Change::Renamed {
                    index: 'R',
                    worktree: '.',
                    from: GitPath::new(b"old"),
                },
            ),
            entry(b"c", Change::Conflict { code: ['U', 'U'] }),
            entry(
                b"a",
                Change::Ordinary {
                    index: '.',
                    worktree: 'D',
                },
            ),
            entry(b"d", Change::Conflict { code: ['D', 'D'] }),
            entry(b"aa", Change::Untracked),
        ];
        let stages = by_stage(&entries);
        assert_eq!(names(stages.conflicts), ["c", "d"]);
        assert_eq!(names(stages.staged), ["a", "z"]);
        assert_eq!(names(stages.unstaged), ["a", "z"]);
        assert_eq!(names(stages.untracked), ["aa", "b"]);
        assert_eq!(stages.paths, 6);
        assert_eq!(by_stage(&[]), Stages::default());
    }

    #[test]
    fn lossy_twins_retain_distinct_identity_and_round_trip() {
        let entries = vec![
            entry(b"\xff", Change::Untracked),
            entry(b"\xfe", Change::Untracked),
            entry("�".as_bytes(), Change::Untracked),
        ];
        assert_eq!(by_stage(&entries).paths, 3);
        assert_eq!(entries[0].path.text, entries[2].path.text);
        assert_eq!(entries[2].path.bytes, None);
        let wire = serde_json::to_vec(&entries).unwrap();
        assert_eq!(
            serde_json::from_slice::<Vec<Entry>>(&wire).unwrap(),
            entries
        );
    }
}
