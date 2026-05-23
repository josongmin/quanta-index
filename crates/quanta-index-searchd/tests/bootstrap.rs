pub mod support;

use quanta_index_searchd::{app::SearchdConfig, runtime::SearchRuntime};

use self::support::SearchdTestPaths;

#[test]
fn runtime_bootstrap_creates_control_plane_database() {
    let paths = match SearchdTestPaths::new() {
        Ok(paths) => paths,
        Err(error) => {
            assert!(false, "test path setup failed: {error}");
            return;
        }
    };
    let config = SearchdConfig::from_state_root(paths.state_root().to_path_buf());

    let runtime_result = SearchRuntime::bootstrap(config);
    let runtime = match runtime_result {
        Ok(runtime) => runtime,
        Err(error) => {
            assert!(false, "bootstrap failed: {error}");
            return;
        }
    };

    assert_eq!(
        runtime.config().control_plane_path,
        paths.control_plane_path()
    );
    assert!(paths.control_plane_path().exists());
}
