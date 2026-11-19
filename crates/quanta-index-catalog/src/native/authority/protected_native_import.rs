//! Atomic Catalog import of authenticated protected Native heads.
//!
//! The held previous root supplies exact operation and ledger rows. A route
//! object alone describes expected heads but cannot recreate source events.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

use quanta_index_contract::index::{
    NativeProtectedNativeSnapshotV1, NativeProtectedSourceRoutesV1,
};
use quanta_index_core::index::{
    CatalogView, ImmutableOperation, IndexError, IngressRef, MemoryLease, MemoryLifetime,
    OperationKey, OperationStatus, ProducerId, StreamId, ViewRef, WorkBudget, WorkUsage,
    WorkspaceKey,
};
use rusqlite::{Connection, OptionalExtension as _, params};

use super::{
    ClaimIdentity, EventClaim, EventClaimState, OperationRow, SqliteCatalog, begin, check_digest,
    claim_identity, confirmed_ingress_object, decode, digest, encode, ensure_roots, event_identity,
    exactly_one, fields_digest, int, operation_key, publication_result, sql, workspace_key,
    write_event_claim, write_operation_payload, write_view,
};

struct ExpectedProtectedRowsV1<'a> {
    operations: BTreeMap<OperationKey, (ImmutableOperation, &'a CatalogView)>,
    views: BTreeMap<ViewRef, &'a CatalogView>,
    current: BTreeMap<WorkspaceKey, &'a CatalogView>,
    genesis: BTreeMap<ViewRef, &'a CatalogView>,
}

impl<'a> ExpectedProtectedRowsV1<'a> {
    fn from_routes(
        primary: OperationKey,
        routes: &'a NativeProtectedSourceRoutesV1,
    ) -> Result<Self, IndexError> {
        routes.validate().map_err(|_| IndexError::DigestConflict)?;
        let mut expected = Self {
            operations: BTreeMap::new(),
            views: BTreeMap::new(),
            current: BTreeMap::new(),
            genesis: BTreeMap::new(),
        };
        for head in &routes.heads {
            expected.add_operation(head.source_operation, &head.source_publication)?;
            expected.add_operation(head.operation, &head.publication)?;
            expected.add_current(primary, &head.publication)?;
        }
        for head in &routes.repomap_heads {
            match head.source_operation {
                Some(operation) => {
                    expected.add_operation(operation, &head.source_publication)?;
                }
                None => {
                    expected.add_view(primary, &head.source_publication)?;
                    let _prior = expected
                        .genesis
                        .insert(head.source_publication.identity, &head.source_publication);
                }
            }
            expected.add_operation(head.operation, &head.publication)?;
            expected.add_current(primary, &head.publication)?;
        }
        if expected.current.is_empty() {
            return Err(IndexError::DigestConflict);
        }
        Ok(expected)
    }

    fn add_operation(
        &mut self,
        operation: ImmutableOperation,
        view: &'a CatalogView,
    ) -> Result<(), IndexError> {
        if operation.key.workspace != view.identity.workspace
            || operation.key.epoch != view.identity.epoch
        {
            return Err(IndexError::DigestConflict);
        }
        if self
            .operations
            .insert(operation.key, (operation, view))
            .is_some_and(|prior| prior != (operation, view))
        {
            return Err(IndexError::DigestConflict);
        }
        Ok(())
    }

    fn add_view(&mut self, primary: OperationKey, view: &'a CatalogView) -> Result<(), IndexError> {
        if view.identity.workspace == primary.workspace
            || view.identity.digest
                != view
                    .identity_digest()
                    .map_err(|_| IndexError::DigestConflict)?
        {
            return Err(IndexError::DigestConflict);
        }
        if self
            .views
            .insert(view.identity, view)
            .is_some_and(|prior| prior != view)
        {
            return Err(IndexError::DigestConflict);
        }
        Ok(())
    }

    fn add_current(
        &mut self,
        primary: OperationKey,
        view: &'a CatalogView,
    ) -> Result<(), IndexError> {
        self.add_view(primary, view)?;
        if self
            .current
            .insert(view.identity.workspace, view)
            .is_some_and(|prior| prior != view)
        {
            return Err(IndexError::DigestConflict);
        }
        Ok(())
    }
}

struct OperationInput<'a> {
    key: OperationKey,
    payload: &'a [u8],
    expected_matches: bool,
}

