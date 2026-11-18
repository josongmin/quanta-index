//! Borrow the exact settled Catalog ledger of selected protected Native heads.
//! The held root and physical object closure are authenticated by the caller.

use super::super::check_digest;
use super::*;
use quanta_index_contract::index::{
    MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1, MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1,
    NativeProtectedFrontierV1, NativeProtectedNativeSnapshotV1, NativeProtectedSourceAuthorityV1,
};

fn selected_workspace(routes: &NativeProtectedSourceRoutesV1, workspace: WorkspaceKey) -> bool {
    routes
        .heads
        .iter()
        .any(|head| head.operation.key.workspace == workspace)
        || routes
            .repomap_heads
            .iter()
            .any(|head| head.operation.key.workspace == workspace)
}

fn bounded_blob<'a>(
    row: &'a rusqlite::Row<'_>,
    column: usize,
    maximum: usize,
) -> Result<&'a [u8], IndexError> {
    let ValueRef::Blob(bytes) = row.get_ref(column).map_err(|error| sql(&error))? else {
        return Err(IndexError::DigestConflict);
    };
    if bytes.is_empty() || bytes.len() > maximum {
        return Err(IndexError::DigestConflict);
    }
    Ok(bytes)
}

// The selected route's named operations have already passed BOOT's bounded
// custody check. Other terminal rows may contain many Source events; their
// read is funded by the snapshot's measured raw-payload loan below.
fn charge_snapshot_operation(
    connection: &Connection,
    key: OperationKey,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let sizes: Option<(i64, i64, i64)> = connection
        .query_row(
            "SELECT CASE WHEN typeof(workspace)='blob' AND length(workspace) BETWEEN 1 AND 4096 THEN length(workspace) END,
                    CASE WHEN typeof(payload)='blob' AND length(payload) BETWEEN 1 AND ?2 THEN length(payload) END,
                    CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN 32 END
             FROM native_operation_v1 WHERE operation=?1",
            params![
                operation_key(key)?,
                i64::try_from(MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1)
                    .map_err(|_| IndexError::ResourceLimited)?
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let (workspace, payload, digest) = sizes.ok_or(IndexError::SnapshotUnavailable)?;
    let digest = uint(digest)?;
    let bytes = uint(workspace)?
        .checked_add(uint(payload)?)
        .and_then(|bytes| bytes.checked_add(digest))
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        reads: 1,
        bytes,
        ..WorkUsage::default()
    })
}

/// Return a borrowed, Original-funded snapshot only after one coherent SQLite
/// read has verified the selected route, complete work settlement, and each
/// canonical ledger row. The result remains provisional until the admitted
/// connection closes and its file identity is rechecked.
pub fn visit_native_protected_snapshot_rows_v1<T>(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    manifest: &NativeBootManifest,
    routes: &NativeProtectedSourceRoutesV1,
    budget: &mut WorkBudget,
    visit: impl FnOnce(&NativeProtectedNativeSnapshotV1, &mut WorkBudget) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
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
            visit_native_protected_snapshot_connection_v1(
                connection,
                observed.root_nonce,
                manifest,
                routes,
                budget,
                visit,
            )
        },
    )
}

