//! TOPT-03 coverage of a deep state root served through
//! its configured socket outside that root.

#![forbid(unsafe_code)]
#![cfg(unix)]

use std::fs::{self, DirBuilder, Permissions};
use std::os::unix::fs::{DirBuilderExt as _, FileTypeExt as _, PermissionsExt as _};
use std::time::Duration;

use super::{
    NativeServiceGuard, NativeUdsClient, TestResult, ViewSelection, budget, create_text_workspace,
    native_text_request, native_text_spec, publish_text_owner, read_session_service,
};

/// macOS `sockaddr_un.sun_path` capacity including the NUL.
const MACOS_SUN_PATH_LEN: usize = 104;

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "socket-layout integration assertions intentionally fail the test while setup uses Result"
)]
fn deep_state_root_serves_native_sdk_on_external_socket() -> TestResult {
    let parent = tempfile::tempdir_in(fs::canonicalize("/tmp")?)?;
    fs::set_permissions(parent.path(), Permissions::from_mode(0o700))?;
    // Keep the socket short even when TMPDIR itself exceeds the UDS limit.
    let mut state_root = parent.path().to_path_buf();
    for level in 0..10 {
        state_root.push(format!("level-{level:02}-abcdefghijklm"));
    }
    assert!(state_root.as_os_str().len() > MACOS_SUN_PATH_LEN);
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&state_root)?;
    let socket = parent.path().join("native.sock");
    assert!(socket.as_os_str().len() < MACOS_SUN_PATH_LEN);
    assert!(!socket.starts_with(&state_root));

    let running = NativeServiceGuard::start(read_session_service(
        &state_root,
        &socket,
        Duration::from_secs(30),
    )?);
    let result = (|| -> TestResult {
        let client = NativeUdsClient::new(&socket);
        let configured = create_text_workspace(&client)?;
        let (view, source) =
            publish_text_owner(&client, configured, 2, 1, "src/deep.rs", "deepneedle")?;
        assert!(fs::symlink_metadata(&socket)?.file_type().is_socket());
        let ranked = client.search_text(
            ViewSelection::Exact(view),
            native_text_spec().instance,
            native_text_request("deepneedle", 1),
            &budget(),
        )?;
        assert_eq!(ranked.view, view);
        assert_eq!(ranked.matched, 1);
        let [hit] = ranked.hits.as_slice() else {
            return Err("deep-root SDK search must return the published Source".into());
        };
        assert_eq!(hit.path, "src/deep.rs");
        assert_eq!(hit.source, source);
        Ok(())
    })();
    result.and(running.stop())
}