pub(super) fn canonical_row_v1(
    payload: &[u8],
    row: &impl serde::Serialize,
) -> Result<bool, IndexError> {
    struct Compare<'a> {
        remaining: &'a [u8],
        matches: bool,
    }
    impl io::Write for Compare<'_> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.matches {
                if let Some(remaining) = self.remaining.strip_prefix(bytes) {
                    self.remaining = remaining;
                } else {
                    self.matches = false;
                }
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut comparison = Compare {
        remaining: payload,
        matches: true,
    };
    serde_json::to_writer(&mut comparison, row)
        .map_err(|cause| IndexError::Storage(format!("encode native catalog row: {cause}")))?;
    Ok(comparison.matches && comparison.remaining.is_empty())
}

pub(super) fn operation_scratch_v1(
    payload: &[u8],
    budget: &mut WorkBudget,
) -> Result<MemoryLease, IndexError> {
    let bytes = u64::try_from(payload.len()).map_err(|_| IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes: bytes.checked_mul(3).ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    // The snapshot already pays for the input. The publication metadata
    // reader admits four lengths including its owned input; here the
    // remaining three cover one decoded row and encode/growth scratch.
    let backing = bytes
        .checked_mul(3)
        .and_then(|bytes| bytes.checked_add(8192))
        .ok_or(IndexError::ResourceLimited)?;
    budget
        .reserve_memory(backing, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)
}

fn validate_operation_rows_v1<'a>(
    snapshot: &'a NativeProtectedNativeSnapshotV1,
    expected: &ExpectedProtectedRowsV1<'_>,
    budget: &mut WorkBudget,
) -> Result<Vec<OperationInput<'a>>, IndexError> {
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    rows.try_reserve_exact(snapshot.operation_rows.len())
        .map_err(|_| IndexError::ResourceLimited)?;
    if rows.capacity() > snapshot.operation_rows.len() {
        return Err(IndexError::ResourceLimited);
    }
    for bytes in &snapshot.operation_rows {
        let _row_memory = operation_scratch_v1(bytes, budget)?;
        let row: OperationRow = decode(bytes)?;
        if !canonical_row_v1(bytes, &row)?
            || !expected.current.contains_key(&row.immutable.key.workspace)
            || row.fence == 0
            || !matches!(
                &row.status,
                OperationStatus::Published(_) | OperationStatus::Refused(_)
            )
        {
            return Err(IndexError::DigestConflict);
        }
        publication_result::validate_operation_result(&row)?;
        if !seen.insert(row.immutable.key) {
            return Err(IndexError::DigestConflict);
        }
        let expected_matches =
            expected
                .operations
                .get(&row.immutable.key)
                .is_some_and(|(operation, view)| {
                    row.immutable == *operation
                        && row.status == OperationStatus::Published(view.identity)
                        && row.published_view.as_ref() == Some(*view)
                });
        rows.push(OperationInput {
            key: row.immutable.key,
            payload: bytes,
            expected_matches,
        });
    }
    rows.sort_unstable_by_key(|row| row.key);
    for key in expected.operations.keys() {
        let index = rows
            .binary_search_by_key(key, |row| row.key)
            .map_err(|_| IndexError::DigestConflict)?;
        if !rows.get(index).is_some_and(|row| row.expected_matches) {
            return Err(IndexError::DigestConflict);
        }
    }
    Ok(rows)
}

