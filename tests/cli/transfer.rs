//! Transfers store files or changes for another agent and announce them once. Nothing is applied.

use super::close::git;
use super::*;
use std::os::unix::fs::symlink;

const PANES: [(&str, &str, &str); 3] = [
    ("w1:p1", "term_lead", "lead"),
    ("w1:p2", "term_work", "worker"),
    ("w1:p3", "term_other", "other"),
];

/// Herdr's agent list and this host's snapshot, both with terminal ids.
fn agents(rig: &Rig, panes: &[(&str, &str, &str)]) {
    let listed: Vec<Value> = panes
        .iter()
        .map(|(pane, terminal, name)| {
            json!({"pane_id":pane,"agent":"pi","agent_status":"idle","name":name,"terminal_id":terminal})
        })
        .collect();
    let published: Vec<Value> = panes
        .iter()
        .map(|(pane, terminal, name)| {
            json!({"pane":pane,"agent":"pi","state":"idle","name":name,"transcript":null,
                "transcript_modified":null,"terminal_id":terminal})
        })
        .collect();
    rig.put("agents.json", &json!({"result":{"agents":listed}}));
    fs::create_dir_all(rig.dir.join("state")).unwrap();
    rig.put_host_at(
        "state",
        &json!({"sampled_ms":now(),"probe":{"host":"local","label":"local","load1":0.0,"cores":8,
            "memory":{"used_bytes":0,"total_bytes":1},"disk_free_percent":50.0,
            "agents":{"total":panes.len(),"by_status":{}},"agent_panes":published,
            "reservations":[],"panes":[]}}),
    );
}

/// Run as the agent in `pane`, in `cwd` when given.
fn as_pane(rig: &Rig, pane: &str, cwd: Option<&Path>, args: &[&str]) -> (i32, Value) {
    let mut command = rig.cmd(args);
    command.arg("--json").env("HERDR_PANE_ID", pane);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let out = command.output().unwrap();
    let value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out.status.code().unwrap(), value)
}

/// A rig with three agents, where the worker reads its inbox.
fn rig(name: &str) -> (Rig, super::observation::Supplied) {
    let rig = Rig::new(name);
    agents(&rig, &PANES);
    let publisher = super::observation::resolved(&rig);
    let (code, claimed) = as_pane(&rig, "w1:p2", None, &["inbox", "claim", "--harness", "pi"]);
    assert_eq!((code, claimed), (0, json!([])));
    (rig, publisher)
}

