pub mod support;

use std::path::PathBuf;
use std::process::Command;

use self::support::SearchdTestPaths;

#[test]
fn serve_command_bootstraps_and_prints_runtime_paths() {
    let paths = match SearchdTestPaths::new() {
        Ok(paths) => paths,
        Err(error) => {
            assert!(false, "test path setup failed: {error}");
            return;
        }
    };

    let output_result = Command::new(searchd_binary_path())
        .arg("serve")
        .env("QUANTA_INDEX_STATE_ROOT", paths.state_root())
        .output();
    let output = match output_result {
        Ok(output) => output,
        Err(error) => {
            assert!(false, "spawn searchd failed: {error}");
            return;
        }
    };

    assert!(output.status.success());

    let stdout_result = String::from_utf8(output.stdout);
    let stdout = match stdout_result {
        Ok(stdout) => stdout,
        Err(error) => {
            assert!(false, "stdout decode failed: {error}");
            return;
        }
    };
    assert!(stdout.contains("quanta-index searchd scaffold"));
    assert!(stdout.contains("control_plane="));
    assert!(stdout.contains("socket_path="));
    assert!(paths.control_plane_path().exists());
}

fn searchd_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_quanta-index-searchd"))
}