pub(super) fn validate_claims_v1(
    snapshot: &NativeProtectedNativeSnapshotV1,
    budget: &mut WorkBudget,
) -> Result<(MemoryLease, Vec<EventClaim>), IndexError> {
    type ClaimIndex = (ClaimIdentity, usize, bool);
    let count = snapshot.event_claims.len();
    let reserve_entries = |entry_size: usize| {
        let bytes = count
            .checked_mul(entry_size)
            .and_then(|bytes| bytes.checked_add(4096))
            .and_then(|bytes| u64::try_from(bytes).ok())
            .ok_or(IndexError::ResourceLimited)?;
        budget
            .reserve_memory(bytes, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)
    };
    let claim_memory = reserve_entries(core::mem::size_of::<EventClaim>())?;
    let _index_memory = reserve_entries(core::mem::size_of::<ClaimIndex>())?;
    let mut claims = Vec::new();
    let mut index: Vec<ClaimIndex> = Vec::new();
    claims
        .try_reserve_exact(count)
        .map_err(|_| IndexError::ResourceLimited)?;
    index
        .try_reserve_exact(count)
        .map_err(|_| IndexError::ResourceLimited)?;
    if claims.capacity() > count || index.capacity() > count {
        return Err(IndexError::ResourceLimited);
    }
    for bytes in &snapshot.event_claims {
        budget.checkpoint()?;
        let claim: EventClaim = decode(bytes)?;
        if !canonical_row_v1(bytes, &claim)? {
            return Err(IndexError::DigestConflict);
        }
        index.push((
            claim_identity(claim.operation, claim.event),
            claims.len(),
            false,
        ));
        claims.push(claim);
    }
    index.sort_unstable_by_key(|entry| entry.0);
    if index.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(IndexError::DigestConflict);
    }
    for payload in &snapshot.operation_rows {
        let _row_memory = operation_scratch_v1(payload, budget)?;
        let row: OperationRow = decode(payload)?;
        let key = row.immutable.key;
        let published = matches!(row.status, OperationStatus::Published(_));
        for (ordinal, event) in row.events.iter().copied().enumerate() {
            if ordinal % 256 == 0 {
                budget.checkpoint()?;
            }
            let found = index
                .binary_search_by_key(&claim_identity(key, event), |entry| entry.0)
                .ok()
                .and_then(|found| index.get_mut(found));
            let matched = if let Some((_, position, matched)) = found {
                let claim = claims.get(*position).ok_or(IndexError::DigestConflict)?;
                if claim.operation == key && claim.event == event {
                    let valid_state = if published {
                        claim.state == EventClaimState::Committed
                    } else {
                        claim.state == EventClaimState::Refused
                    };
                    if !valid_state {
                        return Err(IndexError::DigestConflict);
                    }
                    *matched = true;
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if published && !matched {
                return Err(IndexError::DigestConflict);
            }
        }
    }
    if index.iter().any(|entry| !entry.2) {
        return Err(IndexError::DigestConflict);
    }
    Ok((claim_memory, claims))
}

fn count_workspace_v1(
    tx: &Connection,
    table: &'static str,
    workspace: WorkspaceKey,
) -> Result<u64, IndexError> {
    let sql_v1 = match table {
        "native_view_v1" => "SELECT count(*) FROM native_view_v1 WHERE workspace=?1",
        "native_operation_v1" => "SELECT count(*) FROM native_operation_v1 WHERE workspace=?1",
        "native_frontier_v1" => "SELECT count(*) FROM native_frontier_v1 WHERE workspace=?1",
        "native_source_authority_v1" => {
            "SELECT count(*) FROM native_source_authority_v1 WHERE workspace=?1"
        }
        "native_event_claim_v1" => {
            "SELECT count(*) FROM native_event_claim_v1 c JOIN native_operation_v1 o ON c.operation=o.operation WHERE o.workspace=?1"
        }
        _ => {
            return Err(IndexError::InvalidInput(
                "unknown protected workspace table".into(),
            ));
        }
    };
    let count: i64 = tx
        .query_row(sql_v1, params![workspace_key(workspace)?], |row| row.get(0))
        .map_err(|error| sql(&error))?;
    u64::try_from(count).map_err(|_| IndexError::Storage("negative protected row count".into()))
}

/// Refuse claims that the workspace-scoped joins would otherwise omit.
/// Foreign keys may have been disabled by an earlier writer of the file.
pub(super) fn require_attached_event_claims_v1(
    connection: &Connection,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let orphan_claim: bool = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM native_event_claim_v1 AS claim
                  INDEXED BY native_event_claim_operation_idx_v1
                  WHERE CASE WHEN typeof(claim.operation)='blob'
                             AND octet_length(claim.operation) BETWEEN 1 AND 4096
                        THEN NOT EXISTS(
                            SELECT 1 FROM native_operation_v1 AS operation
                             WHERE operation.operation=claim.operation
                        ) ELSE 1 END
             )",
            [],
            |row| row.get(0),
        )
        .map_err(|error| {
            if error.sqlite_error_code() == Some(rusqlite::ErrorCode::OperationInterrupted) {
                return budget.checkpoint().err().unwrap_or_else(|| sql(&error));
            }
            sql(&error)
        })?;
    budget.checkpoint()?;
    if orphan_claim {
        return Err(IndexError::DigestConflict);
    }
    Ok(())
}

#[derive(Default)]
struct WorkspaceRowCounts {
    views: u64,
    operations: u64,
    claims: u64,
    frontiers: u64,
    authorities: u64,
}

