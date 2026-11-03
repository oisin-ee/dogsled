use super::*;

#[test]
fn malformed_host_values_are_refused_by_typed_put_without_storing_anything() {
    let rig = Rig::new("malformed-host-values");
    for command in [
        "host",
        "done",
        "watchdog",
        "worktrees",
        "repositories",
        "parents",
        "custody",
        "landing",
        "orphans",
        "preservation-operations",
        "sweep",
        "quota-snapshot",
        "quota-refresh",
    ] {
        let before = rig.state(&[command]);
        let mut child = rig
            .cmd(&["state", command, "--put", "--json"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(&mut child.stdin.take().unwrap(), b"false").unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            !output.status.success(),
            "{command} accepted malformed input: {output:?}"
        );
        assert_eq!(
            rig.state(&[command]),
            before,
            "{command} stored malformed input"
        );
    }
}