pub(crate) fn visit_native_protected_snapshot_connection_v1<T>(
    connection: &Connection,
    root_nonce: [u8; 32],
    manifest: &NativeBootManifest,
    routes: &NativeProtectedSourceRoutesV1,
    budget: &mut WorkBudget,
    visit: impl FnOnce(&NativeProtectedNativeSnapshotV1, &mut WorkBudget) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    if boot_manifest::read_stored(connection)?.as_ref() != Some(manifest) {
        return Err(IndexError::DigestConflict);
    }
    boot_manifest::validate_custody(connection, manifest, budget)?;
    let route = manifest
        .protected_source_routes
        .ok_or(IndexError::SnapshotUnavailable)?;
    if usize::try_from(route.count).ok() != Some(routes.heads.len()) {
        return Err(IndexError::DigestConflict);
    }
    let _binding = boot_manifest::validate_protected_routes(
        connection,
        manifest.operation.key,
        route.object,
        routes,
        true,
        None,
        budget,
    )?;

    let capacity = routes
        .heads
        .len()
        .checked_add(routes.repomap_heads.len())
        .ok_or(IndexError::ResourceLimited)?;
    let workspace_backing = u64::try_from(capacity)
        .map_err(|_| IndexError::ResourceLimited)?
        .checked_mul(
            u64::try_from(std::mem::size_of::<WorkspaceKey>())
                .map_err(|_| IndexError::ResourceLimited)?,
        )
        .and_then(|bytes| bytes.checked_add(65_536))
        .ok_or(IndexError::ResourceLimited)?;
    let _workspace_memory = budget
        .reserve_memory(workspace_backing, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let mut workspaces = Vec::with_capacity(capacity);
    workspaces.extend(routes.heads.iter().map(|head| head.operation.key.workspace));
    workspaces.extend(
        routes
            .repomap_heads
            .iter()
            .map(|head| head.operation.key.workspace),
    );
    workspaces.sort_unstable();
    workspaces.dedup();

    let mut counts = [0usize; 4];
    let mut raw_bytes = 0_u64;
    for workspace in &workspaces {
        let workspace = workspace_key(*workspace)?;
        for (index, query) in [
                    "SELECT count(*),coalesce(sum(length(payload)),0) FROM native_operation_v1 WHERE workspace=?1",
                    "SELECT count(*),coalesce(sum(length(c.payload)),0) FROM native_event_claim_v1 c JOIN native_operation_v1 o ON c.operation=o.operation WHERE o.workspace=?1",
                    "SELECT count(*),0 FROM native_frontier_v1 WHERE workspace=?1",
                    "SELECT count(*),0 FROM native_source_authority_v1 WHERE workspace=?1",
                ].iter().enumerate() {
                    budget.charge(WorkUsage { reads: 1, ..WorkUsage::default() })?;
                    let (count, bytes): (i64, i64) = connection.query_row(query, params![&workspace],
                        |row| Ok((row.get(0)?, row.get(1)?))).map_err(|error| sql(&error))?;
                    counts[index] = counts[index].checked_add(
                        usize::try_from(uint(count)?).map_err(|_| IndexError::ResourceLimited)?)
                        .ok_or(IndexError::ResourceLimited)?;
                    if counts[index] > MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1 {
                        return Err(IndexError::ResourceLimited);
                    }
                    raw_bytes = raw_bytes.checked_add(uint(bytes)?)
                        .ok_or(IndexError::ResourceLimited)?;
                }
    }
    if counts[0] == 0 {
        return Err(IndexError::SnapshotUnavailable);
    }
    let row_count = counts.iter().try_fold(0usize, |sum, count| {
        sum.checked_add(*count).ok_or(IndexError::ResourceLimited)
    })?;
    if row_count
        .checked_add(
            routes
                .repomap_heads
                .iter()
                .filter(|head| head.source_operation.is_none())
                .count(),
        )
        .ok_or(IndexError::ResourceLimited)?
        > MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1
    {
        return Err(IndexError::ResourceLimited);
    }
    // Retain raw row DATA plus the worst-case JSON byte-array encoding and
    // canonical object hashing performed by the borrowed visitor.
    let backing = raw_bytes
        .checked_mul(8)
        .and_then(|bytes| bytes.checked_add(u64::try_from(row_count).ok()?.checked_mul(512)?))
        .and_then(|bytes| bytes.checked_add(512_000))
        .ok_or(IndexError::ResourceLimited)?;
    let _snapshot_memory = budget
        .reserve_memory(backing, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let mut result = NativeProtectedNativeSnapshotV1 {
        wire_version: NativeProtectedNativeSnapshotV1::WIRE_VERSION,
        source_root_nonce: Digest(root_nonce),
        source_routes: route.object,
        operation_rows: Vec::with_capacity(counts[0]),
        genesis_views: Vec::with_capacity(routes.repomap_heads.len()),
        frontiers: Vec::with_capacity(counts[2]),
        source_authorities: Vec::with_capacity(counts[3]),
        event_claims: Vec::with_capacity(counts[1]),
        roots: vec![route.object],
    };
    for workspace in workspaces {
        if !selected_workspace(routes, workspace) {
            return Err(IndexError::DigestConflict);
        }
        let scope = workspace_key(workspace)?;
        let mut statement = connection.prepare(
                    "SELECT operation,payload,row_digest FROM native_operation_v1 WHERE workspace=?1 ORDER BY operation"
                ).map_err(|error| sql(&error))?;
        let mut rows = statement
            .query(params![&scope])
            .map_err(|error| sql(&error))?;
        while let Some(row) = rows.next().map_err(|error| sql(&error))? {
            budget.charge(WorkUsage {
                reads: 1,
                bytes: 8192,
                ..WorkUsage::default()
            })?;
            let key: OperationKey = decode(bounded_blob(row, 0, 4096)?)?;
            // Source operations can carry many real events. The total snapshot
            // is bounded and funded above; a fixed 4 KiB row cap would reject
            // valid persisted terminal publications.
            let payload = bounded_blob(row, 1, MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1)?;
            check_digest(
                b"qidx.native.operation.v1",
                payload,
                bounded_blob(row, 2, 32)?,
            )?;
            charge_snapshot_operation(connection, key, budget)?;
            let original =
                read_operation(connection, key)?.ok_or(IndexError::SnapshotUnavailable)?;
            if original.immutable.key.workspace != workspace
                || encode(&original)?.as_slice() != payload
            {
                return Err(IndexError::DigestConflict);
            }
            match original.status {
                OperationStatus::Published(view) => {
                    let publication = original
                        .published_view
                        .as_ref()
                        .ok_or(IndexError::SnapshotUnavailable)?;
                    if publication.identity != view {
                        return Err(IndexError::DigestConflict);
                    }
                }
                OperationStatus::Refused(_) => {}
                OperationStatus::Unknown | OperationStatus::Pending(_) => {
                    return Err(IndexError::Conflict);
                }
            }
            result.operation_rows.push(payload.to_vec());
        }
        drop(rows);
        drop(statement);

        let mut statement = connection.prepare(
                    "SELECT c.identity,c.operation,c.payload,c.row_digest FROM native_event_claim_v1 c JOIN native_operation_v1 o ON c.operation=o.operation WHERE o.workspace=?1 ORDER BY c.identity"
                ).map_err(|error| sql(&error))?;
        let mut rows = statement
            .query(params![&scope])
            .map_err(|error| sql(&error))?;
        while let Some(row) = rows.next().map_err(|error| sql(&error))? {
            budget.charge(WorkUsage {
                reads: 1,
                bytes: 12_288,
                ..WorkUsage::default()
            })?;
            let identity = bounded_blob(row, 0, 4096)?;
            let owner = bounded_blob(row, 1, 4096)?;
            let payload = bounded_blob(row, 2, 4096)?;
            check_digest(
                b"qidx.native.eventclaim.v1",
                payload,
                bounded_blob(row, 3, 32)?,
            )?;
            let claim: super::super::EventClaim = decode(payload)?;
            if super::super::event_identity(claim.operation, claim.event)?.as_slice() != identity
                || operation_key(claim.operation)?.as_slice() != owner
                || claim.operation.workspace != workspace
                || encode(&claim)?.as_slice() != payload
            {
                return Err(IndexError::DigestConflict);
            }
            charge_snapshot_operation(connection, claim.operation, budget)?;
            let source = read_operation(connection, claim.operation)?
                .ok_or(IndexError::SnapshotUnavailable)?;
            if !source.events.contains(&claim.event)
                || !matches!(
                    (source.status, claim.state),
                    (
                        OperationStatus::Published(_),
                        super::super::EventClaimState::Committed
                    ) | (
                        OperationStatus::Refused(_),
                        super::super::EventClaimState::Refused
                    )
                )
            {
                return Err(IndexError::Conflict);
            }
            result.event_claims.push(payload.to_vec());
        }
        drop(rows);
        drop(statement);

        let mut statement = connection.prepare(
                    "SELECT producer,stream,stream_epoch,sequence,row_digest FROM native_frontier_v1 WHERE workspace=?1 ORDER BY producer,stream"
                ).map_err(|error| sql(&error))?;
        let mut rows = statement
            .query(params![&scope])
            .map_err(|error| sql(&error))?;
        while let Some(row) = rows.next().map_err(|error| sql(&error))? {
            budget.charge(WorkUsage {
                reads: 1,
                bytes: 256,
                ..WorkUsage::default()
            })?;
            let producer: [u8; 16] = bounded_blob(row, 0, 16)?
                .try_into()
                .map_err(|_| IndexError::DigestConflict)?;
            let stream: [u8; 16] = bounded_blob(row, 1, 16)?
                .try_into()
                .map_err(|_| IndexError::DigestConflict)?;
            let epoch: i64 = row.get(2).map_err(|error| sql(&error))?;
            let sequence: i64 = row.get(3).map_err(|error| sql(&error))?;
            let actual = bounded_blob(row, 4, 32)?;
            let expected = super::super::fields_digest(
                b"qidx.native.frontier.v1",
                &(&scope, producer.to_vec(), stream.to_vec(), epoch, sequence),
            )?;
            if actual != expected.as_slice() {
                return Err(IndexError::DigestConflict);
            }
            result.frontiers.push(NativeProtectedFrontierV1 {
                workspace,
                producer: quanta_index_core::index::ProducerId(producer),
                stream: quanta_index_core::index::StreamId(stream),
                stream_epoch: uint(epoch)?,
                sequence: uint(sequence)?,
            });
        }
        drop(rows);
        drop(statement);

        let mut statement = connection.prepare(
                    "SELECT workspace_epoch,producer,source_epoch,row_digest FROM native_source_authority_v1 WHERE workspace=?1 ORDER BY workspace_epoch,producer"
                ).map_err(|error| sql(&error))?;
        let mut rows = statement
            .query(params![&scope])
            .map_err(|error| sql(&error))?;
        while let Some(row) = rows.next().map_err(|error| sql(&error))? {
            budget.charge(WorkUsage {
                reads: 1,
                bytes: 256,
                ..WorkUsage::default()
            })?;
            let epoch: i64 = row.get(0).map_err(|error| sql(&error))?;
            let producer: [u8; 16] = bounded_blob(row, 1, 16)?
                .try_into()
                .map_err(|_| IndexError::DigestConflict)?;
            let source_epoch: i64 = row.get(2).map_err(|error| sql(&error))?;
            let actual = bounded_blob(row, 3, 32)?;
            let expected = super::super::fields_digest(
                b"qidx.native.sourceauthority.v1",
                &(&scope, epoch, producer.to_vec(), source_epoch),
            )?;
            if actual != expected.as_slice() {
                return Err(IndexError::DigestConflict);
            }
            result
                .source_authorities
                .push(NativeProtectedSourceAuthorityV1 {
                    workspace,
                    workspace_epoch: uint(epoch)?,
                    producer: quanta_index_core::index::ProducerId(producer),
                    source_epoch: uint(source_epoch)?,
                });
        }
        drop(rows);
        drop(statement);
    }
    for head in &routes.repomap_heads {
        if head.source_operation.is_none() {
            let genesis = retention::charged_exact_view(
                connection,
                head.source_publication.identity,
                budget,
            )?;
            if genesis != head.source_publication || genesis.identity.sequence != 0 {
                return Err(IndexError::DigestConflict);
            }
            result.genesis_views.push(genesis);
        }
    }
    for head in &routes.heads {
        result.roots.extend([
            head.operation.request,
            head.source_operation.request,
            head.publication.profile,
            head.source_publication.profile,
        ]);
        result.roots.extend(
            [
                head.publication.source.owners.0,
                head.publication.source.paths.0,
                head.publication.source.frontiers.0,
                head.publication.projections.0,
                head.publication.members.0,
                head.publication.coverage.0,
            ]
            .into_iter()
            .flatten(),
        );
    }
    for head in &routes.repomap_heads {
        result.roots.push(head.operation.request);
        result.roots.push(head.publication.profile);
        result.roots.push(head.source_publication.profile);
        if let Some(source) = head.source_operation.as_ref() {
            result.roots.push(source.request);
        }
        result.roots.extend(
            [
                head.publication.source.owners.0,
                head.publication.source.paths.0,
                head.publication.source.frontiers.0,
                head.publication.projections.0,
                head.publication.members.0,
                head.publication.coverage.0,
            ]
            .into_iter()
            .flatten(),
        );
        result.roots.push(head.member_binding);
    }
    result.roots.sort_unstable();
    result.roots.dedup();
    result
        .validate_shape_v1()
        .map_err(|_| IndexError::ResourceLimited)?;
    let bytes = result
        .encoded_payload_len_v1()
        .map_err(|_| IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes: u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let value = visit(&result, budget)?;
    budget.checkpoint()?;
    Ok(value)
}
