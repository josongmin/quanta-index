use super::super::*;

fn workspace_hex() -> String {
    "01".repeat(64)
}

fn run_native(args: Vec<String>) -> (ExitCode, String) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = run(args, &mut stdout, &mut stderr);
    let message = String::from_utf8(stderr).expect("CLI errors are UTF-8");
    (exit, message)
}

#[test]
fn native_read_view_requires_an_explicit_socket() {
    let (exit, stderr) = run_native(vec![
        "native".into(),
        "read-view".into(),
        "--workspace".into(),
        workspace_hex(),
        "--epoch".into(),
        "1".into(),
    ]);
    assert_eq!(exit, ExitCode::from(EXIT_USAGE));
    assert!(stderr.contains("--socket"), "{stderr}");
}

#[test]
fn native_read_view_rejects_a_truncated_workspace_identity() {
    let (exit, stderr) = run_native(vec![
        "native".into(),
        "read-view".into(),
        "--socket".into(),
        "/tmp/absent-native.sock".into(),
        "--workspace".into(),
        "01".repeat(63),
        "--epoch".into(),
        "1".into(),
    ]);
    assert_eq!(exit, ExitCode::from(EXIT_USAGE));
    assert!(stderr.contains("workspace"), "{stderr}");
}

#[test]
fn native_text_request_uses_the_explicit_native_socket() {
    let directory = tempfile::tempdir().expect("private missing socket fixture");
    let socket = directory.path().join("absent-native.sock");
    let (exit, stderr) = run_native(vec![
        "native".into(),
        "search-text".into(),
        "--socket".into(),
        socket.to_string_lossy().into_owned(),
        "--workspace".into(),
        workspace_hex(),
        "--epoch".into(),
        "1".into(),
        "--instance".into(),
        "02".repeat(16),
        "--query-text".into(),
        "needle".into(),
        "--top-k".into(),
        "2".into(),
    ]);
    assert_eq!(exit, ExitCode::from(EXIT_TRANSPORT));
    assert!(stderr.contains("native"), "{stderr}");
    assert!(
        stderr.contains(&socket.to_string_lossy().to_string()),
        "{stderr}"
    );
}
