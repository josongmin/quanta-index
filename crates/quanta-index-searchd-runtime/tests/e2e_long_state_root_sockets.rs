//! TOPT-03 proof: a deliberately long state-root path binds all three sockets.
//!
//! macOS `sockaddr_un` fits 104 bytes: a daemon that joined its socket
//! paths onto a deep state root could not bind. The harness keeps its
//! sockets out of the root, so a root deeper than the limit still
//! serves — this test pins that mechanism by booting over a root past
//! the limit and proving every socket bound, listening, and short.

#![forbid(unsafe_code)]
#![cfg(unix)]

use std::error::Error;
use std::os::unix::net::UnixStream;

use quanta_index_searchd_harness::E2eRuntime;

/// macOS `sockaddr_un.sun_path` capacity including the NUL.
const MACOS_SUN_PATH_LEN: usize = 104;

fn is_bound_socket(path: &std::path::Path) -> Result<bool, Box<dyn Error>> {
    use std::os::unix::fs::FileTypeExt as _;
    Ok(std::fs::symlink_metadata(path)?.file_type().is_socket()
        && UnixStream::connect(path).is_ok())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "socket-layout integration assertions intentionally fail the test while setup uses Result"
)]
fn deliberately_long_state_root_binds_all_three_sockets() -> Result<(), Box<dyn Error>> {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    // Ten levels of 23 characters push the root past the socket path
    // limit on any host tempdir.
    let mut deep = parent.path().to_path_buf();
    for level in 0..10 {
        deep.push(format!("level-{level:02}-abcdefghijklm"));
    }
    assert!(
        deep.as_os_str().len() > MACOS_SUN_PATH_LEN,
        "the proof root must exceed the socket path limit: {}",
        deep.display()
    );
    let mut runtime = E2eRuntime::boot_in(&deep)?;
    runtime.start()?;
    assert_eq!(
        runtime.state_root(),
        deep.as_path(),
        "the daemon serves the deep root"
    );
    let (query, control, ingest) = runtime
        .socket_paths()
        .ok_or("a started daemon binds all three sockets")?;
    for socket in <[_; 3]>::from((query, control, ingest)) {
        assert!(
            socket.as_os_str().len() < MACOS_SUN_PATH_LEN,
            "harness sockets stay bindable while the root is not: {}",
            socket.display()
        );
        assert!(
            !socket.starts_with(&deep),
            "harness sockets live outside the deep root: {}",
            socket.display()
        );
        assert!(
            is_bound_socket(socket)?,
            "the deep-root daemon bound and listens on {}",
            socket.display()
        );
    }
    runtime.stop()?;
    Ok(())
}
