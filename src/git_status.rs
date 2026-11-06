//! Bounded, byte-preserving Git status capture for one checkout. No reader-side caching.

use crate::error::{Error, Result};
use crate::shell::Shell;
use portal_types::files::{Change, Entry, Files, FilesState, GitPath, Head};
use std::time::Duration;

const STATUS_LIMIT: usize = 1024 * 1024;

pub(crate) fn read(shell: &dyn Shell, cwd: Option<&str>, timeout: Duration) -> FilesState {
    let Some(cwd) = cwd else {
        return FilesState::Unavailable("checkout directory unavailable".into());
    };
    match capture(shell, cwd, timeout) {
        Ok(state) => state,
        Err(Error::OutputLimit { .. }) => FilesState::TooMany,
        Err(error) => FilesState::Unavailable(error.to_string()),
    }
}

fn words(root: &str, args: &[&str]) -> Vec<String> {
    let mut words = vec!["--no-optional-locks".into(), "-C".into(), root.into()];
    words.extend(args.iter().map(ToString::to_string));
    words
}

fn capture(shell: &dyn Shell, cwd: &str, timeout: Duration) -> Result<FilesState> {
    // System's shared command boundary removes inherited Git repository/index routing.
    let root = shell.run_bounded(
        "git",
        &words(cwd, &["rev-parse", "--show-toplevel"]),
        timeout,
    )?;
    if !root.ok() && root.stderr.contains("not a git repository") {
        return Ok(FilesState::NotRepository);
    }
    let root = crate::shell::checked("git", &[], root)?;
    let root = root.stdout.strip_suffix('\n').unwrap_or(&root.stdout);
    if root.is_empty() {
        return Err(invalid("missing checkout root"));
    }
    let args = words(
        root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=normal",
        ],
    );
    let status = shell.run_bytes_bounded("git", &args, timeout, STATUS_LIMIT)?;
    if status.status != 0 {
        return Err(Error::Command {
            program: "git".into(),
            args: args.join(" "),
            status: status.status.to_string(),
            stderr: status.stderr,
        });
    }
    let (head, entries) = parse(&status.stdout)?;
    Ok(FilesState::Current(Files {
        root: root.into(),
        head,
        entries,
        captured_ms: crate::ledger::now_ms(),
    }))
}

fn invalid(detail: &str) -> Error {
    Error::Parse {
        origin: "git status".into(),
        detail: detail.into(),
    }
}

fn text(bytes: &[u8]) -> Result<&str> {
    std::str::from_utf8(bytes).map_err(|_| invalid("non-UTF-8 status metadata"))
}

fn xy(bytes: &[u8]) -> Result<[char; 2]> {
    match bytes {
        [index, worktree] if bytes.iter().all(|b| b".MTADRCU".contains(b)) => {
            Ok([char::from(*index), char::from(*worktree)])
        }
        _ => Err(invalid("invalid XY status")),
    }
}

/// Fixed metadata fields are space-separated; the last field is an exact, unquoted path.
fn fields(record: &[u8], count: usize) -> Result<Vec<&[u8]>> {
    let fields: Vec<_> = record.splitn(count, |b| *b == b' ').collect();
    if fields.len() != count || fields.iter().any(|field| field.is_empty()) {
        return Err(invalid("incomplete status record"));
    }
    Ok(fields)
}

