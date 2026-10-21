use super::*;
use crate::shell::Fake;

#[test]
fn startup_context_preserves_error_classes_and_removes_terminal_controls() {
    let shell = Fake::new();
    shell.on(
        "herdr pane read w2:p1 --source recent-unwrapped --lines 120 --format text",
        "\u{1b}[31mnpm error ETARGET\u{1b}[0m\n",
    );
    for original in [
        Error::Deadline("herdr agent start".into()),
        Error::Io(std::io::Error::other("startup transport failed")),
        Error::Invalid("startup failed".into()),
        Error::Command {
            program: "herdr".into(),
            args: "agent start".into(),
            status: "1".into(),
            stderr: r#"{"error":{"code":"timeout","message":"startup failed"}}"#.into(),
        },
    ] {
        let class = std::mem::discriminant(&original);
        let message = original.to_string();
        let exit = original.exit_code();
        let error = startup_error(&shell, "w2:p1", original);
        assert_eq!(std::mem::discriminant(error.root()), class);
        assert_eq!(error.root().to_string(), message);
        assert_eq!(error.exit_code(), exit);
        assert!(error.to_string().contains("npm error ETARGET"));
        assert!(!error.to_string().contains('\u{1b}'));
    }
}

#[test]
fn failed_diagnostic_read_returns_the_original_error() {
    let shell = Fake::new();
    let error = startup_error(&shell, "w2:p1", Error::Deadline("native start".into()));
    assert!(matches!(error, Error::Deadline(ref operation) if operation == "native start"));
}
