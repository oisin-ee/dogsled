use super::close::git;
use super::*;

fn rig(name: &str) -> (Rig, PathBuf) {
    let rig = Rig::new(name);
    let repo = rig.dir.join("home/repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "base"]);
    git(&repo, &["checkout", "-q", "-b", "feat/x"]);
    fs::write(repo.join("work"), "discarded work\n").unwrap();
    git(&repo, &["add", "work"]);
    git(&repo, &["commit", "-q", "-m", "unmerged work"]);
    git(&repo, &["checkout", "-q", "main"]);
    let origin = rig.dir.join("origin.git");
    git(&repo, &["init", "-q", "--bare", origin.to_str().unwrap()]);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "origin", "main", "feat/x"]);
    git(&repo, &["remote", "set-head", "origin", "main"]);
    rig.put(
        "state/items.json",
        &json!([{
            "id":"i1","kind":"agent","host":"local","pane":"w2:p1",
            "owner_pane":"w1:p1","name":"child","created_ms":1,"closed_ms":2,
            "branch":{"repo":repo,"name":"feat/x","ahead":1,"landed":false}
        }]),
    );
    rig.snapshot("state/host.json", now(), 0.0, 8);
    (rig, repo)
}

fn mark(rig: &Rig) {
    let item = rig.read("state/items.json")[0].clone();
    rig.put(
        "state/done.json",
        &json!({"term_child":{
            "pane":"w2:p1","item":"i1","marked_ms":1,"closed_ms":2,
            "branch":item["branch"]
        }}),
    );
}

fn rows(rig: &Rig) -> Vec<Value> {
    let (code, board) = rig.json(&["board", "--all"]);
    assert_eq!(code, 0, "{board}");
    board["rows"].as_array().unwrap().clone()
}

#[test]
fn local_drop_deletes_local_and_origin_branches_and_settles_mark_and_board() {
    let (rig, repo) = rig("drop-local");
    mark(&rig);
    assert_eq!(rows(&rig).len(), 1);
    let out = rig.run(&["drop", "i1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "dropped feat/x (1 unmerged commits discarded)\n"
    );
    assert_eq!(git(&repo, &["branch", "--list", "feat/x"]), "");
    assert_eq!(
        git(&repo, &["ls-remote", "origin", "refs/heads/feat/x"]),
        ""
    );
    assert_eq!(rows(&rig), [] as [Value; 0]);
    assert!(rig.read("state/items.json")[0]["dropped_ms"].is_u64());
    assert!(rig.read("state/done.json")["term_child"]["dropped_ms"].is_u64());
    assert_eq!(rig.json(&["drop", "i1"]).0, 0, "repeated drop is harmless");
}

#[test]
fn drop_accepts_already_absent_branches() {
    let (rig, repo) = rig("drop-absent");
    git(&repo, &["branch", "-D", "feat/x"]);
    git(&repo, &["push", "origin", "--delete", "feat/x"]);
    assert_eq!(rig.json(&["drop", "i1"]).0, 0);
    assert_eq!(rows(&rig), [] as [Value; 0]);
}

#[test]
fn drop_refuses_checked_out_branch_without_deleting_origin() {
    let (rig, repo) = rig("drop-checked-out");
    git(&repo, &["worktree", "add", "-q", ".worktrees/x", "feat/x"]);
    let (code, refused) = rig.json(&["drop", "i1"]);
    assert_eq!(code, 2, "{refused}");
    assert!(refused["error"].as_str().unwrap().contains("checked out"));
    assert_ne!(
        git(&repo, &["ls-remote", "origin", "refs/heads/feat/x"]),
        ""
    );
    assert!(rig.read("state/items.json")[0]["dropped_ms"].is_null());
}

#[test]
fn drop_refuses_open_branchless_and_foreign_owned_items() {
    for (name, field, value, message) in [
        ("open", "closed_ms", json!(null), "closed agent"),
        ("branchless", "branch", json!(null), "no retained branch"),
        ("foreign", "owner_pane", json!("w3:p1"), "only owner"),
        ("job", "kind", json!("job"), "closed agent"),
    ] {
        let (rig, repo) = rig(&format!("drop-{name}"));
        let mut items = rig.read("state/items.json");
        items[0][field] = value;
        rig.put("state/items.json", &items);
        let (code, refused) = rig.json(&["drop", "i1"]);
        assert_eq!(code, 2, "{refused}");
        assert!(
            refused["error"].as_str().unwrap().contains(message),
            "{refused}"
        );
        assert_ne!(git(&repo, &["branch", "--list", "feat/x"]), "");
        assert_ne!(
            git(&repo, &["ls-remote", "origin", "refs/heads/feat/x"]),
            ""
        );
    }
}

#[test]
fn missing_mark_and_missing_branch_leave_board_but_existing_branch_stays() {
    for gone in [true, false] {
        let (rig, repo) = rig(&format!("drop-missing-mark-{gone}"));
        if gone {
            git(&repo, &["branch", "-D", "feat/x"]);
            git(&repo, &["push", "origin", "--delete", "feat/x"]);
        }
        assert_eq!(rows(&rig).is_empty(), gone);
        let item = &rig.read("state/items.json")[0];
        assert_eq!(item["branch"]["landed"], gone);
        assert!(item["branch_checked_ms"].is_u64());
    }
}

fn remote(rig: &Rig) {
    rig.peer();
    let mut items = rig.read("state/items.json");
    items[0]["host"] = json!("8103");
    rig.put("state/items.json", &items);
    // Exercise host subcommands in a separate ledger, through the real SSH dispatch path.
    fs::create_dir_all(rig.dir.join("remote-state")).unwrap();
    rig.script(
        "ssh",
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RIG/ssh.log"
case "$*" in
  *'host snapshot'*) printf '{"running":true,'; tr -d '\n' < "$RIG/remote.json" | cut -c2- ;;
  *) for last do :; done
     export DOGSLED_STATE="$RIG/remote-state"
     eval "$last" ;;
esac
"#,
    );
    // remote_command is the test binary, not whichever dogsled happens to be on PATH.
    rig.config(&format!(
        "remote_command = [{}]\n",
        serde_json::to_string(env!("CARGO_BIN_EXE_dogsled")).unwrap()
    ));
}

