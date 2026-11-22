//! Real SDK DSL History fixture, using the existing Legacy producer and Native publication rail.

use super::*;
use quanta_index_searchd_harness::artifact::GitHeadV1;
use quanta_index_searchd_harness::native_index::{
    NativeDslTiming, measure_cold_native_dsl_history, measure_native_dsl_history,
    native_dsl_history_config, native_dsl_scenario_roster,
};
use sha2::Digest as _;
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

const EXPORT_ROOT: &str = "QUANTA_INDEX_NATIVE_DSL_HISTORY_EXPORT_ROOT";
const DAEMON_BIN: &str = "QUANTA_INDEX_NATIVE_DSL_DAEMON_BIN";
const DAEMON_SHA256: &str = "QUANTA_INDEX_NATIVE_DSL_DAEMON_SHA256";

struct FixtureRoot {
    path: PathBuf,
    temporary: Option<tempfile::TempDir>,
    export: bool,
    complete: bool,
}

impl FixtureRoot {
    fn new(export: Option<PathBuf>, source_checkout: &Path) -> TestResult<Self> {
        if let Some(path) = export {
            if !path.is_absolute() {
                return Err(format!("{EXPORT_ROOT} must be absolute").into());
            }
            let parent = path
                .parent()
                .ok_or("History fixture needs a parent directory")?;
            let name = path
                .file_name()
                .ok_or("History fixture needs a directory name")?;
            let path = parent.canonicalize()?.join(name);
            if path.starts_with(source_checkout.canonicalize()?) {
                return Err(format!("{EXPORT_ROOT} must be outside the source checkout").into());
            }
            // create_dir refuses an existing output; never overwrite a retained fixture.
            std::fs::DirBuilder::new().mode(0o700).create(&path)?;
            Ok(Self {
                path,
                temporary: None,
                export: true,
                complete: false,
            })
        } else {
            let temporary = private_root()?;
            Ok(Self {
                path: temporary.path().to_path_buf(),
                temporary: Some(temporary),
                export: false,
                complete: false,
            })
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for FixtureRoot {
    fn drop(&mut self) {
        if self.export && !self.complete {
            drop(std::fs::remove_dir_all(&self.path));
        }
        // Keep the temporary root owned until all SDK measurements finish.
        let _ = &self.temporary;
    }
}

fn sha256(path: &Path) -> TestResult<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = sha2::Sha256::new();
    let _bytes = std::io::copy(&mut file, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}

fn daemon_binary(export: bool) -> TestResult<(PathBuf, String)> {
    let supplied = std::env::var_os(DAEMON_BIN).map(PathBuf::from);
    let expected = std::env::var(DAEMON_SHA256).ok();
    if export && (!supplied.as_deref().is_some_and(Path::is_absolute) || expected.is_none()) {
        return Err(format!("export requires absolute {DAEMON_BIN} and {DAEMON_SHA256}").into());
    }
    let path = std::fs::canonicalize(
        supplied.unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_quanta-index-searchd"))),
    )?;
    let digest = sha256(&path)?;
    if expected
        .as_ref()
        .is_some_and(|expected| expected != &digest)
    {
        return Err(format!("{DAEMON_SHA256} differs from the selected Native daemon").into());
    }
    Ok((path, digest))
}

fn write_private_json(path: &Path, value: &serde_json::Value) -> TestResult {
    use std::os::unix::fs::OpenOptionsExt as _;

    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    serde_json::to_writer(&mut output, value)?;
    output.sync_all()?;
    Ok(())
}

#[test]
fn native_history_dsl_fixed_ledger_warm_cold() -> TestResult {
    let export = std::env::var_os(EXPORT_ROOT).map(PathBuf::from);
    let source_checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source_head = export
        .as_ref()
        .map(|_| GitHeadV1::resolve(&source_checkout))
        .transpose()?;
    let (daemon, daemon_sha256) = daemon_binary(export.is_some())?;
    let mut root = FixtureRoot::new(export, &source_checkout)?;
    let (full, _, full_input, _) = publish_in_legacy_child_with_fixture(true)?;
    let state = root.path().join("state");
    std::fs::create_dir(&state)?;
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(history_service(&state, &socket)?);
    let published = (|| -> TestResult<(ViewRef, HistorySourceCutV1)> {
        let client = NativeUdsClient::new(&socket);
        let configured = create_text_workspace(&client)
            .map_err(|error| format!("DSL workspace setup: {error}"))?;
        let (source_view, _) =
            publish_text_owner(&client, configured, 2, 1, "live.rs", "needle current file")
                .map_err(|error| format!("DSL Source publication: {error}"))?;
        let full_cut = cut(&full, None, full_input.fence.final_replay)?;
        let request = stage_history(&client, source_view, 3, &full_input, None, full_cut.clone())?;
        let view = published(&client.submit(request, &budget())?)
            .map_err(|error| format!("DSL History FULL publication: {error}"))?;
        Ok((view, full_cut))
    })();
    let (view, cut) = match published {
        Ok(value) => value,
        Err(error) => {
            running.stop()?;
            return Err(error);
        }
    };
    running.stop()?;
    let open_key = [91_u8; 32];
    let descriptor = serde_json::json!({
        "socket": socket,
        "view": view,
        "instance": history_spec(),
        "cut": cut,
        "open_key": open_key,
    });
    let ids = native_dsl_scenario_roster();
    let history = ids["history"]
        .as_array()
        .ok_or("missing DSL History roster")?;
    if history.len() != 7 {
        return Err(format!(
            "expected seven DSL History scenarios, got {}",
            history.len()
        )
        .into());
    }
    let config = root.path().join("native.json");
    let config_value = native_dsl_history_config(&state, &socket)?;
    write_private_json(&config, &config_value)?;
    let cold_input = root.path().join(".cold-fixture.pending");
    let mut cold = serde_json::json!({
        "history": descriptor,
        "daemon": daemon,
        "daemon_sha256": daemon_sha256,
        "config": config,
        "config_sha256": sha256(&config)?,
    });
    if let Some(head) = &source_head {
        cold["source_git_head"] = serde_json::json!(head.as_str());
    }
    write_private_json(&cold_input, &cold)?;
    for id in history {
        let id = id.as_str().ok_or("non-string DSL History scenario")?;
        let _sample = measure_cold_native_dsl_history(&cold_input, id)?;
    }
    let warm_input = root.path().join(".history-warm.pending");
    write_private_json(&warm_input, &descriptor)?;
    let running = NativeServiceGuard::start(history_service(&state, &socket)?);
    let timing = NativeDslTiming {
        passes: 1,
        samples: 1,
        prime_queries: 1,
        cooldown: Duration::ZERO,
        pass_settle: Duration::ZERO,
    };
    let warm = (|| -> TestResult {
        for id in history {
            let id = id.as_str().ok_or("non-string DSL History scenario")?;
            let _sample = measure_native_dsl_history(&warm_input, id, timing)?;
        }
        Ok(())
    })();
    let stopped = running.stop();
    warm?;
    stopped?;
    if sha256(&daemon)? != daemon_sha256 {
        return Err("selected Native daemon changed while preparing the History fixture".into());
    }
    if let Some(head) = &source_head
        && GitHeadV1::resolve(&source_checkout)? != *head
    {
        return Err("source checkout changed while preparing the History fixture".into());
    }
    std::fs::rename(warm_input, root.path().join("history-warm.json"))?;
    std::fs::rename(cold_input, root.path().join("cold-fixture.json"))?;
    std::fs::File::open(root.path())?.sync_all()?;
    root.complete = true;
    Ok(())
}
