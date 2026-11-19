//! Explicit Exact-admission floor and bounded retirement of catalog view rows.

use quanta_index_contract::index::{
    NativeProtectedNativeOriginV1, native_protected_view_marker_ref_v1,
};
use quanta_index_core::index::ports::{CatalogExactHistoryPort, ViewRetirementPage};
use quanta_index_core::index::{
    CatalogView, Digest, IndexError, MemoryLease, MemoryLifetime, ObjectRef, RegisteredObject,
    ViewRef, WorkBudget, WorkUsage, WorkspaceKey,
};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params, types::ValueRef};

use super::{
    SqliteCatalog, begin, boot_manifest, current_view, decode, distinct_roots, exactly_one,
    fields_digest, int, object_install, object_key, read_view_row, require_registered_edges, sql,
    uint, workspace_key,
};

const FLOOR_DIGEST_DOMAIN: &[u8] = b"qidx.native.exactfloor.v1";
const MAX_RETIRE_VIEWS: u32 = 256;

fn row_bytes(parts: &[i64]) -> Result<u64, IndexError> {
    parts.iter().try_fold(0_u64, |total, &part| {
        total
            .checked_add(uint(part)?)
            .ok_or(IndexError::ResourceLimited)
    })
}

fn floor_row(
    connection: &Connection,
    workspace: WorkspaceKey,
    epoch: u64,
) -> Result<Option<u64>, IndexError> {
    let workspace_blob = workspace_key(workspace)?;
    let epoch_integer = int(epoch)?;
    let row: Option<(i64, Vec<u8>)> = connection
        .query_row(
            "SELECT retain_from_sequence,row_digest FROM native_exact_history_floor_v1 WHERE workspace=?1 AND epoch=?2",
            params![&workspace_blob, epoch_integer],
            |row| {
                let value = row.get_ref(1)?;
                let rusqlite::types::ValueRef::Blob(digest) = value else {
                    return Err(rusqlite::Error::InvalidColumnType(
                        1,
                        "row_digest".into(),
                        value.data_type(),
                    ));
                };
                if digest.len() != 32 {
                    return Err(rusqlite::Error::InvalidColumnType(
                        1,
                        "row_digest".into(),
                        rusqlite::types::Type::Blob,
                    ));
                }
                Ok((row.get(0)?, digest.to_vec()))
            },
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((stored, checksum)) = row else {
        return Ok(None);
    };
    if checksum
        != fields_digest(
            FLOOR_DIGEST_DOMAIN,
            &(&workspace_blob, epoch_integer, stored),
        )?
    {
        return Err(IndexError::Storage(
            "exact history floor digest mismatch".into(),
        ));
    }
    let floor = uint(stored)?;
    let head: Option<i64> = connection
        .query_row(
            "SELECT sequence FROM native_view_v1 WHERE workspace=?1 AND epoch=?2 ORDER BY sequence DESC LIMIT 1",
            params![&workspace_blob, epoch_integer],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let head = head.ok_or_else(|| IndexError::Storage("floor epoch has no view".into()))?;
    if floor > uint(head)? {
        return Err(IndexError::Storage(
            "exact history floor exceeds epoch head".into(),
        ));
    }
    Ok(Some(floor))
}

/// Check every retained floor through the same canonical digest/head validator
/// used by Exact admission. Old epochs may retain valid history after the
/// workspace advances; only a foreign or malformed workspace is refused.
pub(super) fn verify_source_floors(
    connection: &Connection,
    expected_workspace: WorkspaceKey,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let expected = workspace_key(expected_workspace)?;
    let mut statement = connection
        .prepare(
            "SELECT workspace,epoch FROM native_exact_history_floor_v1
             ORDER BY workspace,epoch",
        )
        .map_err(|error| sql(&error))?;
    let mut rows = statement.query([]).map_err(|error| sql(&error))?;
    loop {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let Some(row) = rows.next().map_err(|error| sql(&error))? else {
            break;
        };
        let ValueRef::Blob(workspace) = row.get_ref(0).map_err(|error| sql(&error))? else {
            return Err(IndexError::Storage(
                "exact floor workspace is not a BLOB".into(),
            ));
        };
        if workspace != expected.as_slice() {
            return Err(IndexError::Conflict);
        }
        let epoch = uint(row.get(1).map_err(|error| sql(&error))?)?;
        budget.charge(WorkUsage {
            reads: 2,
            bytes: u64::try_from(expected.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_add(32)
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        if floor_row(connection, expected_workspace, epoch)?.is_none() {
            return Err(IndexError::Storage(
                "exact floor disappeared in read snapshot".into(),
            ));
        }
    }
    budget.checkpoint()
}

/// Used by Exact acquisition in the same transaction that installs its pin.
pub(super) fn exact_floor_for_epoch(
    connection: &Connection,
    workspace: WorkspaceKey,
    epoch: u64,
) -> Result<u64, IndexError> {
    Ok(floor_row(connection, workspace, epoch)?.unwrap_or(0))
}

pub(super) fn charged_current_view(
    connection: &Connection,
    workspace: WorkspaceKey,
    budget: &mut WorkBudget,
) -> Result<Option<quanta_index_core::index::CatalogView>, IndexError> {
    let workspace_blob = workspace_key(workspace)?;
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let metadata: Option<(i64, i64, i64, i64)> = connection
        .query_row(
            "SELECT epoch,sequence,length(payload),length(row_digest) FROM native_workspace_v1 WHERE workspace=?1",
            params![&workspace_blob],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((epoch, sequence, payload_length, digest_length)) = metadata else {
        return Ok(None);
    };
    let workspace_lengths: [i64; 2] = (payload_length, digest_length).into();
    budget.charge(WorkUsage {
        reads: 1,
        bytes: row_bytes(&workspace_lengths)?,
        ..WorkUsage::default()
    })?;
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let historical: Option<(i64, i64, i64)> = connection
        .query_row(
            "SELECT length(identity_digest),length(payload),length(row_digest) FROM native_view_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3",
            params![&workspace_blob, epoch, sequence],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let (identity_length, view_length, view_digest_length) =
        historical.ok_or(IndexError::SnapshotUnavailable)?;
    let historical_lengths: [i64; 3] = (identity_length, view_length, view_digest_length).into();
    budget.charge(WorkUsage {
        reads: 1,
        bytes: row_bytes(&historical_lengths)?,
        ..WorkUsage::default()
    })?;
    let mut roots = connection
        .prepare(
            "SELECT length(object),length(row_digest) FROM native_view_root_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3 ORDER BY object LIMIT 8",
        )
        .map_err(|error| sql(&error))?;
    let lengths = roots
        .query_map(params![&workspace_blob, epoch, sequence], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| sql(&error))?;
    for (index, lengths) in lengths.enumerate() {
        if index >= 7 {
            return Err(IndexError::Storage("view has too many roots".into()));
        }
        let (object_length, digest_length) = lengths.map_err(|error| sql(&error))?;
        let root_lengths: [i64; 2] = (object_length, digest_length).into();
        budget.charge(WorkUsage {
            reads: 1,
            bytes: row_bytes(&root_lengths)?,
            ..WorkUsage::default()
        })?;
    }
    drop(roots);
    // `current_view` issues the workspace query and both history/roots
    // validation queries after the metadata preflight above.
    budget.charge(WorkUsage {
        reads: 3,
        ..WorkUsage::default()
    })?;
    current_view(connection, workspace)
}

fn next_retire_candidate(
    connection: &Connection,
    workspace: WorkspaceKey,
    epoch: u64,
    after: Option<u64>,
    floor: u64,
    budget: &mut WorkBudget,
) -> Result<Option<CatalogView>, IndexError> {
    let workspace_blob = workspace_key(workspace)?;
    let after_integer = after.map(int).transpose()?.unwrap_or(-1);
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let metadata: Option<(i64, i64, i64, i64)> = connection
        .query_row(
            "SELECT sequence,length(identity_digest),length(payload),length(row_digest) FROM native_view_v1 WHERE workspace=?1 AND epoch=?2 AND sequence>?3 AND sequence<?4 ORDER BY sequence LIMIT 1",
            params![&workspace_blob, int(epoch)?, after_integer, int(floor)?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((sequence, identity_length, payload_length, digest_length)) = metadata else {
        return Ok(None);
    };
    if identity_length != 32 || digest_length != 32 {
        return Err(IndexError::Storage(
            "retiring view digest shape mismatch".into(),
        ));
    }
    let lengths: [i64; 3] = (identity_length, payload_length, digest_length).into();
    budget.charge(WorkUsage {
        reads: 1,
        bytes: row_bytes(&lengths)?,
        ..WorkUsage::default()
    })?;
    let digest_blob: Vec<u8> = connection
        .query_row(
            "SELECT identity_digest FROM native_view_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3",
            params![&workspace_blob, int(epoch)?, sequence],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    let digest_bytes: [u8; 32] = digest_blob
        .try_into()
        .map_err(|_error| IndexError::Storage("retiring view identity width".into()))?;
    let identity = ViewRef {
        workspace,
        epoch,
        sequence: uint(sequence)?,
        digest: Digest(digest_bytes),
    };

    charged_view_roots_and_read(connection, identity, budget).map(Some)
}

/// Authenticate one exact historical view after charging its bounded row and
/// root ledger before the shared decoder allocates either body.
pub(super) fn charged_exact_view(
    connection: &Connection,
    identity: ViewRef,
    budget: &mut WorkBudget,
) -> Result<CatalogView, IndexError> {
    let workspace = workspace_key(identity.workspace)?;
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let lengths: Option<(i64, i64, i64)> = connection
        .query_row(
            "SELECT length(identity_digest),length(payload),length(row_digest)
             FROM native_view_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3",
            params![&workspace, int(identity.epoch)?, int(identity.sequence)?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let lengths: [i64; 3] = lengths.ok_or(IndexError::SnapshotUnavailable)?.into();
    let [identity_length, _, digest_length] = lengths;
    if identity_length != 32 || digest_length != 32 {
        return Err(IndexError::Storage("view digest shape mismatch".into()));
    }
    budget.charge(WorkUsage {
        reads: 1,
        bytes: row_bytes(&lengths)?,
        ..WorkUsage::default()
    })?;
    charged_view_roots_and_read(connection, identity, budget)
}

fn charged_view_roots_and_read(
    connection: &Connection,
    identity: ViewRef,
    budget: &mut WorkBudget,
) -> Result<CatalogView, IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let mut statement = connection
        .prepare(
            "SELECT length(object),length(row_digest) FROM native_view_root_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3 ORDER BY object LIMIT 8",
        )
        .map_err(|error| sql(&error))?;
    let root_lengths = statement
        .query_map(
            params![
                workspace_key(identity.workspace)?,
                int(identity.epoch)?,
                int(identity.sequence)?
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(|error| sql(&error))?;
    for (index, lengths) in root_lengths.enumerate() {
        if index >= 7 {
            return Err(IndexError::Storage("view has too many roots".into()));
        }
        let (object_length, digest_length) = lengths.map_err(|error| sql(&error))?;
        if object_length != 32 || digest_length != 32 {
            return Err(IndexError::Storage("view root shape mismatch".into()));
        }
        let lengths: [i64; 2] = (object_length, digest_length).into();
        budget.charge(WorkUsage {
            reads: 1,
            bytes: row_bytes(&lengths)?,
            ..WorkUsage::default()
        })?;
    }
    drop(statement);
    // `read_view_row` separately loads and validates the view and root ledger.
    budget.charge(WorkUsage {
        reads: 2,
        ..WorkUsage::default()
    })?;
    read_view_row(connection, identity)
}

struct SelectedRoute {
    registration: RegisteredObject,
    unanchored_children: Option<[ObjectRef; 2]>,
    _memory: Option<MemoryLease>,
}

impl SelectedRoute {
    fn read(
        connection: &Connection,
        route: boot_manifest::NativeBootSourceRouteRefV1,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        let memory = budget.reserve_memory(
            boot_manifest::REGISTRATION_SCRATCH_BYTES,
            MemoryLifetime::Temporary,
        )?;
        budget.charge(WorkUsage {
            reads: 12,
            bytes: boot_manifest::REGISTRATION_SCRATCH_BYTES,
            ..WorkUsage::default()
        })?;
        let mut registration = read_route_registration(connection, route.object, budget)?;
        require_registered_edges(connection, route.object, &registration)?;
        let unanchored_children = match route.marker_anchor {
            Some(anchor) => {
                if registration.children.len() != 2 {
                    return Err(IndexError::DigestConflict);
                }
                drop(registration);
                registration = read_route_registration(connection, anchor, budget)?;
                require_registered_edges(connection, anchor, &registration)?;
                None
            }
            None => match registration.children.as_slice() {
                [first, second] => Some([*first, *second]),
                _ => None,
            },
        };
        if registration
            .children
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err(IndexError::DigestConflict);
        }
        Ok(Self {
            registration,
            unanchored_children,
            _memory: memory,
        })
    }
}

fn blocked_by_pin_or_work(
    connection: &Connection,
    identity: ViewRef,
    route: Option<&SelectedRoute>,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    let workspace_blob = workspace_key(identity.workspace)?;
    let epoch = int(identity.epoch)?;
    let sequence = int(identity.sequence)?;
    budget.charge(WorkUsage {
        reads: 2,
        ..WorkUsage::default()
    })?;
    // Expiry is intentionally absent: an OS-live reader may still hold an
    // expired pin. Session recovery or explicit release removes that row.
    let pinned: i64 = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM native_pin_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3)",
            params![&workspace_blob, epoch, sequence],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    let work: i64 = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM native_work_v1 INDEXED BY native_work_created_view_idx_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3)",
            params![&workspace_blob, epoch, sequence],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    if pinned != 0 || work != 0 {
        return Ok(true);
    }
    // Exact markers survive a cold restart under the selected BOOT.
    let Some(route) = route else {
        return Ok(false);
    };
    let _marker_memory = budget.reserve_memory(1024, MemoryLifetime::Temporary)?;
    budget.charge(WorkUsage {
        bytes: 1024,
        ..WorkUsage::default()
    })?;
    let marker =
        native_protected_view_marker_ref_v1(identity).map_err(|_| IndexError::DigestConflict)?;
    if route.registration.children.binary_search(&marker).is_ok() {
        return Ok(true);
    }
    match route.unanchored_children {
        Some(children) => compact_route_contains_marker(connection, children, marker, budget),
        None => Ok(false),
    }
}

fn require_route_edge(
    connection: &Connection,
    parent: ObjectRef,
    child: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        bytes: 64,
        ..WorkUsage::default()
    })?;
    let parent = object_key(parent);
    let child = object_key(child);
    let expected_digest = fields_digest(b"qidx.native.edge.v1", &(&parent, &child))?;
    let protected: Option<bool> = connection
        .query_row(
            "SELECT CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
             FROM native_object_edge_v1 WHERE parent=?1 AND child=?2",
            params![&parent, &child],
            |row| {
                Ok(matches!(row.get_ref(0)?, ValueRef::Blob(digest) if digest == expected_digest))
            },
        )
        .optional()
        .map_err(|error| sql(&error))?;
    if protected != Some(true) {
        return Err(IndexError::DigestConflict);
    }
    Ok(())
}

fn read_route_registration(
    connection: &Connection,
    object: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<RegisteredObject, IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        bytes: 32,
        ..WorkUsage::default()
    })?;
    let registration = object_install::read_metadata(
        connection,
        object,
        object_install::MetadataLedger::Registered,
        || IndexError::CorruptObject("registered object metadata shape mismatch".into()),
        |bytes| {
            budget.charge(WorkUsage {
                bytes,
                ..WorkUsage::default()
            })
        },
    )?
    .ok_or_else(|| {
        IndexError::CorruptObject("referenced root or child is not registered".into())
    })?;
    if registration.reference != object {
        return Err(IndexError::CorruptObject(
            "object key does not match registration".into(),
        ));
    }
    Ok(registration)
}

fn snapshot_owns_previous_route(
    connection: &Connection,
    snapshot: ObjectRef,
    previous: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    let registration = read_route_registration(connection, snapshot, budget)?;
    if registration.children.contains(&previous) {
        require_route_edge(connection, snapshot, previous, budget)?;
        return Ok(true);
    }
    // The paged layout puts the previous route in the first page's roots.
    let [first, _second] = registration.children.as_slice() else {
        return Ok(false);
    };
    if !read_route_registration(connection, *first, budget)?
        .children
        .contains(&previous)
    {
        return Ok(false);
    }
    require_route_edge(connection, snapshot, *first, budget)?;
    require_route_edge(connection, *first, previous, budget)?;
    Ok(true)
}

/// Find the direct marker carrier once in the seal transaction. The old
/// route has the same heads. Each compact predecessor identifies its old
/// route as the sibling also referenced by its inline snapshot or first page.
pub(super) fn compact_marker_anchor_at_seal(
    connection: &Connection,
    selected: ObjectRef,
    origin: &NativeProtectedNativeOriginV1,
    markers: &[ObjectRef],
    budget: &mut WorkBudget,
) -> Result<Option<ObjectRef>, IndexError> {
    let _registration = budget.reserve_memory(
        boot_manifest::REGISTRATION_SCRATCH_BYTES,
        MemoryLifetime::Temporary,
    )?;
    let carries_markers = |registration: &RegisteredObject| {
        markers
            .iter()
            .filter(|marker| **marker != selected)
            .all(|marker| registration.children.binary_search(marker).is_ok())
    };
    let registration = read_route_registration(connection, selected, budget)?;
    if carries_markers(&registration) {
        return Ok(None);
    }
    if registration.children.len() != 2
        || !registration.children.contains(&origin.source_routes)
        || !registration.children.contains(&origin.snapshot)
        || origin.source_routes == origin.snapshot
    {
        return Err(IndexError::DigestConflict);
    }
    drop(registration);
    let mut current = origin.source_routes;
    loop {
        let registration = read_route_registration(connection, current, budget)?;
        if registration
            .children
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
        {
            return Err(IndexError::DigestConflict);
        }
        if carries_markers(&registration) {
            require_registered_edges(connection, current, &registration)?;
            return Ok(Some(current));
        }
        let [left, right] = registration.children.as_slice() else {
            return Err(IndexError::DigestConflict);
        };
        require_route_edge(connection, current, *left, budget)?;
        require_route_edge(connection, current, *right, budget)?;
        current = match (
            snapshot_owns_previous_route(connection, *right, *left, budget)?,
            snapshot_owns_previous_route(connection, *left, *right, budget)?,
        ) {
            (true, false) => *left,
            (false, true) => *right,
            _ => return Err(IndexError::DigestConflict),
        };
    }
}

fn compact_route_contains_marker(
    connection: &Connection,
    children: [ObjectRef; 2],
    marker: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    // Compacted routes retain the unchanged head set via the previous route
    // and snapshot. Every non-compacted route owns its markers directly.
    // Follow two-edge carriers only. Work admission also bounds malformed
    // graphs, including a cycle in an otherwise invalid edge ledger.
    let mut stack_memory = None;
    let mut pending = Vec::new();
    let mut next = Some(children);
    loop {
        if let Some(children) = next.take() {
            let needed = pending
                .len()
                .checked_add(children.len())
                .ok_or(IndexError::ResourceLimited)?;
            if needed > pending.capacity() {
                let capacity = pending.capacity().saturating_mul(2).max(needed);
                let bytes = capacity
                    .checked_mul(std::mem::size_of::<ObjectRef>())
                    .and_then(|bytes| u64::try_from(bytes).ok())
                    .ok_or(IndexError::ResourceLimited)?;
                let replacement = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
                pending
                    .try_reserve_exact(capacity - pending.len())
                    .map_err(|_| IndexError::ResourceLimited)?;
                if pending.capacity() > capacity {
                    return Err(IndexError::ResourceLimited);
                }
                drop(std::mem::replace(&mut stack_memory, replacement));
            }
            pending.extend(children);
        }
        let Some(parent) = pending.pop() else {
            return Ok(false);
        };
        let registration = read_route_registration(connection, parent, budget)?;
        if registration.children.contains(&marker) {
            require_route_edge(connection, parent, marker, budget)?;
            return Ok(true);
        }
        if let [first, second] = registration.children.as_slice() {
            require_route_edge(connection, parent, *first, budget)?;
            require_route_edge(connection, parent, *second, budget)?;
            next = Some([*first, *second]);
        }
    }
}

fn delete_retired_view(
    connection: &Connection,
    view: &CatalogView,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let workspace_blob = workspace_key(view.identity.workspace)?;
    let epoch = int(view.identity.epoch)?;
    let sequence = int(view.identity.sequence)?;
    let root_count = distinct_roots(view).len();
    if root_count == 0 || root_count > 7 {
        return Err(IndexError::Storage(
            "retiring view root count invalid".into(),
        ));
    }
    let writes = u64::try_from(root_count)
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_add(1)
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        writes,
        mutations: writes,
        ..WorkUsage::default()
    })?;
    let removed_roots = connection
        .execute(
            "DELETE FROM native_view_root_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3",
            params![&workspace_blob, epoch, sequence],
        )
        .map_err(|error| sql(&error))?;
    if removed_roots != root_count {
        return Err(IndexError::Storage(
            "view root deletion count mismatch".into(),
        ));
    }
    let removed_view = connection
        .execute(
            "DELETE FROM native_view_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3",
            params![&workspace_blob, epoch, sequence],
        )
        .map_err(|error| sql(&error))?;
    exactly_one(removed_view, "retire exact history view")?;
    if super::reclaim::is_bound(connection)? {
        budget.charge(WorkUsage {
            reads: writes.checked_mul(10).ok_or(IndexError::ResourceLimited)?,
            writes,
            bytes: writes.checked_mul(640).ok_or(IndexError::ResourceLimited)?,
            mutations: writes,
        })?;
        for root in distinct_roots(view) {
            super::reclaim::release(connection, root)?;
        }
    }
    Ok(())
}

fn write_floor(
    connection: &Connection,
    workspace: WorkspaceKey,
    epoch: u64,
    prior: Option<u64>,
    retain_from_sequence: u64,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if retain_from_sequence == prior.unwrap_or(0) {
        return Ok(());
    }
    let workspace_blob = workspace_key(workspace)?;
    let epoch_integer = int(epoch)?;
    let retain_integer = int(retain_from_sequence)?;
    let checksum = fields_digest(
        FLOOR_DIGEST_DOMAIN,
        &(&workspace_blob, epoch_integer, retain_integer),
    )?;
    budget.charge(WorkUsage {
        writes: 1,
        mutations: 1,
        bytes: u64::try_from(workspace_blob.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_add(32 + 16)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let changed = if prior.is_some() {
        connection.execute(
            "UPDATE native_exact_history_floor_v1 SET retain_from_sequence=?3,row_digest=?4 WHERE workspace=?1 AND epoch=?2",
            params![&workspace_blob, epoch_integer, retain_integer, checksum],
        )
    } else {
        connection.execute(
            "INSERT INTO native_exact_history_floor_v1(workspace,epoch,retain_from_sequence,row_digest) VALUES (?1,?2,?3,?4)",
            params![&workspace_blob, epoch_integer, retain_integer, checksum],
        )
    }.map_err(|error| sql(&error))?;
    exactly_one(changed, "advance exact history floor")
}

fn tail_floor_snapshot(
    connection: &Connection,
    workspace: WorkspaceKey,
    epoch: u64,
    preceding: u64,
    budget: &mut WorkBudget,
) -> Result<(Option<u64>, u64), IndexError> {
    let current = charged_current_view(connection, workspace, budget)?
        .ok_or(IndexError::SnapshotUnavailable)?;
    if current.identity.epoch != epoch {
        return Err(IndexError::SnapshotUnavailable);
    }
    budget.charge(WorkUsage {
        reads: 2,
        bytes: 32,
        ..WorkUsage::default()
    })?;
    let prior = floor_row(connection, workspace, epoch)?;
    if prior.is_some_and(|floor| floor > current.identity.sequence) {
        return Err(IndexError::Storage(
            "exact floor exceeds current view".into(),
        ));
    }
    let floor = current
        .identity
        .sequence
        .saturating_sub(preceding)
        .max(prior.unwrap_or(0));
    Ok((prior, floor))
}

impl CatalogExactHistoryPort for SqliteCatalog {
    fn retain_exact_history_tail(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        sequence_window: u64,
        budget: &mut WorkBudget,
    ) -> Result<u64, IndexError> {
        let preceding = sequence_window.checked_sub(1).ok_or_else(|| {
            IndexError::InvalidInput("exact history sequence window must be positive".into())
        })?;
        let _window_integer = int(sequence_window)?;
        budget.checkpoint()?;
        let mut guard = self.lock_native_budget(budget)?;
        // The common no-op round needs one consistent head/floor snapshot, not
        // a SQLite writer reservation. A later writer may still advance the
        // head; the advancing path rechecks both rows under BEGIN IMMEDIATE.
        let read = guard
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(|error| sql(&error))?;
        let (prior, floor) = tail_floor_snapshot(&read, workspace, epoch, preceding, budget)?;
        if prior.unwrap_or(0) == floor {
            budget.checkpoint()?;
            read.commit().map_err(|error| sql(&error))?;
            drop(guard);
            return Ok(floor);
        }
        drop(read);
        let tx = begin(&mut guard)?;
        let (prior, floor) = tail_floor_snapshot(&tx, workspace, epoch, preceding, budget)?;
        write_floor(&tx, workspace, epoch, prior, floor, budget)?;
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(floor)
    }

    fn exact_history_floor(&self, workspace: WorkspaceKey, epoch: u64) -> Result<u64, IndexError> {
        let guard = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let current = current_view(&guard, workspace)?.ok_or(IndexError::SnapshotUnavailable)?;
        if current.identity.epoch != epoch {
            return Err(IndexError::SnapshotUnavailable);
        }
        let floor = exact_floor_for_epoch(&guard, workspace, epoch);
        drop(guard);
        floor
    }

    fn advance_exact_history_floor(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        retain_from_sequence: u64,
        budget: &mut WorkBudget,
    ) -> Result<u64, IndexError> {
        let _retain_integer = int(retain_from_sequence)?;
        budget.checkpoint()?;
        let mut guard = self.lock_native_budget(budget)?;
        let tx = begin(&mut guard)?;
        let current =
            charged_current_view(&tx, workspace, budget)?.ok_or(IndexError::SnapshotUnavailable)?;
        if current.identity.epoch != epoch {
            return Err(IndexError::SnapshotUnavailable);
        }
        if retain_from_sequence > current.identity.sequence {
            return Err(IndexError::InvalidInput(
                "exact history floor exceeds current view".into(),
            ));
        }
        budget.charge(WorkUsage {
            reads: 2,
            bytes: 32,
            ..WorkUsage::default()
        })?;
        let prior = floor_row(&tx, workspace, epoch)?;
        let previous = prior.unwrap_or(0);
        if retain_from_sequence < previous {
            return Err(IndexError::Conflict);
        }
        write_floor(&tx, workspace, epoch, prior, retain_from_sequence, budget)?;
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(retain_from_sequence)
    }

    fn retire_exact_history_page(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        after: Option<u64>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<ViewRetirementPage, IndexError> {
        if !(1..=MAX_RETIRE_VIEWS).contains(&limit) {
            return Err(IndexError::InvalidInput(
                "view retirement page limit must be in 1..=256".into(),
            ));
        }
        let _cursor_integer = after.map(int).transpose()?;
        budget.checkpoint()?;
        let mut guard = self.lock_native_budget(budget)?;
        let tx = begin(&mut guard)?;
        budget.charge(WorkUsage {
            reads: 2,
            bytes: 32,
            ..WorkUsage::default()
        })?;
        let floor = exact_floor_for_epoch(&tx, workspace, epoch)?;
        let current =
            charged_current_view(&tx, workspace, budget)?.ok_or(IndexError::SnapshotUnavailable)?;
        if floor == 0 || after.is_some_and(|sequence| sequence >= floor) {
            budget.checkpoint()?;
            tx.commit().map_err(|error| sql(&error))?;
            drop(guard);
            return Ok(ViewRetirementPage {
                after,
                examined: 0,
                retired: 0,
                blocked: 0,
                has_more: false,
            });
        }
        let selected_route = boot_manifest::read_stored(&tx)?
            .and_then(|selected| selected.protected_source_routes)
            .map(|route| SelectedRoute::read(&tx, route, budget))
            .transpose()?;
        let mut cursor = after;
        let mut examined = 0_u32;
        let mut retired = 0_u32;
        let mut blocked = 0_u32;
        while examined < limit {
            let Some(view) = next_retire_candidate(&tx, workspace, epoch, cursor, floor, budget)?
            else {
                break;
            };
            let identity = view.identity;
            if identity == current.identity {
                return Err(IndexError::Storage(
                    "current view appears below retention floor".into(),
                ));
            }
            let protected = blocked_by_pin_or_work(&tx, identity, selected_route.as_ref(), budget)?;
            if protected {
                blocked = blocked.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            } else {
                delete_retired_view(&tx, &view, budget)?;
                retired = retired.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
            examined = examined.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            cursor = Some(identity.sequence);
        }
        // Validate page+1 before commit so corruption cannot strand a
        // partially retired page behind an apparently usable cursor.
        let has_more =
            next_retire_candidate(&tx, workspace, epoch, cursor, floor, budget)?.is_some();
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(ViewRetirementPage {
            after: cursor,
            examined,
            retired,
            blocked,
            has_more,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::index::{
        CollectionId, SlotId, TenantId, WorktreeId, native_immutable_object_ref_v1,
        native_protected_view_marker_bytes_v1,
    };
    use quanta_index_core::{RequestBudgetV1, index::WorkLimits};
    use std::time::Duration;

    #[test]
    fn compact_selected_route_protects_exact_marker_without_operation_custody()
    -> Result<(), Box<dyn std::error::Error>> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(super::super::SCHEMA)?;
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        let retained = ViewRef {
            workspace,
            epoch: 1,
            sequence: 0,
            digest: Digest([5; 32]),
        };
        let unrelated = ViewRef {
            sequence: 1,
            digest: Digest([6; 32]),
            ..retained
        };
        let register = |payload: &[u8], children: &[ObjectRef]| {
            let reference = native_immutable_object_ref_v1(payload, children)
                .ok_or(IndexError::ResourceLimited)?;
            let bytes = 16 + payload.len() + 32 * children.len();
            super::super::register(
                &connection,
                &[RegisteredObject::unscoped(
                    reference,
                    u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
                    children.to_vec(),
                )],
            )?;
            Ok::<_, IndexError>(reference)
        };
        let marker = register(&native_protected_view_marker_bytes_v1(retained)?, &[])?;
        let orphan = register(&native_protected_view_marker_bytes_v1(unrelated)?, &[])?;
        let profile = register(b"old route profile", &[])?;
        let request = register(b"old route request", &[])?;
        let mut direct_children = [marker, profile, request];
        direct_children.sort_unstable();
        let direct = register(b"old route", &direct_children)?;
        let first_page = register(b"old snapshot page one", &[direct, profile, request])?;
        let second_page = register(b"old snapshot page two", &[])?;
        let snapshot = register(b"old snapshot manifest", &[first_page, second_page])?;
        let mut first_children = [direct, snapshot];
        first_children.sort_unstable();
        let first = register(b"first compact route", &first_children)?;
        let snapshot = register(b"new snapshot", &[first, profile, request])?;
        let mut selected_children = [first, snapshot];
        selected_children.sort_unstable();
        let selected = register(b"selected compact route", &selected_children)?;
        assert_ne!(orphan, marker);
        assert!(!selected_children.contains(&marker));

        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 128,
                bytes: 16 * 1024 * 1024,
                writes: 0,
                mutations: 0,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        let unanchored = boot_manifest::NativeBootSourceRouteRefV1 {
            object: selected,
            count: 0,
            marker_anchor: None,
        };
        let legacy_record = serde_json::json!({ "object": selected, "count": 0 });
        assert_eq!(
            serde_json::from_value::<boot_manifest::NativeBootSourceRouteRefV1>(legacy_record)?,
            unanchored
        );
        let route = SelectedRoute::read(&connection, unanchored, &mut budget)?;
        // The test has only registered objects and edges, as after terminal
        // operation custody is retired. The old route still holds the marker.
        assert!(blocked_by_pin_or_work(
            &connection,
            retained,
            Some(&route),
            &mut budget
        )?);
        assert!(!blocked_by_pin_or_work(
            &connection,
            unrelated,
            Some(&route),
            &mut budget
        )?);
        let marker_anchor = compact_marker_anchor_at_seal(
            &connection,
            selected,
            &NativeProtectedNativeOriginV1 {
                source_root_nonce: Digest([7; 32]),
                source_routes: first,
                snapshot,
            },
            &[selected, marker],
            &mut budget,
        )?;
        assert_eq!(marker_anchor, Some(direct));
        let anchored = boot_manifest::NativeBootSourceRouteRefV1 {
            marker_anchor,
            ..unanchored
        };
        let anchored = serde_json::from_slice(&serde_json::to_vec(&anchored)?)?;
        let loaded = SelectedRoute::read(&connection, anchored, &mut budget)?;
        let mut point_lookup = WorkBudget::with_request(
            WorkLimits {
                reads: 4,
                bytes: 4096,
                writes: 0,
                mutations: 0,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        assert!(blocked_by_pin_or_work(
            &connection,
            retained,
            Some(&loaded),
            &mut point_lookup
        )?);
        assert!(!blocked_by_pin_or_work(
            &connection,
            unrelated,
            Some(&loaded),
            &mut point_lookup
        )?);
        assert_eq!(point_lookup.usage().reads, 4);
        assert_eq!(
            connection.execute(
                "DELETE FROM native_object_edge_v1 WHERE parent=?1 AND child=?2",
                params![object_key(direct), object_key(marker)],
            )?,
            1
        );
        assert_eq!(
            blocked_by_pin_or_work(&connection, retained, Some(&route), &mut budget),
            Err(IndexError::DigestConflict)
        );
        assert!(SelectedRoute::read(&connection, anchored, &mut budget).is_err());
        Ok(())
    }
}
