use super::*;

fn capture(rig: &Rig, command: &mut Command) -> Output {
    let stdout = rig.dir.join("git-env-stdout");
    let stderr = rig.dir.join("git-env-stderr");
    command
        .stdout(fs::File::create(&stdout).unwrap())
        .stderr(fs::File::create(&stderr).unwrap());
    let status = Owned::spawn(command).wait();
    Output {
        status,
        stdout: fs::read(stdout).unwrap(),
        stderr: fs::read(stderr).unwrap(),
    }
}

fn git(rig: &Rig, path: &Path, args: &[&str]) -> String {
    let output = capture(
        rig,
        isolated("git")
            .arg("-C")
            .arg(path)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args(args),
    );
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn hook_environment_cannot_redirect_worktree_observation() {
    let rig = Rig::new("git-env-observation");
    let repo = rig.dir.join("home/repo");
    let hook = rig.dir.join("home/hook");
    let tree = repo.join("tree");
    for path in [&repo, &hook] {
        fs::create_dir_all(path).unwrap();
        git(&rig, path, &["init", "-q", "-b", "main"]);
        let file = if path == &hook { "hook" } else { "file" };
        fs::write(path.join(file), "base\n").unwrap();
        git(&rig, path, &["add", "."]);
        git(&rig, path, &["commit", "-qm", "base"]);
    }
    let base = git(&rig, &repo, &["rev-parse", "HEAD"]);
    git(
        &rig,
        &repo,
        &["worktree", "add", "-qb", "feature", tree.to_str().unwrap()],
    );
    fs::write(tree.join("feature"), "feature\n").unwrap();
    git(&rig, &tree, &["add", "."]);
    git(&rig, &tree, &["commit", "-qm", "feature"]);
    let head = git(&rig, &tree, &["rev-parse", "HEAD"]);
    rig.put(
        "agents.json",
        &json!({"result":{"agents":[{
            "pane_id":"w1:p1", "terminal_id":"term_owner", "agent":"pi",
            "agent_status":"working", "cwd":tree
        }]}}),
    );
    // Herdr itself runs Git too: System must isolate every child, not only `git`.
    rig.script("herdr", &HERDR_SCRIPT.replace(
        "case \"$*\" in",
        "if test \"$1 $2\" = 'agent list'; then\n  git -C \"$RIG/home/repo/tree\" rev-parse HEAD > \"$RIG/herdr-head\"\nfi\ncase \"$*\" in",
    ));
    for (dir, index) in [(true, false), (false, true), (true, true)] {
        for dirty in [false, true] {
            fs::write(
                tree.join("feature"),
                if dirty { "dirty\n" } else { "feature\n" },
            )
            .unwrap();
            let mut command = rig.cmd(&["serve", "--once"]);
            if dir {
                command.env("GIT_DIR", hook.join(".git"));
                command.env("GIT_WORK_TREE", &hook);
            }
            if index {
                command.env("GIT_INDEX_FILE", hook.join(".git/index"));
            }
            let output = capture(&rig, &mut command);
            assert!(output.status.success(), "{output:?}");
            let host = rig.host();
            let facts = &host["probe"]["agent_panes"][0]["worktree"];
            assert_eq!(facts["path"], json!(tree), "{host}");
            assert_eq!(facts["branch"], "feature");
            assert_eq!(facts["ahead"], 1);
            assert_eq!(facts["dirty"], dirty);
            assert_eq!(facts["landed"], false);
            assert_eq!(facts["head"], head);
            assert_eq!(facts["base"], base);
            assert_eq!(
                fs::read_to_string(rig.dir.join("herdr-head"))
                    .unwrap()
                    .trim(),
                head
            );
        }
    }
}
