//! Main checkout resolution is shared by the CLI, host transport, leads, and MCP.

use super::lead::{checkout, clone_source, peer_transport, remote_publication};
use super::*;

const SLUG: &str = "owner/repo";
const ORIGIN: &str = "git@github.com:owner/repo.git";

fn command(rig: &Rig, args: &[&str]) -> (i32, Value) {
    let mut process = rig.start(args);
    let status = process.wait();
    let (stdout, stderr) = process.output();
    let value = serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("{error}: stdout={stdout}, stderr={stderr}"));
    (status.code().unwrap(), value)
}

fn open(rig: &Rig) -> (i32, Value) {
    command(rig, &["--json", "repo", "open", SLUG])
}

fn git(repo: &Path, args: &[&str]) -> String {
    let mut cmd = isolated("git");
    cmd.arg("-C").arg(repo).args(args);
    let mut process = Rig::background(cmd);
    let status = process.wait();
    let (stdout, stderr) = process.output();
    assert!(status.success(), "{stderr}");
    stdout.trim().to_owned()
}

fn clones(rig: &Rig) -> Vec<String> {
    rig.log("git.log")
        .lines()
        .filter(|line| line.starts_with("clone "))
        .map(str::to_owned)
        .collect()
}

#[test]
fn reuses_https_checkout_without_changing_branch_head_index_or_dirty_bytes() {
    let rig = Rig::short("repo-open-reuse");
    clone_source(&rig);
    let repo = checkout(&rig, "elsewhere", "https://github.com/owner/repo.git");
    git(&repo, &["switch", "-q", "-c", "in-progress"]);
    fs::write(repo.join("work"), b"staged\n").unwrap();
    git(&repo, &["add", "work"]);
    fs::write(repo.join("work"), b"dirty\0\xff\n").unwrap();
    let paths = [
        ".git/HEAD",
        ".git/refs/heads/in-progress",
        ".git/index",
        "work",
    ];
    let before: Vec<_> = paths
        .iter()
        .map(|path| fs::read(repo.join(path)).unwrap())
        .collect();
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let (code, opened) = open(&rig);
    assert_eq!(code, 0, "{opened}");
    assert_eq!(opened["repository"], "github.com/owner/repo");
    assert_eq!(opened["host"], "local");
    assert_eq!(opened["path"], json!(repo));
    assert_eq!(opened["branch"], "in-progress");
    assert_eq!(opened["head"], head);
    assert!(opened["dirty"].as_u64().unwrap() >= 1, "{opened}");
    assert_eq!(opened["cloned"], false);
    for (path, bytes) in paths.iter().zip(before) {
        assert_eq!(fs::read(repo.join(path)).unwrap(), bytes, "{path}");
    }
    assert_eq!(clones(&rig), Vec::<String>::new());
    assert_eq!(rig.log("herdr.log"), "");
    assert_eq!(rig.log("ssh.log"), "");
    assert_eq!(rig.items(), json!([]));
    assert_eq!(rig.tickets(), json!([]));
}

#[test]
fn clones_once_and_uses_owner_prefix_for_a_name_collision() {
    for collision in [false, true] {
        let rig = Rig::short("repo-open-clone");
        clone_source(&rig);
        let other = collision.then(|| checkout(&rig, "repo", "git@github.com:other/repo.git"));
        let destination = rig.dir.join(if collision {
            "home/dev/owner-repo"
        } else {
            "home/dev/repo"
        });
        let (code, opened) = open(&rig);
        assert_eq!(code, 0, "{opened}");
        assert_eq!(opened["path"], json!(destination.canonicalize().unwrap()));
        assert_eq!(opened["cloned"], true);
        assert_eq!(opened["branch"], "main");
        assert_eq!(opened["dirty"], 0);
        assert_eq!(opened["head"], git(&destination, &["rev-parse", "HEAD"]));
        let (code, reused) = open(&rig);
        assert_eq!(code, 0, "{reused}");
        let mut expected = opened;
        expected["cloned"] = json!(false);
        assert_eq!(reused, expected);
        assert_eq!(
            clones(&rig),
            [format!("clone {ORIGIN} {}", destination.display())]
        );
        if let Some(other) = other {
            assert_eq!(
                git(&other, &["remote", "get-url", "origin"]),
                "git@github.com:other/repo.git"
            );
        }
        assert_eq!(rig.log("herdr.log"), "");
    }
}

#[test]
fn refuses_two_occupied_destinations_without_changing_them() {
    let rig = Rig::short("repo-open-occupied");
    clone_source(&rig);
    let other = checkout(&rig, "repo", "git@github.com:other/repo.git");
    let occupied = rig.dir.join("home/dev/owner-repo");
    fs::write(&occupied, b"leave me alone\0\xff").unwrap();
    let before = fs::read(other.join(".git/HEAD")).unwrap();
    let (code, error) = open(&rig);
    assert_eq!(code, 2, "{error}");
    assert!(error.to_string().contains("occupied"), "{error}");
    assert_eq!(fs::read(other.join(".git/HEAD")).unwrap(), before);
    assert_eq!(fs::read(occupied).unwrap(), b"leave me alone\0\xff");
    assert_eq!(clones(&rig), Vec::<String>::new());
    assert_eq!(rig.log("herdr.log"), "");
    assert_eq!(rig.log("ssh.log"), "");
}

