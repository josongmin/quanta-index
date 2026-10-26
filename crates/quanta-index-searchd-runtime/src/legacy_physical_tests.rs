//! Physical admission must precede all mutable production adapter opens.

use quanta_index_core::{CoreError, INGEST_RESOURCE_BUDGET_EXCEEDED_CODE};
use quanta_index_searchd::app::config::SearchdConfig;
use quanta_index_searchd::app::state_format::{StateRootFormatV1, detect_state_root_format_v1};
use std::os::unix::fs::PermissionsExt as _;

#[test]
fn missing_physical_policy_refuses_before_state_root_creation()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("must-remain-absent");
    let config = SearchdConfig::from_test_state_root(root.clone())
        .try_with_legacy_publication_physical_policy(None)?;
    let Err(error) = super::build_runtime(config) else {
        return Err("missing physical policy booted".into());
    };
    if !matches!(
        error.downcast_ref::<CoreError>(),
        Some(CoreError::InvalidContract(_))
    ) {
        return Err(format!("wrong pre-open refusal: {error:#}").into());
    }
    if root.try_exists()? {
        return Err("refused physical composition created the state root".into());
    }
    Ok(())
}

#[test]
fn physical_inventory_refusal_preserves_existing_bytes_before_adapter_open()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("state");
    std::fs::create_dir(&root)?;
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
    // An existing current-format root needs a recognised owned directory.
    // Keep it empty so a Catalog open would remain visible after refusal.
    let catalog_marker = root.join("catalog");
    std::fs::create_dir(&catalog_marker)?;
    std::fs::set_permissions(&catalog_marker, std::fs::Permissions::from_mode(0o700))?;
    let existing = root.join("existing-object");
    std::fs::write(&existing, b"existing-immutable-object")?;
    if detect_state_root_format_v1(&root)? != (StateRootFormatV1::CurrentV1 { manifest: false }) {
        return Err("fixture is not a current-format root".into());
    }
    let config = SearchdConfig::from_test_state_root(root.clone())
        .try_with_search_corpus_history_retention_limits(8, 16 << 20, 128, 256 << 20)?;
    let mut policy = config
        .legacy_publication_physical_policy()
        .ok_or("test constructor omitted its explicit fixture policy")?;
    policy.inventory.bytes = 8;
    for parent in [
        &mut policy.maintenance,
        &mut policy.control,
        &mut policy.work,
        &mut policy.history,
        &mut policy.catalog_parent,
    ] {
        parent.bytes = 1;
        parent.files = 1;
    }
    policy.durable_control_file_bytes = 1;
    policy.repomap_control_file_bytes = 1;
    policy.repomap_candidate_file_bytes = 1;
    let config = config
        .try_with_source_publication_max_bytes(1)?
        .try_with_legacy_publication_physical_policy(Some(policy))?;
    let Err(error) = super::build_runtime(config) else {
        return Err("over-cap persisted inventory booted".into());
    };
    if !matches!(error.downcast_ref::<CoreError>(),
        Some(CoreError::Typed { code, .. }) if *code == INGEST_RESOURCE_BUDGET_EXCEEDED_CODE)
    {
        return Err(format!("wrong inventory refusal: {error:#}").into());
    }
    if std::fs::read(&existing)? != b"existing-immutable-object" {
        return Err("inventory refusal changed original bytes".into());
    }
    if std::fs::read_dir(&catalog_marker)?.next().is_some() {
        return Err("inventory refusal opened the Catalog before admission".into());
    }
    for name in ["indexes", "authorities", "repo-map", "activations"] {
        if root.join(name).try_exists()? {
            return Err(format!("inventory refusal opened mutable {name}").into());
        }
    }
    Ok(())
}