fn tracked(
    record: &[u8],
    records: &mut std::slice::Split<'_, u8, impl FnMut(&u8) -> bool>,
) -> Result<Entry> {
    let kind = record
        .first()
        .copied()
        .ok_or_else(|| invalid("empty record"))?;
    let count = match kind {
        b'1' => 9,
        b'2' => 10,
        b'u' => 11,
        _ => return Err(invalid("unknown tracked record")),
    };
    let fields = fields(record, count)?;
    if fields[0].len() != 1 {
        return Err(invalid("unknown tracked record"));
    }
    let code = xy(fields[1])?;
    let change = match kind {
        b'1' => Change::Ordinary {
            index: code[0],
            worktree: code[1],
        },
        b'2' => {
            let score = fields[8];
            if !matches!(score.first(), Some(b'R' | b'C'))
                || score.len() < 2
                || !score[1..].iter().all(u8::is_ascii_digit)
            {
                return Err(invalid("invalid rename/copy score"));
            }
            let from = records
                .next()
                .filter(|path| !path.is_empty())
                .ok_or_else(|| invalid("missing rename/copy source"))?;
            Change::Renamed {
                index: code[0],
                worktree: code[1],
                from: GitPath::new(from),
            }
        }
        b'u' => {
            const UNMERGED: [[char; 2]; 7] = [
                ['D', 'D'],
                ['A', 'U'],
                ['U', 'D'],
                ['U', 'A'],
                ['D', 'U'],
                ['A', 'A'],
                ['U', 'U'],
            ];
            if !UNMERGED.contains(&code) {
                return Err(invalid("invalid unmerged code"));
            }
            Change::Conflict { code }
        }
        _ => return Err(invalid("unknown tracked record")),
    };
    Ok(Entry {
        path: GitPath::new(fields[count - 1]),
        change,
    })
}

