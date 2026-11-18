//! Destination-only census for a Native protected-workspace import.
//! The old root may retain historical Exact views; the fresh destination may
//! contain only declared routed heads and map-only genesis views.

use super::*;
use quanta_index_contract::index::NativeProtectedNativeSnapshotV1;
use std::collections::BTreeMap;

pub fn verify_native_protected_snapshot_install_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    manifest: &NativeBootManifest,
    routes: &NativeProtectedSourceRoutesV1,
    original: &NativeProtectedNativeSnapshotV1,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    original
        .validate_shape_v1()
        .map_err(|_| IndexError::DigestConflict)?;
    let encoded = u64::try_from(
        original
            .encoded_payload_len_v1()
            .map_err(|_| IndexError::DigestConflict)?,
    )
    .map_err(|_| IndexError::ResourceLimited)?;
    let _original_memory = budget
        .reserve_memory(
            encoded.checked_mul(2).ok_or(IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes: encoded,
        ..WorkUsage::default()
    })?;
    let origin = routes
        .native_origin
        .as_ref()
        .ok_or(IndexError::DigestConflict)?;
    if origin.source_root_nonce != original.source_root_nonce
        || origin.source_routes != original.source_routes
        || origin.snapshot
            != original
                .object_ref_v1()
                .map_err(|_| IndexError::DigestConflict)?
    {
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
            protected_native_snapshot::visit_native_protected_snapshot_connection_v1(
                connection,
                observed.root_nonce,
                manifest,
                routes,
                budget,
                |installed, budget| {
                    if installed.operation_rows != original.operation_rows
                        || installed.genesis_views != original.genesis_views
                        || installed.frontiers != original.frontiers
                        || installed.source_authorities != original.source_authorities
                        || installed.event_claims != original.event_claims
                    {
                        return Err(IndexError::DigestConflict);
                    }
                    verify_imported_exact_views_v1(connection, routes, budget)
                },
            )
        },
    )
}

pub(in crate::native::authority) fn verify_imported_exact_views_v1(
    connection: &Connection,
    routes: &NativeProtectedSourceRoutesV1,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let capacity = routes
        .heads
        .len()
        .checked_add(
            routes
                .repomap_heads
                .len()
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .ok_or(IndexError::ResourceLimited)?;
    let bytes = u64::try_from(capacity)
        .map_err(|_| IndexError::ResourceLimited)?
        .checked_mul(1024)
        .and_then(|value| value.checked_add(16_384))
        .ok_or(IndexError::ResourceLimited)?;
    let _memory = budget
        .reserve_memory(bytes, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let mut expected: BTreeMap<WorkspaceKey, BTreeMap<ViewRef, CatalogView>> = BTreeMap::new();
    let mut insert = |view: &CatalogView| -> Result<(), IndexError> {
        let views = expected.entry(view.identity.workspace).or_default();
        if views
            .insert(view.identity, view.clone())
            .is_some_and(|prior| prior != *view)
        {
            return Err(IndexError::DigestConflict);
        }
        Ok(())
    };
    for head in &routes.heads {
        insert(&head.publication)?;
    }
    for head in &routes.repomap_heads {
        insert(&head.publication)?;
        if head.source_operation.is_none() {
            insert(&head.source_publication)?;
        }
    }
    for (workspace, views) in expected {
        budget.charge(WorkUsage {
            reads: u64::try_from(views.len())
                .map_err(|_| IndexError::ResourceLimited)?
                .checked_mul(3)
                .and_then(|value| value.checked_add(1))
                .ok_or(IndexError::ResourceLimited)?,
            bytes: 4096,
            ..WorkUsage::default()
        })?;
        let count: i64 = connection
            .query_row(
                "SELECT count(*) FROM native_view_v1 WHERE workspace=?1",
                params![workspace_key(workspace)?],
                |row| row.get(0),
            )
            .map_err(|error| sql(&error))?;
        if usize::try_from(count).ok() != Some(views.len()) {
            return Err(IndexError::DigestConflict);
        }
        for (identity, expected_view) in views {
            if retention::charged_exact_view(connection, identity, budget)? != expected_view {
                return Err(IndexError::DigestConflict);
            }
        }
    }
    Ok(())
}
