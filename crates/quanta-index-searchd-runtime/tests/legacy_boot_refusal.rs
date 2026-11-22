//! Pinned old-Q boot refusals for roots that require an offline rebuild.

#![forbid(unsafe_code)]

use std::error::Error;
use std::fs;
use std::path::Path;

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::PROCESS_MEMORY_ENVELOPE_EXCEEDED_CODE;
use quanta_index_searchd::app::SearchdConfig;
use quanta_index_searchd::app::state_format::{
    LEGACY_AUXILIARY_SNAPSHOT_RELATIVES, inventory_state_root_v1,
};
use quanta_index_searchd_harness::legacy_reference::LegacySearchdBinary;
use quanta_index_searchd_harness::private_tempdir as private_root;

use crate::{legacy_start_probe, offline_legacy_root::build_legacy_root};

type TestResult = Result<(), Box<dyn Error>>;

fn assert_refused_before_root_mutation(root: &Path) -> TestResult {
    let before = inventory_state_root_v1(root, &[])?;
    let binary = LegacySearchdBinary::from_environment()?;
    let output = legacy_start_probe::require_start_failure(&binary, root, 8)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() != Some(70)
        || !stderr.contains(SearchPlaneErrorCodeV2::StateRootFormatUnsupported.as_wire_str())
    {
        return Err(format!(
            "old-Q boot did not refuse the root with STATE_ROOT_FORMAT_UNSUPPORTED: exit={:?}, stderr={stderr}",
            output.status.code()
        )
        .into());
    }
    if inventory_state_root_v1(root, &[])? != before {
        return Err("refused old-Q boot changed the state-root inventory".into());
    }
    Ok(())
}

#[test]
fn production_boot_refuses_a_legacy_root_instead_of_migrating() -> TestResult {
    let root = private_root()?;
    build_legacy_root(root.path())?;
    // The old boot migration path recognized this semantic journal.
    fs::create_dir_all(root.path().join("semantic"))?;
    fs::write(root.path().join("semantic/journal.cbor"), b"legacy-journal")?;
    assert_refused_before_root_mutation(root.path())
}

#[test]
fn production_boot_refuses_auxiliary_snapshot_before_creating_catalog_or_lease() -> TestResult {
    let root = private_root()?;
    let snapshot = root.path().join(LEGACY_AUXILIARY_SNAPSHOT_RELATIVES[0]);
    fs::create_dir_all(snapshot.parent().ok_or("snapshot has no parent")?)?;
    fs::write(
        &snapshot,
        quanta_index_ipc::encode_cbor_payload(&serde_json::json!({"entries": {}}))?,
    )?;
    assert_refused_before_root_mutation(root.path())?;
    if root.path().join("catalog").exists() || root.path().join(".searchd-state-root.lock").exists()
    {
        return Err("refused old-Q boot created a catalog or state-root lease".into());
    }
    Ok(())
}

#[test]
fn an_envelope_over_its_ceiling_refuses_boot_typed_before_any_socket() -> TestResult {
    let root = private_root()?;
    let declared = SearchdConfig::from_test_state_root(root.path().to_path_buf())
        .process_memory_envelope()?
        .declared_bytes()?;
    let ceiling = declared
        .checked_sub(1)
        .ok_or("empty process memory envelope")?;
    let binary = LegacySearchdBinary::from_environment()?;
    let mut command = binary.benchmark_command(root.path(), 8)?;
    let _configured = command.env(
        "QUANTA_INDEX_PROCESS_MEMORY_CEILING_BYTES",
        ceiling.to_string(),
    );
    let output = legacy_start_probe::wait_for_refusal(&binary, command)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() != Some(70)
        || !output.stdout.is_empty()
        || !stderr.contains(PROCESS_MEMORY_ENVELOPE_EXCEEDED_CODE.as_wire_str())
        || !stderr.contains(&format!("declared policies sum to {declared} bytes"))
        || !stderr.contains(&format!("over the {ceiling} byte ceiling"))
        || root.path().join("search-plane").try_exists()?
    {
        return Err(format!(
            "old-Q did not refuse the oversized envelope before socket setup: exit={:?}, stderr={stderr}",
            output.status.code(),
        ).into());
    }
    Ok(())
}