/// The stored transfers, without unfinished or parked directories.
fn stored(rig: &Rig) -> Vec<String> {
    let Ok(entries) = fs::read_dir(rig.dir.join("state/transfers")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

/// A checkout on branch `feat`, one commit after `main`.
fn checkout(rig: &Rig) -> (PathBuf, String, String) {
    let repo = rig.dir.join("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(
        &repo,
        &["remote", "add", "origin", "git@github.com:owner/repo.git"],
    );
    fs::write(repo.join("file"), "base\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    let base = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    git(&repo, &["checkout", "-q", "-b", "feat"]);
    fs::write(repo.join("file"), "changed\n").unwrap();
    git(&repo, &["commit", "-q", "-am", "change"]);
    let head = git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    (repo.canonicalize().unwrap(), base, head)
}

fn transfer_refs(repo: &Path) -> String {
    git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/dogsled/transfer/",
        ],
    )
}

#[test]
fn files_reach_the_recipient_with_one_notice_and_either_end_can_drop() {
    let (rig, _publisher) = rig("transfer-files");
    let notes = rig.dir.join("notes.md");
    fs::write(&notes, "read this\n").unwrap();
    let (code, sent) = as_pane(
        &rig,
        "w1:p1",
        None,
        &[
            "transfer",
            "send",
            "worker",
            "--files",
            notes.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{sent}");
    assert_eq!(sent["kind"], "files");
    assert_eq!(sent["to"]["terminal"], "term_work");
    let id = sent["id"].as_str().unwrap().to_owned();
    assert_eq!(stored(&rig), std::slice::from_ref(&id));

    let inbox = rig.inbox("term_work");
    let records = inbox.as_array().unwrap();
    assert_eq!(records.len(), 1, "{inbox}");
    assert_eq!(records[0]["kind"], "transfer");
    assert_eq!(records[0]["id"], sent["notice"]);
    let text = records[0]["text"].as_str().unwrap();
    assert!(
        text.contains(&id) && text.contains("notes.md (10 bytes)"),
        "{text}"
    );
    assert!(
        text.contains(&format!("dogsled transfer show {id}")),
        "{text}"
    );

    let (_, received) = as_pane(&rig, "w1:p2", None, &["transfer", "list"]);
    assert_eq!(received.as_array().unwrap().len(), 1, "{received}");
    assert_eq!(received[0]["direction"], "received");
    assert_eq!(received[0]["from"], "lead");
    let (_, sent_list) = as_pane(&rig, "w1:p1", None, &["transfer", "list"]);
    assert_eq!(sent_list[0]["direction"], "sent");
    assert_eq!(
        as_pane(&rig, "w1:p3", None, &["transfer", "list"]).1,
        json!([])
    );

    let (code, shown) = as_pane(&rig, "w1:p2", None, &["transfer", "show", &id]);
    assert_eq!(code, 0, "{shown}");
    let path = shown["paths"][0].as_str().unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "read this\n");
    // The source file is untouched, and the stored copy is separate from it.
    assert_ne!(Path::new(path), notes);
    assert_eq!(fs::read_to_string(&notes).unwrap(), "read this\n");

    // A changed stored file is refused, not shown.
    fs::write(path, "tampered\n").unwrap();
    let (code, refused) = as_pane(&rig, "w1:p2", None, &["transfer", "show", &id]);
    assert_eq!(code, 2);
    assert!(
        refused["error"].as_str().unwrap().contains("digest"),
        "{refused}"
    );

    let (code, refused) = as_pane(&rig, "w1:p3", None, &["transfer", "drop", &id]);
    assert_eq!(code, 2);
    assert!(
        refused["error"].as_str().unwrap().contains("another agent"),
        "{refused}"
    );
    assert_eq!(stored(&rig), std::slice::from_ref(&id));
    let (code, removed) = as_pane(&rig, "w1:p2", None, &["transfer", "drop", &id]);
    assert_eq!((code, removed["removed"].clone()), (0, json!(true)));
    assert_eq!(stored(&rig), Vec::<String>::new());
    assert_eq!(
        as_pane(&rig, "w1:p2", None, &["transfer", "list"]).1,
        json!([])
    );
}

#[test]
fn refused_sends_store_nothing() {
    let (rig, _publisher) = rig("transfer-refused");
    let file = rig.dir.join("real");
    fs::write(&file, "x").unwrap();
    let link = rig.dir.join("link");
    symlink(&file, &link).unwrap();
    let inside = rig.dir.join("tree/.git");
    fs::create_dir_all(&inside).unwrap();
    fs::write(inside.join("config"), "x").unwrap();
    let large = rig.dir.join("large");
    fs::File::create(&large)
        .unwrap()
        .set_len(10 * 1024 * 1024 + 1)
        .unwrap();
    for (path, reason) in [
        (rig.dir.join("tree"), "not a regular file"),
        (link, "not a regular file"),
        (rig.dir.join("absent"), "cannot be read"),
        (inside.join("config"), "inside a Git directory"),
        (large, "larger than 10 MiB"),
    ] {
        let (code, refused) = as_pane(
            &rig,
            "w1:p1",
            None,
            &[
                "transfer",
                "send",
                "worker",
                "--files",
                path.to_str().unwrap(),
            ],
        );
        assert_eq!(code, 2, "{refused}");
        assert!(
            refused["error"].as_str().unwrap().contains(reason),
            "{refused}"
        );
    }
    // Two files with one name would overwrite each other where they are stored.
    let twin = rig.dir.join("tree/real");
    fs::write(&twin, "y").unwrap();
    let (code, refused) = as_pane(
        &rig,
        "w1:p1",
        None,
        &[
            "transfer",
            "send",
            "worker",
            "--files",
            file.to_str().unwrap(),
            twin.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 2);
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("two files are named real"),
        "{refused}"
    );
    // A recipient without an inbox reader gets no stored transfer and no notice.
    let (code, refused) = as_pane(
        &rig,
        "w1:p1",
        None,
        &[
            "transfer",
            "send",
            "other",
            "--files",
            file.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 2);
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("no dogsled inbox reader"),
        "{refused}"
    );
    assert_eq!(stored(&rig), Vec::<String>::new());
    assert_eq!(rig.inbox("term_work"), json!([]));
}

#[test]
fn changes_pin_the_source_without_touching_either_checkout() {
    let (rig, _publisher) = rig("transfer-changes");
    let (repo, base, head) = checkout(&rig);
    let status = git(&repo, &["status", "--porcelain=v2", "--branch"]);
    let (code, sent) = as_pane(
        &rig,
        "w1:p1",
        Some(&repo),
        &["transfer", "send", "worker", "--changes"],
    );
    assert_eq!(code, 0, "{sent}");
    assert_eq!(sent["kind"], "changes");
    let id = sent["id"].as_str().unwrap().to_owned();
    assert!(
        sent["summary"].as_str().unwrap().contains("1 file changed"),
        "{sent}"
    );
    assert_eq!(
        git(&repo, &["status", "--porcelain=v2", "--branch"]),
        status
    );
    assert_eq!(
        transfer_refs(&repo).trim(),
        format!("refs/dogsled/transfer/{id} {head}")
    );

    let (code, shown) = as_pane(&rig, "w1:p2", None, &["transfer", "show", &id]);
    assert_eq!(code, 0, "{shown}");
    assert_eq!(shown["since"], base);
    assert_eq!(shown["source"]["commit"], head);
    assert_eq!(shown["source"]["snapshot"], Value::Null);
    let text = rig.inbox("term_work")[0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(text.contains("Nothing was applied"), "{text}");

    // The sender drops it from its checkout: the stored transfer and the ref both go.
    let (code, removed) = as_pane(&rig, "w1:p1", Some(&repo), &["transfer", "drop", &id]);
    assert_eq!(code, 0, "{removed}");
    assert_eq!(stored(&rig), Vec::<String>::new());
    assert_eq!(transfer_refs(&repo), "");
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]).trim(), head);
}

