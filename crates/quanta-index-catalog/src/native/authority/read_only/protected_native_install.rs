//! Destination-only census for a Native protected-workspace import.
//! The old root may retain historical Exact views; the fresh destination may
//! contain only declared routed heads and map-only genesis views.

use super::protected_native_snapshot::NativeProtectedSnapshotWitnessV1;
use super::*;

pub fn verify_native_protected_snapshot_install_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    manifest: &NativeBootManifest,
    routes: &NativeProtectedSourceRoutesV1,
    original: &NativeProtectedSnapshotWitnessV1,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if routes.native_origin.as_ref() != Some(original.origin_v1()) {
        return Err(IndexError::DigestConflict);
    }
    with_admitted_connection(
        state_root,
        io,
        budget,
        32_768,
        |error| error,
        |connection, path, budget, allocation| {
            let (observed, _, _, _) = inspect(
                connection,
                path,
                None,
                RollbackCheck::None,
                None,
                None,
                budget,
                allocation,
            )?;
            if observed.selected.as_ref() != Some(manifest) {
                return Err(IndexError::DigestConflict);
            }
            protected_native_snapshot::compare_native_protected_snapshot_witness_connection_v1(
                connection, manifest, routes, original, budget,
            )?;
            verify_imported_exact_views_v1(connection, routes, budget)
        },
    )
}

pub(in crate::native::authority) fn verify_imported_exact_views_v1(
    connection: &Connection,
    routes: &NativeProtectedSourceRoutesV1,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    // Each read loads one fixed-width view and its bounded root rows.
    let _scratch = budget
        .reserve_memory(16_384, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    routes.validate().map_err(|_| IndexError::DigestConflict)?;
    for head in &routes.heads {
        verify_workspace_exact_views(connection, &head.publication, None, budget)?;
    }
    for head in &routes.repomap_heads {
        let pair = (head.repo_id.as_str(), head.revision_id.as_str());
        // Route validation permits a shared workspace only when both heads
        // declare the same pair and the same published view.
        if routes
            .heads
            .binary_search_by(|source| {
                (source.repo_id.as_str(), source.revision_id.as_str()).cmp(&pair)
            })
            .is_ok()
        {
            continue;
        }
        let genesis = head
            .source_operation
            .is_none()
            .then_some(&head.source_publication);
        verify_workspace_exact_views(connection, &head.publication, genesis, budget)?;
    }
    Ok(())
}

fn verify_workspace_exact_views(
    connection: &Connection,
    publication: &CatalogView,
    genesis: Option<&CatalogView>,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if genesis.is_some_and(|genesis| genesis.identity == publication.identity) {
        return Err(IndexError::DigestConflict);
    }
    let (expected_count, row_limit): (i64, i64) = if genesis.is_some() { (2, 3) } else { (1, 2) };
    budget.charge(WorkUsage {
        reads: 1,
        bytes: 4096,
        ..WorkUsage::default()
    })?;
    let count: i64 = connection
        .query_row(
            "SELECT count(*) FROM (
                 SELECT 1 FROM native_view_v1 WHERE workspace=?1 LIMIT ?2
             )",
            params![workspace_key(publication.identity.workspace)?, row_limit],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    if count != expected_count {
        return Err(IndexError::DigestConflict);
    }
    for view in std::iter::once(publication).chain(genesis) {
        if &retention::charged_exact_view(connection, view.identity, budget)? != view {
            return Err(IndexError::DigestConflict);
        }
    }
    Ok(())
}
