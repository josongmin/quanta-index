//! Common process fixture for pinned scale, trace and restore tests.

use std::error::Error;
use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

use quanta_index_searchd_harness::legacy_reference::{
    LegacySearchdBinary, LegacySearchdCommand, SearchdBinaryProcess,
};
use quanta_index_searchd_harness::scale_runtime_policy::{
    ScaleRuntimeConfig, daemon_scale_capacity_environment,
};
use quanta_index_searchd_harness::source_fixture::ScaleTier;

pub(super) const SUPPORTED_PROFILE_TIMEOUT: Duration = Duration::from_secs(600);

pub(super) fn start_process(
    state: &Path,
    tier: ScaleTier,
    binary: &LegacySearchdBinary,
) -> Result<SearchdBinaryProcess, Box<dyn Error>> {
    let mut command = scale_process_command(state, binary)?;
    // All declared scale tiers use the same existing history profile.
    // Medium retains its original client and readiness deadlines.
    let _configured = command.envs(ScaleRuntimeConfig::default().daemon_history_environment()?);
    let process = if tier == ScaleTier::Medium {
        SearchdBinaryProcess::start_with_pinned_command(state, command)?
    } else {
        let _configured = command.envs(daemon_scale_capacity_environment());
        SearchdBinaryProcess::start_with_pinned_command_and_timeout(
            state,
            command,
            SUPPORTED_PROFILE_TIMEOUT,
        )?
    };
    binary.verify()?;
    Ok(process)
}

pub(super) fn scale_process_binary() -> Result<LegacySearchdBinary, Box<dyn Error>> {
    let binary = std::env::var_os("QUANTA_INDEX_SCALE_RESTART_SEARCHD_BINARY");
    let digest = std::env::var_os("QUANTA_INDEX_SCALE_RESTART_SEARCHD_SHA256");
    checked_process_binary(binary.as_deref(), digest.as_deref())?
        .map_or_else(LegacySearchdBinary::from_environment, Ok)
}

pub(super) fn scale_process_command<'binary>(
    state: &Path,
    binary: &'binary LegacySearchdBinary,
) -> Result<LegacySearchdCommand<'binary>, Box<dyn Error>> {
    let mut command = binary.command()?;
    let _configured = command.arg("serve").arg("--state-root").arg(state);
    let _configured = command.apply_searchd_env(2);
    Ok(command)
}

pub(super) fn checked_process_binary(
    binary: Option<&OsStr>,
    digest: Option<&OsStr>,
) -> Result<Option<LegacySearchdBinary>, Box<dyn Error>> {
    let (binary, digest) = match (binary, digest) {
        (None, None) => return Ok(None),
        (Some(binary), Some(digest)) => (Path::new(binary), digest.to_str()),
        _ => return Err("scale restart binary requires both path and SHA256".into()),
    };
    let digest = digest.ok_or("scale restart binary SHA256 is not UTF-8")?;
    Ok(Some(LegacySearchdBinary::from_path_and_sha(
        binary, digest,
    )?))
}