#[test]
fn dirty_work_travels_only_on_request_and_an_empty_change_refuses() {
    let (rig, _publisher) = rig("transfer-dirty");
    let (repo, _base, head) = checkout(&rig);
    // Nothing since HEAD, and the worktree is clean: nothing to send, nothing left behind.
    let (code, refused) = as_pane(
        &rig,
        "w1:p1",
        Some(&repo),
        &["transfer", "send", "worker", "--changes", "--since", "HEAD"],
    );
    assert_eq!(code, 2);
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("nothing to send"),
        "{refused}"
    );
    assert_eq!(stored(&rig), Vec::<String>::new());
    assert_eq!(transfer_refs(&repo), "");

    fs::write(repo.join("untracked"), "new\n").unwrap();
    fs::write(repo.join("file"), "dirty\n").unwrap();
    // Without the option, dirty work stays where it is.
    let (code, refused) = as_pane(
        &rig,
        "w1:p1",
        Some(&repo),
        &["transfer", "send", "worker", "--changes", "--since", "HEAD"],
    );
    assert_eq!(code, 2, "{refused}");
    let (code, sent) = as_pane(
        &rig,
        "w1:p1",
        Some(&repo),
        &[
            "transfer",
            "send",
            "worker",
            "--changes",
            "--since",
            "HEAD",
            "--include-worktree",
        ],
    );
    assert_eq!(code, 0, "{sent}");
    let summary = sent["summary"].as_str().unwrap();
    assert!(
        summary.contains("untracked") && summary.contains("2 files changed"),
        "{summary}"
    );
    let (_, shown) = as_pane(
        &rig,
        "w1:p2",
        None,
        &["transfer", "show", sent["id"].as_str().unwrap()],
    );
    assert_eq!(shown["source"]["commit"], head);
    assert_ne!(shown["source"]["snapshot"]["commit"], head);
    // The sender's dirty files and index are as they were.
    assert_eq!(fs::read_to_string(repo.join("file")).unwrap(), "dirty\n");
    assert_eq!(
        git(&repo, &["status", "--porcelain"]),
        " M file\n?? untracked\n"
    );

    let (code, refused) = as_pane(
        &rig,
        "w1:p1",
        Some(&repo),
        &[
            "transfer",
            "send",
            "worker",
            "--changes",
            "--since",
            "no-such-revision",
        ],
    );
    assert_eq!(code, 2);
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("--since no-such-revision"),
        "{refused}"
    );
}

#[test]
fn a_large_change_keeps_the_notice_small_and_a_reused_pane_sees_nothing() {
    let (rig, _publisher) = rig("transfer-large");
    let (repo, _base, _head) = checkout(&rig);
    for index in 0..300 {
        fs::write(
            repo.join(format!("generated-file-with-a-long-name-{index:04}.txt")),
            "x\n",
        )
        .unwrap();
    }
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "many"]);
    let (code, sent) = as_pane(
        &rig,
        "w1:p1",
        Some(&repo),
        &["transfer", "send", "worker", "--changes"],
    );
    assert_eq!(code, 0, "{sent}");
    let text = rig.inbox("term_work")[0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(text.len() < 8 * 1024, "{}", text.len());
    assert!(
        text.contains("more files") && text.contains("301 files changed"),
        "{text}"
    );

    // Another terminal now has the pane: the transfer was for the old one.
    let mut panes = PANES;
    panes[1] = ("w1:p2", "term_new", "worker");
    agents(&rig, &panes);
    assert_eq!(
        as_pane(&rig, "w1:p2", None, &["transfer", "list"]).1,
        json!([])
    );
    let (code, refused) = as_pane(
        &rig,
        "w1:p2",
        None,
        &["transfer", "drop", sent["id"].as_str().unwrap()],
    );
    assert_eq!(code, 2, "{refused}");
    assert_eq!(stored(&rig).len(), 1);
}