fn expected_workspace_counts(
    expected: &ExpectedProtectedRowsV1<'_>,
    operations: &[OperationInput<'_>],
    claims: &[EventClaim],
    snapshot: &NativeProtectedNativeSnapshotV1,
) -> BTreeMap<WorkspaceKey, WorkspaceRowCounts> {
    let mut counts = BTreeMap::<WorkspaceKey, WorkspaceRowCounts>::new();
    for view in expected.views.values() {
        counts.entry(view.identity.workspace).or_default().views += 1;
    }
    for operation in operations {
        counts
            .entry(operation.key.workspace)
            .or_default()
            .operations += 1;
    }
    for claim in claims {
        counts.entry(claim.operation.workspace).or_default().claims += 1;
    }
    for row in &snapshot.frontiers {
        counts.entry(row.workspace).or_default().frontiers += 1;
    }
    for row in &snapshot.source_authorities {
        counts.entry(row.workspace).or_default().authorities += 1;
    }
    counts
}

fn load_existing<T>(
    tx: &Connection,
    query: &str,
    args: impl rusqlite::Params,
    visit: impl FnOnce(&rusqlite::Row<'_>) -> Result<T, IndexError>,
) -> Result<Option<T>, IndexError> {
    tx.query_row(query, args, |row| Ok(visit(row)))
        .optional()
        .map_err(|error| sql(&error))?
        .transpose()
}

fn blob<'a>(row: &'a rusqlite::Row<'_>, index: usize) -> Result<&'a [u8], IndexError> {
    row.get_ref(index)
        .map_err(|error| sql(&error))?
        .as_blob()
        .map_err(|_| IndexError::Storage("native catalog row column must be a BLOB".into()))
}

fn existing_payload_matches(
    row: &rusqlite::Row<'_>,
    expected: &[u8],
    domain: &[u8],
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    // All three exact-read queries bound payload and digest inside SQLite
    // before placing them last, after two fixed binding columns.
    let payload = blob(row, 2)?;
    let row_digest = blob(row, 3)?;
    let bytes = payload
        .len()
        .checked_add(row_digest.len())
        .and_then(|len| u64::try_from(len).ok())
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        reads: 1,
        bytes,
        ..WorkUsage::default()
    })?;
    check_digest(domain, payload, row_digest)?;
    Ok(payload == expected)
}

