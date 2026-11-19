//! Borrow the exact settled Catalog ledger of selected protected Native heads.
//! The held root and physical object closure are authenticated by the caller.

use super::super::{
    check_digest, decode_stored_operation,
    protected_native_import::{
        canonical_row_v1, operation_scratch_v1, require_attached_event_claims_v1,
        validate_claims_v1,
    },
};
use super::*;
use quanta_index_contract::index::{
    MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1, MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1,
    NativeProtectedFrontierV1, NativeProtectedNativeOriginV1, NativeProtectedNativeSnapshotV1,
    NativeProtectedSourceAuthorityV1, ObjectRef,
};
use sha2::{Digest as _, Sha256};
use std::{io, mem::size_of};

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

fn charge_snapshot_row(sizes: &[usize], budget: &mut WorkBudget) -> Result<(), IndexError> {
    let bytes = sizes
        .iter()
        .copied()
        .try_fold(0_usize, usize::checked_add)
        .and_then(|bytes| u64::try_from(bytes).ok())
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        reads: 1,
        bytes,
        ..WorkUsage::default()
    })
}

/// Visit the funded snapshot after the admitted SQLite connection has closed
/// and its input identities have been rechecked. The caller retains the old
/// root lease while consuming the snapshot and its physical object closure.
pub fn visit_native_protected_snapshot_rows_v1<T>(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    manifest: &NativeBootManifest,
    routes: &NativeProtectedSourceRoutesV1,
    budget: &mut WorkBudget,
    visit: impl FnOnce(&NativeProtectedNativeSnapshotV1, &mut WorkBudget) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    let snapshot = with_admitted_connection(
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
            read_native_protected_snapshot_connection_v1(
                connection,
                observed.root_nonce,
                manifest,
                routes,
                budget,
            )
        },
    )?;
    budget.checkpoint()?;
    let value = visit(&snapshot.rows, budget)?;
    budget.checkpoint()?;
    Ok(value)
}

#[cfg(test)]
pub(crate) fn visit_native_protected_snapshot_connection_v1<T>(
    connection: &Connection,
    root_nonce: [u8; 32],
    manifest: &NativeBootManifest,
    routes: &NativeProtectedSourceRoutesV1,
    budget: &mut WorkBudget,
    visit: impl FnOnce(&NativeProtectedNativeSnapshotV1, &mut WorkBudget) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    let snapshot = read_native_protected_snapshot_connection_v1(
        connection, root_nonce, manifest, routes, budget,
    )?;
    let value = visit(&snapshot.rows, budget)?;
    budget.checkpoint()?;
    Ok(value)
}

struct FundedSnapshot {
    rows: NativeProtectedNativeSnapshotV1,
    _memory: MemoryLease,
}