fn parse(bytes: &[u8]) -> Result<(Head, Vec<Entry>)> {
    if !bytes.ends_with(&[0]) {
        return Err(invalid("status is not NUL terminated"));
    }
    let mut oid = None;
    let mut branch = None;
    let mut entries = Vec::new();
    let mut records = bytes[..bytes.len() - 1].split(|b| *b == 0);
    while let Some(record) = records.next() {
        if let Some(value) = record.strip_prefix(b"# branch.oid ") {
            if oid.replace(text(value)?).is_some() {
                return Err(invalid("duplicate branch oid"));
            }
        } else if let Some(value) = record.strip_prefix(b"# branch.head ") {
            if branch.replace(text(value)?).is_some() {
                return Err(invalid("duplicate branch head"));
            }
        } else if record.starts_with(b"# branch.upstream ") || record.starts_with(b"# branch.ab ") {
            // Optional branch metadata does not affect Files.
        } else if let Some(path) = record.strip_prefix(b"? ") {
            if path.is_empty() {
                return Err(invalid("empty untracked path"));
            }
            entries.push(Entry {
                path: GitPath::new(path),
                change: Change::Untracked,
            });
        } else if record.starts_with(b"! ") {
            // Ignored paths never appear in Files, even if supplied by a producer.
        } else if matches!(record.first(), Some(b'1' | b'2' | b'u')) {
            entries.push(tracked(record, &mut records)?);
        } else {
            return Err(invalid("unknown status record"));
        }
    }
    let oid = oid
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid("missing branch oid"))?;
    let branch = branch
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid("missing branch head"))?;
    let head = match (oid, branch) {
        ("(initial)", branch) => Head::Unborn {
            branch: branch.into(),
        },
        (oid, "(detached)") => Head::Detached { oid: oid.into() },
        (_, branch) => Head::Branch(branch.into()),
    };
    Ok((head, entries))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::{GIT_LOCAL_ENV, System};
    use std::path::PathBuf;

    struct Repo(PathBuf);

    impl Repo {
        fn new(committed: bool) -> Self {
            let repo = Self(std::env::temp_dir().join(crate::ledger::new_id()));
            std::fs::create_dir_all(&repo.0).unwrap();
            repo.git(&["init", "-q", "-b", "main"]);
            if committed {
                repo.write("file", "base\n");
                repo.git(&["add", "."]);
                repo.git(&["commit", "-qm", "base"]);
            }
            repo
        }

        fn git(&self, args: &[&str]) -> String {
            let mut words = Vec::new();
            for key in GIT_LOCAL_ENV {
                words.extend(["-u".into(), (*key).into()]);
            }
            words.extend([
                "git".into(),
                "-C".into(),
                self.0.to_str().unwrap().into(),
                "-c".into(),
                "user.name=Fixture".into(),
                "-c".into(),
                "user.email=fixture@example.invalid".into(),
            ]);
            words.extend(args.iter().map(ToString::to_string));
            System::default().run_ok("env", &words).unwrap().stdout
        }

        fn write(&self, name: &str, content: &str) {
            std::fs::write(self.0.join(name), content).unwrap();
        }

        fn files_at(path: &std::path::Path) -> Files {
            match read(&System::default(), path.to_str(), Duration::from_secs(10)) {
                FilesState::Current(files) => files,
                state => panic!("expected current Files: {state:?}"),
            }
        }

        fn files(&self) -> Files {
            Self::files_at(&self.0)
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn clean_and_both_index_and_worktree_changes() {
        let repo = Repo::new(true);
        let clean = repo.files();
        assert_eq!(clean.head, Head::Branch("main".into()));
        assert_eq!(clean.entries, []);
        assert!(clean.captured_ms > 0);
        repo.write("file", "staged\n");
        repo.git(&["add", "file"]);
        repo.write("file", "unstaged\n");
        assert_eq!(
            repo.files().entries,
            [Entry {
                path: GitPath::new(b"file"),
                change: Change::Ordinary {
                    index: 'M',
                    worktree: 'M'
                },
            }]
        );
    }

    #[test]
    fn deletion_and_rename_keep_exact_source() {
        let repo = Repo::new(true);
        std::fs::remove_file(repo.0.join("file")).unwrap();
        assert_eq!(
            repo.files().entries[0].change,
            Change::Ordinary {
                index: '.',
                worktree: 'D'
            }
        );
        repo.git(&["add", "file"]);
        assert_eq!(
            repo.files().entries[0].change,
            Change::Ordinary {
                index: 'D',
                worktree: '.'
            }
        );
        repo.write("new name\n", "base\n");
        repo.git(&["add", "."]);
        assert_eq!(
            repo.files().entries,
            [Entry {
                path: GitPath::new(b"new name\n"),
                change: Change::Renamed {
                    index: 'R',
                    worktree: '.',
                    from: GitPath::new(b"file")
                },
            }]
        );
    }

    #[test]
    fn all_seven_unmerged_codes_come_from_real_indexes() {
        use std::fmt::Write as _;
        let repo = Repo::new(true);
        let oid = repo.git(&["rev-parse", "HEAD:file"]).trim().to_owned();
        let codes = [
            ("DD", vec![1]),
            ("AU", vec![2]),
            ("UD", vec![1, 2]),
            ("UA", vec![3]),
            ("DU", vec![1, 3]),
            ("AA", vec![2, 3]),
            ("UU", vec![1, 2, 3]),
        ];
        let mut input = String::new();
        for (code, stages) in &codes {
            repo.write(code, "conflicted\n");
            for stage in stages {
                writeln!(input, "100644 {oid} {stage}\t{code}").unwrap();
            }
        }
        let out = System::default()
            .run_input_bounded(
                "git",
                &words(repo.0.to_str().unwrap(), &["update-index", "--index-info"]),
                input.as_bytes(),
                Duration::from_secs(5),
            )
            .unwrap();
        assert!(out.ok(), "{}", out.stderr);
        let files = repo.files();
        assert_eq!(files.entries.len(), 7);
        for (code, _) in codes {
            let entry = files.entries.iter().find(|e| e.path.text == code).unwrap();
            let mut chars = code.chars();
            assert_eq!(
                entry.change,
                Change::Conflict {
                    code: [chars.next().unwrap(), chars.next().unwrap()]
                }
            );
        }
    }

    #[test]
    fn untracked_directory_is_one_row_and_ignored_paths_are_absent() {
        let repo = Repo::new(true);
        repo.write("loose", "untracked");
        std::fs::create_dir_all(repo.0.join("directory")).unwrap();
        repo.write("directory/one", "one");
        repo.write("directory/two", "two");
        repo.write(".gitignore", "ignored\n");
        repo.git(&["add", ".gitignore"]);
        repo.git(&["commit", "-qm", "ignore"]);
        repo.write("ignored", "ignored");
        let entries = repo.files().entries;
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.change == Change::Untracked));
        assert_eq!(
            entries
                .iter()
                .map(|e| e.path.text.as_str())
                .collect::<Vec<_>>(),
            ["directory/", "loose"]
        );
    }

    #[test]
    fn non_utf8_path_and_its_lossy_twin_are_distinct() {
        use std::os::unix::ffi::OsStrExt as _;
        let repo = Repo::new(true);
        std::fs::write(
            repo.0.join(std::ffi::OsStr::from_bytes(b"name\xff")),
            "bytes",
        )
        .unwrap();
        repo.write("name�", "utf8");
        let entries = repo.files().entries;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path.text, entries[1].path.text);
        assert_ne!(entries[0].path, entries[1].path);
        assert_eq!(portal_types::files::by_stage(&entries).paths, 2);
        assert!(
            entries
                .iter()
                .any(|e| e.path.bytes.as_deref() == Some(b"name\xff"))
        );
        assert!(entries.iter().any(|e| e.path.bytes.is_none()));
    }

    #[test]
    fn unborn_detached_and_linked_worktree_heads() {
        let unborn = Repo::new(false);
        assert_eq!(
            unborn.files().head,
            Head::Unborn {
                branch: "main".into()
            }
        );
        let repo = Repo::new(true);
        let oid = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();
        repo.git(&["checkout", "--detach", "-q"]);
        assert_eq!(repo.files().head, Head::Detached { oid });
        let tree = repo.0.join("linked");
        repo.git(&["worktree", "add", "-qb", "feature", tree.to_str().unwrap()]);
        let files = Repo::files_at(&tree);
        assert_eq!(files.root, tree.to_str().unwrap());
        assert_eq!(files.head, Head::Branch("feature".into()));
        assert_eq!(files.entries, []);
        std::fs::create_dir_all(tree.join("subdir")).unwrap();
        assert_eq!(Repo::files_at(&tree.join("subdir")).root, files.root);
    }

    #[test]
    fn not_repository_and_missing_cwd_are_explicit() {
        let outside = std::env::temp_dir().join(crate::ledger::new_id());
        std::fs::create_dir_all(&outside).unwrap();
        assert_eq!(
            read(&System::default(), outside.to_str(), Duration::from_secs(5)),
            FilesState::NotRepository
        );
        std::fs::remove_dir_all(outside).unwrap();
        assert!(matches!(
            read(&System::default(), None, Duration::from_secs(5)),
            FilesState::Unavailable(_)
        ));
    }

    #[test]
    fn oversized_real_status_is_too_many() {
        let repo = Repo::new(false);
        for n in 0..5000 {
            repo.write(&format!("{n:04}-{}", "x".repeat(220)), "");
        }
        assert_eq!(
            read(&System::default(), repo.0.to_str(), Duration::from_secs(10)),
            FilesState::TooMany
        );
    }

    #[test]
    fn malformed_records_fail_instead_of_disappearing() {
        for record in [
            b"x unknown\0".as_slice(),
            b"1 M. short\0",
            b"? \0",
            b"? unterminated",
            b"u .. N... 0 0 0 0 a b c path\0",
            b"2 R. N... 0 0 0 a b R100 path\0",
        ] {
            let mut bytes = b"# branch.oid abc\0# branch.head main\0".to_vec();
            bytes.extend(record);
            assert!(parse(&bytes).is_err(), "{record:?}");
        }
        let (_, entries) = parse(b"# branch.oid abc\0# branch.head main\0! ignored\0").unwrap();
        assert_eq!(entries, []);
        let (_, entries) = parse(b"# branch.oid abc\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +0 -0\x002 C. N... 100644 100644 100644 a b C100 copy\0original\0").unwrap();
        assert_eq!(
            entries[0].change,
            Change::Renamed {
                index: 'C',
                worktree: '.',
                from: GitPath::new(b"original")
            }
        );
    }
}