fn compare_existing_operation(
    tx: &Connection,
    operation: &OperationInput<'_>,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let workspace = workspace_key(operation.key.workspace)?;
    let epoch = int(operation.key.epoch)?;
    let payload_bytes =
        u64::try_from(operation.payload.len()).map_err(|_| IndexError::ResourceLimited)?;
    let _stored_payload = budget
        .reserve_memory(payload_bytes, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let actual = load_existing(
        tx,
        "SELECT CASE WHEN typeof(workspace)!='blob' THEN 0
                     WHEN length(workspace) != ?3 THEN X'' ELSE workspace END,
                epoch,
                CASE WHEN typeof(payload)!='blob' THEN 0
                     WHEN length(payload) != ?2 THEN X'' ELSE payload END,
                CASE WHEN typeof(row_digest)!='blob' THEN 0
                     WHEN length(row_digest) != 32 THEN X'' ELSE row_digest END
         FROM native_operation_v1 WHERE operation=?1",
        params![
            operation_key(operation.key)?,
            i64::try_from(payload_bytes).map_err(|_| IndexError::ResourceLimited)?,
            i64::try_from(workspace.len()).map_err(|_| IndexError::ResourceLimited)?,
        ],
        |row| {
            let matches = existing_payload_matches(
                row,
                operation.payload,
                b"qidx.native.operation.v1",
                budget,
            )?;
            Ok(matches
                && blob(row, 0)? == workspace.as_slice()
                && row.get::<_, i64>(1).map_err(|error| sql(&error))? == epoch)
        },
    )?;
    if actual != Some(true) {
        return Err(IndexError::DigestConflict);
    }
    Ok(())
}

fn validate_existing_v1(
    tx: &Connection,
    expected: &ExpectedProtectedRowsV1<'_>,
    operations: &[OperationInput<'_>],
    claims: &[EventClaim],
    snapshot: &NativeProtectedNativeSnapshotV1,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    let mut existing = 0;
    for workspace in expected.current.keys() {
        let present: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM native_workspace_v1 WHERE workspace=?1)",
                params![workspace_key(*workspace)?],
                |row| row.get(0),
            )
            .map_err(|error| sql(&error))?;
        if present {
            existing += 1;
        }
    }
    if existing == 0 {
        for workspace in expected.current.keys() {
            if count_workspace_v1(tx, "native_view_v1", *workspace)? != 0
                || count_workspace_v1(tx, "native_operation_v1", *workspace)? != 0
                || count_workspace_v1(tx, "native_frontier_v1", *workspace)? != 0
                || count_workspace_v1(tx, "native_source_authority_v1", *workspace)? != 0
            {
                return Err(IndexError::Conflict);
            }
        }
        return Ok(false);
    }
    if existing != expected.current.len() {
        return Err(IndexError::Conflict);
    }
    let counts = expected_workspace_counts(expected, operations, claims, snapshot);
    for (workspace, terminal) in &expected.current {
        let expected_bytes = encode(*terminal)?;
        let epoch = int(terminal.identity.epoch)?;
        let sequence = int(terminal.identity.sequence)?;
        let actual = load_existing(
            tx,
            "SELECT epoch,sequence,
                    CASE WHEN typeof(payload)!='blob' THEN 0
                         WHEN length(payload) != ?2 THEN X'' ELSE payload END,
                    CASE WHEN typeof(row_digest)!='blob' THEN 0
                         WHEN length(row_digest) != 32 THEN X'' ELSE row_digest END
             FROM native_workspace_v1 WHERE workspace=?1",
            params![
                workspace_key(*workspace)?,
                i64::try_from(expected_bytes.len()).map_err(|_| IndexError::ResourceLimited)?
            ],
            |row| {
                let matches =
                    existing_payload_matches(row, &expected_bytes, b"qidx.native.view.v1", budget)?;
                Ok(matches
                    && row.get::<_, i64>(0).map_err(|error| sql(&error))? == epoch
                    && row.get::<_, i64>(1).map_err(|error| sql(&error))? == sequence)
            },
        )?;
        if actual != Some(true) {
            return Err(IndexError::Conflict);
        }
        let count = counts.get(workspace).ok_or(IndexError::DigestConflict)?;
        for (table, expected) in [
            ("native_view_v1", count.views),
            ("native_operation_v1", count.operations),
            ("native_event_claim_v1", count.claims),
            ("native_frontier_v1", count.frontiers),
            ("native_source_authority_v1", count.authorities),
        ] {
            if count_workspace_v1(tx, table, *workspace)? != expected {
                return Err(IndexError::Conflict);
            }
        }
    }
    for (identity, view) in &expected.views {
        if &super::boot_manifest::load_publication(tx, *identity, budget)? != *view {
            return Err(IndexError::DigestConflict);
        }
    }
    for operation in operations {
        compare_existing_operation(tx, operation, budget)?;
    }
    for claim in claims {
        let owner = operation_key(claim.operation)?;
        let identity = event_identity(claim.operation, claim.event)?;
        let payload = encode(claim)?;
        let actual = load_existing(
            tx,
            "SELECT CASE WHEN typeof(operation)!='blob' THEN 0
                         WHEN length(operation) != ?3 THEN X'' ELSE operation END,
                    identity,
                    CASE WHEN typeof(payload)!='blob' THEN 0
                         WHEN length(payload) != ?2 THEN X'' ELSE payload END,
                    CASE WHEN typeof(row_digest)!='blob' THEN 0
                         WHEN length(row_digest) != 32 THEN X'' ELSE row_digest END
             FROM native_event_claim_v1 WHERE identity=?1",
            params![
                &identity,
                i64::try_from(payload.len()).map_err(|_| IndexError::ResourceLimited)?,
                i64::try_from(owner.len()).map_err(|_| IndexError::ResourceLimited)?,
            ],
            |row| {
                let matches =
                    existing_payload_matches(row, &payload, b"qidx.native.eventclaim.v1", budget)?;
                Ok(matches
                    && blob(row, 0)? == owner.as_slice()
                    && blob(row, 1)? == identity.as_slice())
            },
        )?;
        if actual != Some(true) {
            return Err(IndexError::DigestConflict);
        }
    }
    for row in &snapshot.frontiers {
        let workspace = workspace_key(row.workspace)?;
        let producer = row.producer.0.to_vec();
        let stream = row.stream.0.to_vec();
        let epoch = int(row.stream_epoch)?;
        let sequence = int(row.sequence)?;
        let expected_digest = fields_digest(
            b"qidx.native.frontier.v1",
            &(&workspace, &producer, &stream, epoch, sequence),
        )?;
        let actual = load_existing(
            tx,
            "SELECT stream_epoch,sequence,
                    CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32
                         THEN row_digest ELSE X'' END
             FROM native_frontier_v1 WHERE workspace=?1 AND producer=?2 AND stream=?3",
            params![&workspace, &producer, &stream],
            |record| {
                Ok(
                    record.get::<_, i64>(0).map_err(|error| sql(&error))? == epoch
                        && record.get::<_, i64>(1).map_err(|error| sql(&error))? == sequence
                        && blob(record, 2)? == expected_digest.as_slice(),
                )
            },
        )?;
        if actual != Some(true) {
            return Err(IndexError::DigestConflict);
        }
    }
    for row in &snapshot.source_authorities {
        let workspace = workspace_key(row.workspace)?;
        let producer = row.producer.0.to_vec();
        let workspace_epoch = int(row.workspace_epoch)?;
        let source_epoch = int(row.source_epoch)?;
        let expected_digest = fields_digest(
            b"qidx.native.sourceauthority.v1",
            &(&workspace, workspace_epoch, &producer, source_epoch),
        )?;
        let actual = load_existing(
            tx,
            "SELECT source_epoch,
                    CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32
                         THEN row_digest ELSE X'' END
             FROM native_source_authority_v1 WHERE workspace=?1 AND workspace_epoch=?2 AND producer=?3",
            params![&workspace, workspace_epoch, &producer],
            |record| {
                Ok(record.get::<_, i64>(0).map_err(|error| sql(&error))? == source_epoch
                    && blob(record, 1)? == expected_digest.as_slice())
            },
        )?;
        if actual != Some(true) {
            return Err(IndexError::DigestConflict);
        }
    }
    Ok(true)
}