#[test]
fn explicit_host_sends_one_repo_open_and_never_clones_locally() {
    let rig = Rig::short("repo-open-remote");
    clone_source(&rig);
    let _observer = remote_publication(&rig);
    peer_transport(&rig, false);
    let (code, snapshot) = rig.json(&["host", "snapshot"]);
    assert_eq!((code, &snapshot["repo_open"]), (0, &json!(1)), "{snapshot}");
    rig.put("peer-snapshot.json", &snapshot);
    let herdr_before = rig.log("herdr.log");
    let (code, opened) = command(&rig, &["--json", "repo", "open", SLUG, "--host", "netcup"]);
    assert_eq!(code, 0, "{opened}");
    assert_eq!(opened["repository"], "github.com/owner/repo");
    assert_eq!(opened["cloned"], true);
    assert_eq!(opened["host"], "netcup");
    let destination = rig.dir.join("peer-home/dev/repo");
    assert_eq!(opened["path"], json!(destination.canonicalize().unwrap()));
    assert!(!rig.dir.join("home/dev/repo").exists());
    assert_eq!(
        clones(&rig),
        [format!("clone {ORIGIN} {}", destination.display())]
    );
    let ssh = rig.log("ssh.log");
    let calls: Vec<_> = ssh.lines().collect();
    // One capability read, then one open.
    assert_eq!(calls.len(), 2, "{ssh}");
    assert!(
        calls[0].ends_with("peer-host host snapshot --json"),
        "{ssh}"
    );
    assert!(
        calls[1].ends_with("peer-host host repo-open --repo owner/repo --json"),
        "{ssh}"
    );
    assert_eq!(rig.log("herdr.log"), herdr_before);
}

#[test]
fn a_peer_without_repo_open_is_refused_before_any_effect() {
    let rig = Rig::short("repo-open-old-peer");
    clone_source(&rig);
    let _observer = remote_publication(&rig);
    peer_transport(&rig, false);
    let (_, mut snapshot) = rig.json(&["host", "snapshot"]);
    snapshot.as_object_mut().unwrap().remove("repo_open");
    rig.put("peer-snapshot.json", &snapshot);
    let (code, refused) = command(&rig, &["--json", "repo", "open", SLUG, "--host", "netcup"]);
    assert_eq!(code, 2, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("netcup runs a dogsled without `repo open`"),
        "{refused}"
    );
    assert!(
        !rig.log("ssh.log").contains("repo-open"),
        "{}",
        rig.log("ssh.log")
    );
    assert_eq!(clones(&rig), Vec::<String>::new());
    assert!(!rig.dir.join("peer-home/dev/repo").exists());
}

#[test]
fn invalid_slugs_exit_two_before_git_ssh_or_herdr() {
    let rig = Rig::short("repo-open-invalid");
    clone_source(&rig);
    for slug in [
        "",
        "repo",
        "/repo",
        "owner/",
        "owner/repo/extra",
        "../repo",
        "owner/..",
        "https://github.com/owner/repo",
        "owner/repo;true",
        "owner/repo\n",
    ] {
        for prefix in [
            vec!["--json", "repo", "open", slug, "--host", "netcup"],
            vec!["--json", "repo", "open", slug],
            vec!["--json", "host", "repo-open", "--repo", slug],
        ] {
            let (code, error) = command(&rig, &prefix);
            assert_eq!(code, 2, "{slug:?}: {error}");
            assert!(error.to_string().contains("owner/name"), "{error}");
        }
    }
    assert_eq!(rig.log("git.log"), "");
    assert_eq!(rig.log("ssh.log"), "");
    assert_eq!(rig.log("herdr.log"), "");
}

#[test]
fn mcp_returns_the_same_object_and_rejects_unknown_fields() {
    let rig = Rig::short("repo-open-mcp");
    checkout(&rig, "elsewhere", ORIGIN);
    let mut client = super::mcp::Client::start(&rig);
    let (refused, error) = client.call("repo_open", &json!({"repo":SLUG,"extra":true}));
    assert!(error, "{refused}");
    let (opened, error) = client.call("repo_open", &json!({"repo":SLUG}));
    assert!(!error, "{opened}");
    let (code, cli) = open(&rig);
    assert_eq!(code, 0, "{cli}");
    assert_eq!(opened, cli);
    assert_eq!(rig.log("herdr.log"), "");
    assert_eq!(rig.log("ssh.log"), "");
}

#[test]
fn detached_and_unborn_checkouts_keep_null_metadata() {
    let rig = Rig::short("repo-open-head");
    let repo = checkout(&rig, "repo", ORIGIN);
    git(&repo, &["checkout", "--detach", "-q"]);
    let (code, detached) = open(&rig);
    assert_eq!(code, 0, "{detached}");
    assert_eq!(detached["branch"], Value::Null);
    assert_eq!(detached["head"], git(&repo, &["rev-parse", "HEAD"]));
    git(&repo, &["switch", "--orphan", "unborn", "-q"]);
    let (code, unborn) = open(&rig);
    assert_eq!(code, 0, "{unborn}");
    assert_eq!(unborn["branch"], "unborn");
    assert_eq!(unborn["head"], Value::Null);
    assert_eq!(unborn["cloned"], false);
}
