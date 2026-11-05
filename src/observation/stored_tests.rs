use super::{Availability, StoredOnly};
use crate::shell::Shell as _;

#[test]
fn fixture_shell_refuses_nonterminal_reads_and_mutations_with_explicit_gaps() {
    let shell = StoredOnly::default();
    for (program, args) in [
        ("git", vec!["status"]),
        ("ssh", vec!["peer"]),
        ("df", vec!["-Pk", "/"]),
        ("herdr", vec!["agent", "start"]),
        ("herdr", vec!["machine", "list"]),
        ("herdr", vec!["pane", "close", "--pane", "w1:p1"]),
    ] {
        let error = shell
            .run(
                program,
                &args.into_iter().map(str::to_owned).collect::<Vec<_>>(),
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("external observation unavailable")
        );
    }
    assert!(shell.read_file("/proc/stat").is_err());
    let gaps = shell.gaps();
    assert!(
        gaps.iter()
            .all(|gap| gap.availability == Availability::Unavailable)
    );
    assert_eq!(gaps.len(), 5);
}