fn index_backing_v1(
    snapshot: &NativeProtectedNativeSnapshotV1,
    routes: &NativeProtectedSourceRoutesV1,
) -> Option<u64> {
    fn slots<T>(count: usize) -> Option<usize> {
        count.checked_mul(core::mem::size_of::<T>())
    }

    fn tree<T>(count: usize) -> Option<usize> {
        // Account for vacant tree slots, internal child links, and small
        // roots. The row payloads are not stored in these nodes.
        let entry = core::mem::size_of::<T>().checked_add(3 * core::mem::size_of::<usize>())?;
        count.checked_mul(entry)?.checked_mul(3)?.checked_add(4096)
    }

    let heads = routes.heads.len().checked_add(routes.repomap_heads.len())?;
    let operations = heads.checked_mul(2)?;
    let views = heads.checked_mul(2)?;
    let roots = views.checked_mul(7)?.checked_add(operations)?;
    let bytes = [
        slots::<OperationInput<'_>>(snapshot.operation_rows.len())?,
        tree::<OperationKey>(snapshot.operation_rows.len())?,
        tree::<(OperationKey, ImmutableOperation, &CatalogView)>(operations)?,
        tree::<(ViewRef, &CatalogView)>(views)?,
        tree::<(WorkspaceKey, &CatalogView)>(heads)?,
        tree::<(WorkspaceKey, WorkspaceRowCounts)>(heads)?,
        tree::<(ViewRef, &CatalogView)>(snapshot.genesis_views.len().checked_mul(2)?)?,
        tree::<(WorkspaceKey, ProducerId, StreamId)>(snapshot.frontiers.len())?,
        tree::<(WorkspaceKey, u64, ProducerId)>(snapshot.source_authorities.len())?,
        tree::<quanta_index_core::index::ObjectRef>(roots)?,
        routes.children_backing_bytes().ok()?,
        // Fixed SQLite keys, bound view reads, and event claim encodes.
        65_536,
    ]
    .into_iter()
    .try_fold(0usize, usize::checked_add)?;
    u64::try_from(bytes).ok()
}

