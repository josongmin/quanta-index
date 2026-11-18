//! Match a carried Native route and its actual Catalog ledger against the held
//! previous root. Candidate-owned objects cannot establish their own provenance.

use super::*;
use quanta_index_core::index::{
    with_native_protected_snapshot_v1, with_native_protected_source_routes_v1,
};

pub(super) fn verify_preserved_native_v1(
    previous: &HeldPreviousNativeV1,
    candidate: &NativeBootAuthority,
    command: &NativeStateCommandV1,
    budget: &mut WorkBudget,
) -> Result<()> {
    let old_manifest = previous
        .observed_v1()
        .snapshot
        .selected
        .as_ref()
        .ok_or(IndexError::SnapshotUnavailable)?;
    let expected = command.identity.ok_or(IndexError::SnapshotUnavailable)?;
    if old_manifest.protected_source_routes.is_none() {
        if expected.protected_source_routes.is_some() {
            return Err(IndexError::DigestConflict.into());
        }
        return Ok(());
    }
    let root = candidate.lease.state_root_identity_v1();
    let observed = verify_native_boot_catalog(root, candidate.io.as_ref(), budget)?;
    let manifest = observed
        .selected
        .as_ref()
        .ok_or(IndexError::SnapshotUnavailable)?;
    if identity(observed.root_nonce, manifest)? != expected {
        return Err(IndexError::DigestConflict.into());
    }
    let route = manifest
        .protected_source_routes
        .ok_or(IndexError::SnapshotUnavailable)?;
    let objects = root.join("native-objects");
    let io = Arc::new(NativeObjectReadIo::new(Arc::clone(&candidate.physical)));
    let lock = io.open(&objects.join(".object-namespace.lease"), budget)?;
    let reader = FileObjectReader::open_existing(&objects, lock, io, budget)?;
    if observed.namespace != Some(reader.namespace_binding()?) {
        return Err(IndexError::DigestConflict.into());
    }
    let route_object = reader.get(route.object, budget)?;
    previous.with_protected_routes_v1(budget, |old_routes, _old_reader, budget| {
        quanta_index_catalog::visit_native_protected_snapshot_rows_v1(
            &previous.selected_config_v1().state_root,
            previous.catalog_io_v1(),
            old_manifest,
            old_routes,
            budget,
            |old_snapshot, budget| {
                with_native_protected_source_routes_v1(
                    route.object,
                    Some(route.count),
                    &route_object,
                    budget,
                    |routes, budget| {
                        let origin = routes
                            .native_origin
                            .as_ref()
                            .ok_or(IndexError::SnapshotUnavailable)?;
                        if origin.source_root_nonce != old_snapshot.source_root_nonce
                            || origin.source_routes != old_snapshot.source_routes
                            || origin.snapshot
                                != old_snapshot
                                    .object_ref_v1()
                                    .map_err(|_| IndexError::DigestConflict)?
                            || routes.heads != old_routes.heads
                            || routes.repomap_heads != old_routes.repomap_heads
                        {
                            return Err(IndexError::DigestConflict);
                        }
                        let stored = reader.get(origin.snapshot, budget)?;
                        with_native_protected_snapshot_v1(
                            origin.snapshot,
                            &stored,
                            budget,
                            |snapshot, budget| {
                                if snapshot != old_snapshot {
                                    return Err(IndexError::DigestConflict);
                                }
                                // Compare the installed SQL rows as well as the staged
                                // snapshot. A correct artifact cannot hide lost claims,
                                // altered event fences, extra rows or a changed frontier.
                                quanta_index_catalog::verify_native_protected_snapshot_install_v1(
                                    root,
                                    candidate.io.as_ref(),
                                    manifest,
                                    routes,
                                    snapshot,
                                    budget,
                                )
                            },
                        )
                    },
                )
            },
        )?;
        Ok(())
    })?;
    drop(reader);
    candidate.physical.sweep_retired()?;
    require_retired(candidate)?;
    previous.require_current_and_retired_v1(budget)
}