fn read_native_protected_snapshot_connection_v1(
    connection: &Connection,
    root_nonce: [u8; 32],
    manifest: &NativeBootManifest,
    routes: &NativeProtectedSourceRoutesV1,
    budget: &mut WorkBudget,
) -> Result<FundedSnapshot, IndexError> {
    let census = prepare_snapshot_census(connection, manifest, routes, budget)?;
    let SnapshotRowCounts {
        counts:
            [
                operation_count,
                claim_count,
                frontier_count,
                authority_count,
            ],
        genesis_count,
        raw_bytes,
        keys,
    } = measure_snapshot_rows(connection, routes, &census.workspaces, budget)?;
    // Count every root before deduplication. The settled SQL transaction
    // fixes the row counts and the raw sizes through the scans below.
    let root_capacity = routes
        .heads
        .len()
        .checked_mul(10)
        .and_then(|heads| {
            routes
                .repomap_heads
                .len()
                .checked_mul(11)
                .and_then(|maps| heads.checked_add(maps))
        })
        .and_then(|roots| roots.checked_add(1))
        .ok_or(IndexError::ResourceLimited)?;
    let backing = [
        (operation_count, size_of::<Vec<u8>>()),
        (claim_count, size_of::<Vec<u8>>()),
        (frontier_count, size_of::<NativeProtectedFrontierV1>()),
        (
            authority_count,
            size_of::<NativeProtectedSourceAuthorityV1>(),
        ),
        (genesis_count, size_of::<CatalogView>()),
        (root_capacity, size_of::<ObjectRef>()),
    ]
    .into_iter()
    .try_fold(raw_bytes, |total, (count, size)| {
        let bytes = u64::try_from(count.checked_mul(size)?).ok()?;
        total.checked_add(bytes)
    })
    .ok_or(IndexError::ResourceLimited)?;
    let snapshot_memory = budget
        .reserve_memory(backing, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let mut result = NativeProtectedNativeSnapshotV1 {
        wire_version: NativeProtectedNativeSnapshotV1::WIRE_VERSION,
        source_root_nonce: Digest(root_nonce),
        source_routes: census.route,
        operation_rows: Vec::with_capacity(operation_count),
        genesis_views: Vec::with_capacity(genesis_count),
        frontiers: Vec::with_capacity(frontier_count),
        source_authorities: Vec::with_capacity(authority_count),
        event_claims: Vec::with_capacity(claim_count),
        roots: Vec::with_capacity(root_capacity),
    };
    result.roots.push(census.route);
    scan_snapshot_rows(
        connection,
        routes,
        &census.workspaces,
        keys,
        budget,
        |row| {
            match row {
                SnapshotRow::Operation(payload) => result.operation_rows.push(payload.to_vec()),
                SnapshotRow::Claim(payload) => result.event_claims.push(payload.to_vec()),
                SnapshotRow::Frontier(frontier) => result.frontiers.push(frontier),
                SnapshotRow::Authority(authority) => result.source_authorities.push(authority),
                SnapshotRow::Genesis(genesis) => result.genesis_views.push(genesis.clone()),
            }
            Ok(())
        },
    )?;
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
    validate_snapshot_claims(&result, budget)?;
    result
        .validate_shape_v1()
        .map_err(|_| IndexError::ResourceLimited)?;
    let bytes = result
        .encoded_payload_len_with_work_v1(&mut || budget.checkpoint())
        .map_err(|cause| cause.into_error(|_| IndexError::ResourceLimited))?;
    budget.charge(WorkUsage {
        bytes: u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    Ok(FundedSnapshot {
        rows: result,
        _memory: snapshot_memory,
    })
}

/// Compact result of an authenticated predecessor census. It retains no
/// decoded snapshot rows or physical payload backing.
pub struct NativeProtectedSnapshotWitnessV1 {
    origin: NativeProtectedNativeOriginV1,
    pages: [Option<ObjectRef>; 2],
    counts: [usize; 5],
    digests: [[u8; 32]; 5],
}

impl NativeProtectedSnapshotWitnessV1 {
    pub fn origin_v1(&self) -> &NativeProtectedNativeOriginV1 {
        &self.origin
    }

    pub fn object_references_v1(&self) -> impl Iterator<Item = ObjectRef> {
        std::iter::once(self.origin.snapshot).chain(self.pages.into_iter().flatten())
    }

    #[cfg(test)]
    fn from_snapshot(
        snapshot: &NativeProtectedNativeSnapshotV1,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        Self::from_snapshot_for_origin(snapshot, None, budget)
    }

    fn from_snapshot_for_origin(
        snapshot: &NativeProtectedNativeSnapshotV1,
        expected: Option<&NativeProtectedNativeOriginV1>,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        if expected.is_some_and(|expected| {
            expected.source_root_nonce != snapshot.source_root_nonce
                || expected.source_routes != snapshot.source_routes
        }) {
            return Err(IndexError::DigestConflict);
        }
        let plan = snapshot
            .physical_plan_with_work_v1(&mut || budget.checkpoint())
            .map_err(|cause| cause.into_error(|_| IndexError::DigestConflict))?;
        let bytes = plan.pages_v1().map(|(_, len)| len).sum::<usize>();
        budget.charge(WorkUsage {
            bytes: u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let mut pages = if let Some((_, _, children)) = plan.manifest_v1() {
            let [first, second] = children else {
                return Err(IndexError::DigestConflict);
            };
            [Some(*first), Some(*second)]
        } else {
            [None, None]
        };
        let mut origin = NativeProtectedNativeOriginV1 {
            source_root_nonce: snapshot.source_root_nonce,
            source_routes: snapshot.source_routes,
            snapshot: plan.reference_v1(),
        };
        if let Some(expected) = expected {
            if expected.snapshot != origin.snapshot {
                // The original V1 exporter wrote up to 32 MiB as one object.
                // Select that physical identity only when its exact canonical
                // body and ordered roots reproduce the candidate's reference.
                if plan.manifest_v1().is_none() {
                    return Err(IndexError::DigestConflict);
                }
                // A second count/hash serialization is real bounded work,
                // even though it retains no additional payload backing.
                budget.charge(WorkUsage {
                    bytes: u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                if snapshot
                    .legacy_inline_object_ref_with_work_v1(&mut || budget.checkpoint())
                    .map_err(|cause| cause.into_error(|_| IndexError::DigestConflict))?
                    != expected.snapshot
                {
                    return Err(IndexError::DigestConflict);
                }
                pages = [None, None];
                origin = expected.clone();
            }
        }
        let mut digests = SnapshotRowDigests::new();
        for row in snapshot
            .operation_rows
            .iter()
            .map(|row| SnapshotRow::Operation(row))
            .chain(
                snapshot
                    .event_claims
                    .iter()
                    .map(|row| SnapshotRow::Claim(row)),
            )
            .chain(
                snapshot
                    .frontiers
                    .iter()
                    .cloned()
                    .map(SnapshotRow::Frontier),
            )
            .chain(
                snapshot
                    .source_authorities
                    .iter()
                    .cloned()
                    .map(SnapshotRow::Authority),
            )
            .chain(snapshot.genesis_views.iter().map(SnapshotRow::Genesis))
        {
            budget.checkpoint()?;
            digests.record(row)?;
        }
        Ok(Self {
            origin,
            pages,
            counts: [
                snapshot.operation_rows.len(),
                snapshot.event_claims.len(),
                snapshot.frontiers.len(),
                snapshot.source_authorities.len(),
                snapshot.genesis_views.len(),
            ],
            digests: digests.finish(),
        })
    }
}

/// Read and validate every predecessor row before producing a fixed-width
/// witness for the later destination comparison.
pub fn read_native_protected_snapshot_witness_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    manifest: &NativeBootManifest,
    routes: &NativeProtectedSourceRoutesV1,
    expected_origin: &NativeProtectedNativeOriginV1,
    budget: &mut WorkBudget,
) -> Result<NativeProtectedSnapshotWitnessV1, IndexError> {
    visit_native_protected_snapshot_rows_v1(
        state_root,
        io,
        manifest,
        routes,
        budget,
        |snapshot, budget| {
            NativeProtectedSnapshotWitnessV1::from_snapshot_for_origin(
                snapshot,
                Some(expected_origin),
                budget,
            )
        },
    )
}

struct SnapshotRowDigests([Sha256; 5]);

impl SnapshotRowDigests {
    fn new() -> Self {
        Self(std::array::from_fn(|family| {
            let mut digest = Sha256::new();
            digest.update(b"qidx.native.protected-snapshot.census.v1\0");
            digest.update([family as u8]);
            digest
        }))
    }

    fn record(&mut self, row: SnapshotRow<'_>) -> Result<(), IndexError> {
        // Each family is its own ordered sequence of fixed-width row hashes.
        // Individual payload lengths cannot alias row boundaries.
        match row {
            SnapshotRow::Operation(bytes) => self.0[0].update(Sha256::digest(bytes)),
            SnapshotRow::Claim(bytes) => self.0[1].update(Sha256::digest(bytes)),
            SnapshotRow::Frontier(frontier) => self.0[2].update(hash_snapshot_row(&frontier)?),
            SnapshotRow::Authority(authority) => self.0[3].update(hash_snapshot_row(&authority)?),
            SnapshotRow::Genesis(genesis) => self.0[4].update(hash_snapshot_row(genesis)?),
        }
        Ok(())
    }

    fn finish(self) -> [[u8; 32]; 5] {
        self.0.map(|digest| digest.finalize().into())
    }
}

fn hash_snapshot_row(row: &impl serde::Serialize) -> Result<[u8; 32], IndexError> {
    struct HashWriter(Sha256);
    impl io::Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer(&mut writer, row)
        .map_err(|error| IndexError::Storage(error.to_string()))?;
    Ok(writer.0.finalize().into())
}

/// Compare every destination row against the authenticated predecessor.
/// The caller binds the origin and verifies the destination Exact views.
pub(in crate::native::authority) fn compare_native_protected_snapshot_witness_connection_v1(
    connection: &Connection,
    manifest: &NativeBootManifest,
    routes: &NativeProtectedSourceRoutesV1,
    original: &NativeProtectedSnapshotWitnessV1,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let census = prepare_snapshot_census(connection, manifest, routes, budget)?;
    let measured = measure_snapshot_rows(connection, routes, &census.workspaces, budget)?;
    let [operations, claims, frontiers, authorities] = measured.counts;
    if [
        operations,
        claims,
        frontiers,
        authorities,
        measured.genesis_count,
    ] != original.counts
    {
        return Err(IndexError::DigestConflict);
    }
    let mut digests = SnapshotRowDigests::new();
    scan_snapshot_rows(
        connection,
        routes,
        &census.workspaces,
        measured.keys,
        budget,
        |row| digests.record(row),
    )?;
    if digests.finish() != original.digests {
        return Err(IndexError::DigestConflict);
    }
    budget.checkpoint()
}

#[cfg(test)]
pub(in crate::native::authority) fn compare_native_protected_snapshot_connection_v1(
    connection: &Connection,
    manifest: &NativeBootManifest,
    routes: &NativeProtectedSourceRoutesV1,
    original: &NativeProtectedNativeSnapshotV1,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let original = NativeProtectedSnapshotWitnessV1::from_snapshot(original, budget)?;
    compare_native_protected_snapshot_witness_connection_v1(
        connection, manifest, routes, &original, budget,
    )
}

fn validate_snapshot_claims(
    snapshot: &NativeProtectedNativeSnapshotV1,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    // Use the restore validator to check the exact owner of each claim and
    // coverage of every Published event.
    let (_claim_memory, _claims) = validate_claims_v1(snapshot, budget).map_err(|error| {
        if error == IndexError::DigestConflict {
            IndexError::Conflict
        } else {
            error
        }
    })?;
    budget.checkpoint()
}

struct SnapshotCensus {
    route: ObjectRef,
    workspaces: Vec<WorkspaceKey>,
    _memory: MemoryLease,
}

fn prepare_snapshot_census(
    connection: &Connection,
    manifest: &NativeBootManifest,
    routes: &NativeProtectedSourceRoutesV1,
    budget: &mut WorkBudget,
) -> Result<SnapshotCensus, IndexError> {
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

    require_attached_event_claims_v1(connection, budget)?;

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
    let workspace_memory = budget
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
    Ok(SnapshotCensus {
        route: route.object,
        workspaces,
        _memory: workspace_memory,
    })
}

enum SnapshotRow<'a> {
    Operation(&'a [u8]),
    Claim(&'a [u8]),
    Frontier(NativeProtectedFrontierV1),
    Authority(NativeProtectedSourceAuthorityV1),
    Genesis(&'a CatalogView),
}

struct SnapshotSqlKey {
    bytes: Vec<u8>,
    payload_bytes: i64,
}

fn collect_sorted_keys(
    connection: &Connection,
    sql_query: &str,
    bindings: impl rusqlite::Params,
    mut remaining_bytes: u64,
    keys: &mut Vec<SnapshotSqlKey>,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let mut statement = connection
        .prepare_cached(sql_query)
        .map_err(|error| sql(&error))?;
    let mut rows = statement.query(bindings).map_err(|error| sql(&error))?;
    while let Some(row) = rows.next().map_err(|error| sql(&error))? {
        let key = bounded_blob(row, 0, 4096)?;
        let payload_bytes = row
            .get::<_, Option<i64>>(1)
            .map_err(|error| sql(&error))?
            .ok_or(IndexError::DigestConflict)?;
        charge_snapshot_row(&[key.len(), size_of::<i64>()], budget)?;
        remaining_bytes = remaining_bytes
            .checked_sub(u64::try_from(key.len()).map_err(|_| IndexError::ResourceLimited)?)
            .ok_or(IndexError::ResourceLimited)?;
        if keys.len() == keys.capacity() {
            return Err(IndexError::ResourceLimited);
        }
        keys.push(SnapshotSqlKey {
            bytes: key.to_vec(),
            payload_bytes,
        });
    }
    // The V1 snapshot orders these rows by their serialized SQLite BLOB key.
    // Sort only those keys before reading any row payload.
    keys.sort_unstable_by(|left, right| left.bytes.cmp(&right.bytes));
    Ok(())
}

fn scan_snapshot_rows(
    connection: &Connection,
    routes: &NativeProtectedSourceRoutesV1,
    workspaces: &[WorkspaceKey],
    key_backing: SnapshotKeyBacking,
    budget: &mut WorkBudget,
    mut visit: impl FnMut(SnapshotRow<'_>) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    let key_bytes = key_backing
        .count
        .checked_mul(size_of::<SnapshotSqlKey>())
        .and_then(|bytes| u64::try_from(bytes).ok())
        .and_then(|bytes| bytes.checked_add(key_backing.bytes))
        .ok_or(IndexError::ResourceLimited)?;
    let _key_memory = budget
        .reserve_memory(key_bytes, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let mut keys = Vec::new();
    keys.try_reserve_exact(key_backing.count)
        .map_err(|_| IndexError::ResourceLimited)?;
    if keys.capacity() > key_backing.count {
        return Err(IndexError::ResourceLimited);
    }
    let mut operation_row = connection
        .prepare(
            "SELECT epoch,
                    CASE WHEN typeof(payload)='blob' AND length(payload)=?3 THEN payload END,
                    CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
               FROM native_operation_v1 WHERE operation=?1 AND workspace=?2",
        )
        .map_err(|error| sql(&error))?;
    let mut claim_row = connection
        .prepare(
            "SELECT CASE WHEN typeof(operation)='blob' AND length(operation) BETWEEN 1 AND 4096 THEN operation END,
                    CASE WHEN typeof(payload)='blob' AND length(payload)=?2 THEN payload END,
                    CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
               FROM native_event_claim_v1 WHERE identity=?1",
        )
        .map_err(|error| sql(&error))?;
    let max_operation_bytes = i64::try_from(MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1)
        .map_err(|_| IndexError::ResourceLimited)?;
    for &workspace in workspaces {
        let scope = workspace_key(workspace)?;
        collect_sorted_keys(
            connection,
            "SELECT CASE WHEN typeof(operation)='blob' AND length(operation) BETWEEN 1 AND 4096 THEN operation END,
                    CASE WHEN typeof(payload)='blob' AND length(payload) BETWEEN 1 AND ?2 THEN length(payload) END
               FROM native_operation_v1 INDEXED BY native_operation_workspace_idx_v1 WHERE workspace=?1",
            params![&scope, max_operation_bytes],
            key_backing.bytes,
            &mut keys,
            budget,
        )?;
        for SnapshotSqlKey {
            bytes: key_bytes,
            payload_bytes,
        } in keys.drain(..)
        {
            let _payload_memory = budget
                .reserve_memory(uint(payload_bytes)?, MemoryLifetime::Temporary)?
                .ok_or(IndexError::ResourceLimited)?;
            let mut rows = operation_row
                .query(params![&key_bytes, &scope, payload_bytes])
                .map_err(|error| sql(&error))?;
            let row = rows
                .next()
                .map_err(|error| sql(&error))?
                .ok_or(IndexError::SnapshotUnavailable)?;
            let epoch: i64 = row.get(0).map_err(|error| sql(&error))?;
            let payload = bounded_blob(row, 1, MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1)?;
            let digest = bounded_blob(row, 2, 32)?;
            charge_snapshot_row(
                &[
                    key_bytes.len(),
                    payload.len(),
                    digest.len(),
                    size_of::<i64>(),
                ],
                budget,
            )?;
            let key: OperationKey = decode(&key_bytes)?;
            if key.workspace != workspace || operation_key(key)? != key_bytes {
                return Err(IndexError::DigestConflict);
            }
            let _row_memory = operation_scratch_v1(payload, budget)?;
            let original =
                decode_stored_operation(connection, key, &scope, epoch, payload, digest)?;
            if !canonical_row_v1(payload, &original)? {
                return Err(IndexError::DigestConflict);
            }
            if !matches!(
                original.status,
                OperationStatus::Published(_) | OperationStatus::Refused(_)
            ) {
                return Err(IndexError::Conflict);
            }
            visit(SnapshotRow::Operation(payload))?;
        }
        collect_sorted_keys(
            connection,
            "SELECT CASE WHEN typeof(c.identity)='blob' AND length(c.identity) BETWEEN 1 AND 4096 THEN c.identity END,
                    CASE WHEN typeof(c.payload)='blob' AND length(c.payload) BETWEEN 1 AND 4096 THEN length(c.payload) END
               FROM native_operation_v1 AS o INDEXED BY native_operation_workspace_idx_v1
               CROSS JOIN native_event_claim_v1 AS c INDEXED BY native_event_claim_operation_idx_v1
              WHERE o.workspace=?1 AND c.operation=o.operation",
            params![&scope],
            key_backing.bytes,
            &mut keys,
            budget,
        )?;
        for SnapshotSqlKey {
            bytes: identity,
            payload_bytes,
        } in keys.drain(..)
        {
            let _payload_memory = budget
                .reserve_memory(uint(payload_bytes)?, MemoryLifetime::Temporary)?
                .ok_or(IndexError::ResourceLimited)?;
            let mut rows = claim_row
                .query(params![&identity, payload_bytes])
                .map_err(|error| sql(&error))?;
            let row = rows
                .next()
                .map_err(|error| sql(&error))?
                .ok_or(IndexError::SnapshotUnavailable)?;
            let owner = bounded_blob(row, 0, 4096)?;
            let payload = bounded_blob(row, 1, 4096)?;
            let digest = bounded_blob(row, 2, 32)?;
            charge_snapshot_row(
                &[identity.len(), owner.len(), payload.len(), digest.len()],
                budget,
            )?;
            check_digest(b"qidx.native.eventclaim.v1", payload, digest)?;
            let claim: super::super::EventClaim = decode(payload)?;
            if super::super::event_identity(claim.operation, claim.event)? != identity
                || operation_key(claim.operation)?.as_slice() != owner
                || claim.operation.workspace != workspace
                || !canonical_row_v1(payload, &claim)?
            {
                return Err(IndexError::DigestConflict);
            }
            visit(SnapshotRow::Claim(payload))?;
        }

        let mut statement = connection.prepare(
                    "SELECT CASE WHEN typeof(producer)='blob' AND length(producer)=16 THEN producer END,
                            CASE WHEN typeof(stream)='blob' AND length(stream)=16 THEN stream END,
                            stream_epoch,sequence,
                            CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
                       FROM native_frontier_v1 WHERE workspace=?1 ORDER BY producer,stream"
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
            visit(SnapshotRow::Frontier(NativeProtectedFrontierV1 {
                workspace,
                producer: quanta_index_core::index::ProducerId(producer),
                stream: quanta_index_core::index::StreamId(stream),
                stream_epoch: uint(epoch)?,
                sequence: uint(sequence)?,
            }))?;
        }
        drop(rows);
        drop(statement);

        let mut statement = connection.prepare(
                    "SELECT workspace_epoch,
                            CASE WHEN typeof(producer)='blob' AND length(producer)=16 THEN producer END,
                            source_epoch,
                            CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
                       FROM native_source_authority_v1 WHERE workspace=?1 ORDER BY workspace_epoch,producer"
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
            visit(SnapshotRow::Authority(NativeProtectedSourceAuthorityV1 {
                workspace,
                workspace_epoch: uint(epoch)?,
                producer: quanta_index_core::index::ProducerId(producer),
                source_epoch: uint(source_epoch)?,
            }))?;
        }
        drop(rows);
        drop(statement);
    }
    for head in &routes.repomap_heads {
        if head.source_operation.is_none() {
            let genesis = boot_manifest::load_publication(
                connection,
                head.source_publication.identity,
                budget,
            )?;
            if genesis != head.source_publication || genesis.identity.sequence != 0 {
                return Err(IndexError::DigestConflict);
            }
            visit(SnapshotRow::Genesis(&genesis))?;
        }
    }
    Ok(())
}

struct SnapshotRowCounts {
    counts: [usize; 4],
    genesis_count: usize,
    raw_bytes: u64,
    keys: SnapshotKeyBacking,
}

#[derive(Clone, Copy, Default)]
struct SnapshotKeyBacking {
    count: usize,
    bytes: u64,
}

fn measure_snapshot_rows(
    connection: &Connection,
    routes: &NativeProtectedSourceRoutesV1,
    workspaces: &[WorkspaceKey],
    budget: &mut WorkBudget,
) -> Result<SnapshotRowCounts, IndexError> {
    let genesis_count = routes
        .repomap_heads
        .iter()
        .filter(|head| head.source_operation.is_none())
        .count();
    let mut remaining_rows = MAX_NATIVE_PROTECTED_SNAPSHOT_ROWS_V1
        .checked_sub(genesis_count)
        .ok_or(IndexError::ResourceLimited)?;
    let mut counts = [0usize; 4];
    let mut raw_bytes = 0_u64;
    let mut keys = SnapshotKeyBacking::default();
    let maximum_bytes = u64::try_from(MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1)
        .map_err(|_| IndexError::ResourceLimited)?;
    for workspace in workspaces {
        let workspace = workspace_key(*workspace)?;
        for (total, (query, sorts_keys)) in counts.iter_mut().zip([
            (
                "SELECT count(*),coalesce(sum(bytes),0),coalesce(sum(key_bytes),0) FROM
                 (SELECT octet_length(payload) AS bytes,
                         CASE WHEN typeof(operation)='blob' AND length(operation) BETWEEN 1 AND 4096
                              THEN length(operation) END AS key_bytes
                    FROM native_operation_v1 INDEXED BY native_operation_workspace_idx_v1
                   WHERE workspace=?1 LIMIT ?2)",
                true,
            ),
            (
                "SELECT count(*),coalesce(sum(bytes),0),coalesce(sum(key_bytes),0) FROM
                 (SELECT octet_length(c.payload) AS bytes,
                         CASE WHEN typeof(c.identity)='blob' AND length(c.identity) BETWEEN 1 AND 4096
                              THEN length(c.identity) END AS key_bytes
                    FROM native_operation_v1 AS o INDEXED BY native_operation_workspace_idx_v1
                    CROSS JOIN native_event_claim_v1 AS c INDEXED BY native_event_claim_operation_idx_v1
                   WHERE o.workspace=?1 AND c.operation=o.operation LIMIT ?2)",
                true,
            ),
            (
                "SELECT count(*),0,0 FROM
                 (SELECT 1 FROM native_frontier_v1 WHERE workspace=?1 LIMIT ?2)",
                false,
            ),
            (
                "SELECT count(*),0,0 FROM
                 (SELECT 1 FROM native_source_authority_v1 WHERE workspace=?1 LIMIT ?2)",
                false,
            ),
        ]) {
            let limit = remaining_rows
                .checked_add(1)
                .and_then(|limit| i64::try_from(limit).ok())
                .ok_or(IndexError::ResourceLimited)?;
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            let (count, bytes, key_bytes): (i64, i64, i64) = connection
                .query_row(query, params![&workspace, limit], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .map_err(|error| sql(&error))?;
            let count = usize::try_from(uint(count)?).map_err(|_| IndexError::ResourceLimited)?;
            remaining_rows = remaining_rows
                .checked_sub(count)
                .ok_or(IndexError::ResourceLimited)?;
            *total = total.checked_add(count).ok_or(IndexError::ResourceLimited)?;
            if sorts_keys {
                keys.count = keys.count.max(count);
                keys.bytes = keys.bytes.max(uint(key_bytes)?);
            }
            raw_bytes = raw_bytes
                .checked_add(uint(bytes)?)
                .filter(|bytes| *bytes <= maximum_bytes)
                .ok_or(IndexError::ResourceLimited)?;
        }
    }
    if matches!(counts, [0, ..]) {
        return Err(IndexError::SnapshotUnavailable);
    }
    Ok(SnapshotRowCounts {
        counts,
        genesis_count,
        raw_bytes,
        keys,
    })
}

#[cfg(test)]
mod legacy_inline_witness_tests {
    use super::*;
    use quanta_index_contract::index::Digest;
    use quanta_index_core::index::WorkLimits;
    use std::time::{Duration, Instant};

    #[test]
    fn predecessor_witness_binds_old_inline_identity_without_changing_new_export()
    -> Result<(), IndexError> {
        let route = ObjectRef(Digest([41; 32]));
        let snapshot = NativeProtectedNativeSnapshotV1 {
            wire_version: NativeProtectedNativeSnapshotV1::WIRE_VERSION,
            source_root_nonce: Digest([42; 32]),
            source_routes: route,
            operation_rows: vec![vec![255; 4_300_000]],
            genesis_views: Vec::new(),
            frontiers: Vec::new(),
            source_authorities: Vec::new(),
            event_claims: Vec::new(),
            roots: vec![route],
        };
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 0,
                writes: 0,
                bytes: 100_000_000,
                mutations: 0,
            },
            Instant::now() + Duration::from_secs(30),
        );
        let old = snapshot
            .legacy_inline_object_ref_with_work_v1(&mut || budget.checkpoint())
            .map_err(|cause| cause.into_error(|_| IndexError::DigestConflict))?;
        let new = snapshot
            .object_ref_v1()
            .map_err(|_| IndexError::DigestConflict)?;
        assert_ne!(old, new);
        let origin = NativeProtectedNativeOriginV1 {
            source_root_nonce: snapshot.source_root_nonce,
            source_routes: route,
            snapshot: old,
        };
        let witness = NativeProtectedSnapshotWitnessV1::from_snapshot_for_origin(
            &snapshot,
            Some(&origin),
            &mut budget,
        )?;
        assert_eq!(witness.origin_v1(), &origin);
        assert_eq!(witness.object_references_v1().collect::<Vec<_>>(), [old]);
        let wrong = NativeProtectedNativeOriginV1 {
            snapshot: ObjectRef(Digest([43; 32])),
            ..origin
        };
        assert!(matches!(
            NativeProtectedSnapshotWitnessV1::from_snapshot_for_origin(
                &snapshot,
                Some(&wrong),
                &mut budget,
            ),
            Err(IndexError::DigestConflict)
        ));
        Ok(())
    }
}