impl SqliteCatalog {
    /// Import exact Catalog history from a separately authenticated held Native root.
    /// This transaction does not issue a Source ACK or consume the staging ingress.
    pub fn restore_protected_native_heads_v1(
        &self,
        primary: OperationKey,
        ingress: IngressRef,
        snapshot: &NativeProtectedNativeSnapshotV1,
        source_routes: &NativeProtectedSourceRoutesV1,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        snapshot
            .validate_shape_v1()
            .map_err(|_| IndexError::DigestConflict)?;
        let bytes = snapshot
            .encoded_payload_len_with_work_v1(&mut || budget.checkpoint())
            .map_err(|cause| cause.into_error(|_| IndexError::ResourceLimited))?;
        let row_count = snapshot
            .operation_rows
            .len()
            .checked_add(snapshot.genesis_views.len())
            .and_then(|count| count.checked_add(snapshot.frontiers.len()))
            .and_then(|count| count.checked_add(snapshot.source_authorities.len()))
            .and_then(|count| count.checked_add(snapshot.event_claims.len()))
            .and_then(|count| count.checked_add(snapshot.roots.len()))
            .ok_or(IndexError::ResourceLimited)?;
        let row_count = u64::try_from(row_count).map_err(|_| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            reads: row_count
                .checked_mul(8)
                .and_then(|value| value.checked_add(64))
                .ok_or(IndexError::ResourceLimited)?,
            writes: row_count
                .checked_mul(4)
                .and_then(|value| value.checked_add(64))
                .ok_or(IndexError::ResourceLimited)?,
            mutations: row_count
                .checked_mul(4)
                .and_then(|value| value.checked_add(64))
                .ok_or(IndexError::ResourceLimited)?,
            bytes: u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let origin = source_routes
            .native_origin
            .as_ref()
            .ok_or(IndexError::DigestConflict)?;
        if origin.source_root_nonce != snapshot.source_root_nonce
            || origin.source_routes != snapshot.source_routes
            || origin.snapshot
                != snapshot
                    .physical_plan_with_work_v1(&mut || budget.checkpoint())
                    .map_err(|cause| cause.into_error(|_| IndexError::DigestConflict))?
                    .reference_v1()
            || ingress.operation.workspace != primary.workspace
            || ingress.operation.epoch != primary.epoch
            || ingress.operation == primary
        {
            return Err(IndexError::DigestConflict);
        }
        let indices_memory = budget
            .reserve_memory(
                index_backing_v1(snapshot, source_routes).ok_or(IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?
            .ok_or(IndexError::ResourceLimited)?;
        let expected = ExpectedProtectedRowsV1::from_routes(primary, source_routes)?;
        let operations = validate_operation_rows_v1(snapshot, &expected, budget)?;
        let (claim_memory, claims) = validate_claims_v1(snapshot, budget)?;
        let mut frontier_keys = BTreeSet::new();
        for row in &snapshot.frontiers {
            if !expected.current.contains_key(&row.workspace)
                || !frontier_keys.insert((row.workspace, row.producer, row.stream))
            {
                return Err(IndexError::DigestConflict);
            }
        }
        let mut authority_keys = BTreeSet::new();
        for row in &snapshot.source_authorities {
            if !expected.current.contains_key(&row.workspace)
                || !authority_keys.insert((row.workspace, row.workspace_epoch, row.producer))
            {
                return Err(IndexError::DigestConflict);
            }
        }
        let genesis = snapshot
            .genesis_views
            .iter()
            .map(|view| (view.identity, view))
            .collect::<BTreeMap<_, _>>();
        if genesis != expected.genesis || snapshot.genesis_views.len() != genesis.len() {
            return Err(IndexError::DigestConflict);
        }
        drop(genesis);
        drop(authority_keys);
        drop(frontier_keys);
        let mut guard = self.lock_native_budget(budget)?;
        let tx = begin(&mut guard)?;
        super::boot_manifest::require_open_protected_route_ingress(&tx, ingress, budget)?;
        if super::boot_manifest::protected_route_operation_exists(&tx, ingress.operation, budget)? {
            return Err(IndexError::Conflict);
        }
        super::boot_manifest::with_protected_operation(&tx, primary, budget, |row, _budget| {
            if !matches!(&row.status, OperationStatus::Published(_)) {
                return Err(IndexError::Conflict);
            }
            Ok(())
        })?;
        {
            let mut statement = tx
                .prepare(
                    "SELECT CASE WHEN typeof(workspace)!='blob' THEN 0
                                 WHEN length(workspace) > 4096 THEN X'' ELSE workspace END,
                            CASE WHEN typeof(workspace)='blob' THEN length(workspace) END
                     FROM native_workspace_v1 ORDER BY workspace",
                )
                .map_err(|error| sql(&error))?;
            let mut workspaces = statement.query([]).map_err(|error| sql(&error))?;
            while let Some(row) = workspaces.next().map_err(|error| sql(&error))? {
                budget.checkpoint()?;
                let encoded = blob(row, 0)?;
                let stored_bytes: i64 = row.get(1).map_err(|error| sql(&error))?;
                if stored_bytes > 4096 {
                    return Err(IndexError::Conflict);
                }
                let workspace: WorkspaceKey = decode(encoded)?;
                if encoded != workspace_key(workspace)?.as_slice()
                    || (workspace != primary.workspace
                        && !expected.current.contains_key(&workspace))
                {
                    return Err(IndexError::Conflict);
                }
            }
        }
        let new_route = {
            let children_bytes = source_routes
                .children_backing_bytes()
                .map_err(|_| IndexError::DigestConflict)?;
            let route_scratch = u64::try_from(children_bytes)
                .map_err(|_| IndexError::ResourceLimited)?
                .checked_add(4096)
                .ok_or(IndexError::ResourceLimited)?;
            let _route_memory = budget
                .reserve_memory(route_scratch, MemoryLifetime::Temporary)?
                .ok_or(IndexError::ResourceLimited)?;
            let (reference, bytes, _) = source_routes
                .object_identity_with_work_v1(&mut || budget.checkpoint())
                .map_err(|cause| cause.into_error(|_| IndexError::DigestConflict))?;
            budget.charge(WorkUsage {
                bytes: u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            reference
        };
        let mut required_roots = BTreeSet::new();
        for view in expected.views.values() {
            required_roots.extend(super::distinct_roots(view));
        }
        required_roots.extend(
            expected
                .operations
                .values()
                .map(|(operation, _)| operation.request),
        );
        for root in required_roots {
            if !confirmed_ingress_object(&tx, ingress.operation, root)? {
                return Err(IndexError::DigestConflict);
            }
        }
        if !confirmed_ingress_object(&tx, ingress.operation, origin.snapshot)?
            || !confirmed_ingress_object(&tx, ingress.operation, new_route)?
        {
            return Err(IndexError::DigestConflict);
        }
        for root in &snapshot.roots {
            if !confirmed_ingress_object(&tx, ingress.operation, *root)? {
                return Err(IndexError::DigestConflict);
            }
        }
        // The import may be an exact retry or a first import into a file with
        // leftover rows. Scan before either path can return success or mutate.
        let request = budget.request().clone();
        tx.progress_handler(
            1024,
            Some(move || {
                request
                    .checkpoint("restore protected Native heads")
                    .is_err()
            }),
        )
        .map_err(|error| sql(&error))?;
        let attached = require_attached_event_claims_v1(&tx, budget);
        tx.progress_handler(0, None::<fn() -> bool>)
            .map_err(|error| sql(&error))?;
        attached?;
        let already = validate_existing_v1(&tx, &expected, &operations, &claims, snapshot, budget)?;
        if !already {
            for view in expected.views.values() {
                ensure_roots(&tx, view)?;
                write_view(&tx, view)?;
            }
            for (workspace, terminal) in &expected.current {
                let payload = encode(terminal)?;
                exactly_one(
                    tx.execute(
                        "INSERT INTO native_workspace_v1(workspace,epoch,sequence,payload,row_digest) VALUES (?1,?2,?3,?4,?5)",
                        params![workspace_key(*workspace)?, int(terminal.identity.epoch)?, int(terminal.identity.sequence)?, &payload, digest(b"qidx.native.view.v1", &payload).to_vec()],
                    ).map_err(|error| sql(&error))?,
                    "import protected workspace",
                )?;
            }
            for input in &operations {
                // SQLite owns the bound BLOB and can copy it again when
                // building the record. The validated input remains borrowed.
                let backing = u64::try_from(input.payload.len())
                    .ok()
                    .and_then(|bytes| bytes.checked_mul(2))
                    .and_then(|bytes| bytes.checked_add(8192))
                    .ok_or(IndexError::ResourceLimited)?;
                let _sqlite_row = budget
                    .reserve_memory(backing, MemoryLifetime::Temporary)?
                    .ok_or(IndexError::ResourceLimited)?;
                write_operation_payload(&tx, input.key, input.payload, true, true)?;
            }
            for claim in &claims {
                write_event_claim(&tx, *claim, true)?;
            }
            for row in &snapshot.frontiers {
                let workspace = workspace_key(row.workspace)?;
                let epoch = int(row.stream_epoch)?;
                let sequence = int(row.sequence)?;
                let producer = row.producer.0.to_vec();
                let stream = row.stream.0.to_vec();
                exactly_one(tx.execute(
                    "INSERT INTO native_frontier_v1(workspace,producer,stream,stream_epoch,sequence,row_digest) VALUES (?1,?2,?3,?4,?5,?6)",
                    params![&workspace, &producer, &stream, epoch, sequence,
                        fields_digest(b"qidx.native.frontier.v1", &(&workspace, &producer, &stream, epoch, sequence))?],
                ).map_err(|error| sql(&error))?, "import protected frontier")?;
            }
            for row in &snapshot.source_authorities {
                let workspace = workspace_key(row.workspace)?;
                let workspace_epoch = int(row.workspace_epoch)?;
                let source_epoch = int(row.source_epoch)?;
                let producer = row.producer.0.to_vec();
                exactly_one(tx.execute(
                    "INSERT INTO native_source_authority_v1(workspace,workspace_epoch,producer,source_epoch,row_digest) VALUES (?1,?2,?3,?4,?5)",
                    params![&workspace, workspace_epoch, &producer, source_epoch,
                        fields_digest(b"qidx.native.sourceauthority.v1", &(&workspace, workspace_epoch, &producer, source_epoch))?],
                ).map_err(|error| sql(&error))?, "import protected source authority")?;
            }
        }
        drop(claims);
        drop(claim_memory);
        drop(operations);
        drop(expected);
        drop(indices_memory);
        let _verified_route = super::boot_manifest::validate_protected_routes(
            &tx,
            primary,
            new_route,
            source_routes,
            true,
            Some(ingress),
            budget,
        )?;
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        Ok(())
    }
}