#[test]
fn remote_drop_uses_ssh_and_settles_origin_item() {
    let (rig, repo) = rig("drop-remote");
    remote(&rig);
    let (code, dropped) = rig.json(&["drop", "i1", "--host", "netcup"]);
    assert_eq!(code, 0, "{dropped}");
    assert_eq!(dropped["discarded"], 1);
    assert!(rig.log("ssh.log").contains("host drop --item"));
    assert_eq!(git(&repo, &["branch", "--list", "feat/x"]), "");
    assert_eq!(
        git(&repo, &["ls-remote", "origin", "refs/heads/feat/x"]),
        ""
    );
    assert_eq!(rows(&rig), [] as [Value; 0]);
}

#[test]
fn missing_mark_checks_batch_once_per_host_per_hour() {
    let (rig, _) = rig("drop-hourly");
    remote(&rig);
    let mut items = rig.read("state/items.json");
    let mut second = items[0].clone();
    second["id"] = json!("i2");
    second["pane"] = json!("w2:p2");
    items.as_array_mut().unwrap().push(second);
    rig.put("state/items.json", &items);
    assert_eq!(rows(&rig).len(), 2);
    assert_eq!(
        rig.log("ssh.log").matches("host refresh-branches").count(),
        1
    );
    assert!(rig.read("state/items.json")[1]["branch_checked_ms"].is_u64());
    assert_eq!(rows(&rig).len(), 2);
    assert_eq!(
        rig.log("ssh.log").matches("host refresh-branches").count(),
        1
    );
    let mut items = rig.read("state/items.json");
    for item in items.as_array_mut().unwrap() {
        item["branch_checked_ms"] = json!(now() - 3_600_001);
    }
    rig.put("state/items.json", &items);
    assert_eq!(rows(&rig).len(), 2);
    assert_eq!(
        rig.log("ssh.log").matches("host refresh-branches").count(),
        2
    );
}

#[test]
fn concurrent_status_reads_share_the_hourly_check() {
    let (rig, _) = rig("drop-concurrent");
    remote(&rig);
    let mut first = rig.start(&["board", "--all", "--json"]);
    let mut second = rig.start(&["board", "--all", "--json"]);
    assert!(first.wait().success());
    assert!(second.wait().success());
    assert_eq!(
        rig.log("ssh.log").matches("host refresh-branches").count(),
        1
    );
}

#[test]
fn failed_branch_check_keeps_item_unchanged_and_only_logs() {
    let (rig, _) = rig("drop-refresh-failure");
    remote(&rig);
    let before = rig.read("state/items.json");
    rig.script(
        "ssh",
        r#"#!/bin/sh
case "$*" in
  *'host snapshot'*) printf '{"running":true,'; tr -d '\n' < "$RIG/remote.json" | cut -c2- ;;
  *) echo branch-check-failed >&2; exit 1 ;;
esac
"#,
    );
    let out = rig.run(&["board", "--all", "--json"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("dogsled branches: netcup:"));
    assert_eq!(rig.read("state/items.json"), before);
}

#[test]
fn dropped_or_landed_item_is_hidden_even_with_a_stale_mark() {
    for dropped in [true, false] {
        let (rig, _) = rig(&format!("drop-hidden-{dropped}"));
        let mut items = rig.read("state/items.json");
        if dropped {
            items[0]["dropped_ms"] = json!(3);
        } else {
            items[0]["branch"]["landed"] = json!(true);
        }
        rig.put("state/items.json", &items);
        mark(&rig);
        assert_eq!(rows(&rig), [] as [Value; 0]);
    }
}

#[test]
fn stale_host_or_existing_mark_does_not_trigger_missing_mark_check() {
    let (rig, _) = rig("drop-no-check");
    rig.snapshot("state/host.json", now() - 31_000, 0.0, 8);
    assert_eq!(rows(&rig).len(), 1);
    assert!(rig.read("state/items.json")[0]["branch_checked_ms"].is_null());
    rig.snapshot("state/host.json", now(), 0.0, 8);
    mark(&rig);
    assert_eq!(rows(&rig).len(), 1);
    assert!(rig.read("state/items.json")[0]["branch_checked_ms"].is_null());
}
