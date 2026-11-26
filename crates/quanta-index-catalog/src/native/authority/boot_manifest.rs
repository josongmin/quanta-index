//! Selected boot custody uses the existing operation-object relation.
//! It preserves the original request and source/profile, not old search results.

use quanta_index_contract::{
    RepoId, RevisionId,
    index::{
        MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1, MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1,
        NativeProtectedRepoMapHeadV1, NativeProtectedSourceRoutesV1, RetiredProjectionPolicyRefV1,
    },
};
use quanta_index_core::index::{
    CatalogView, Digest, ImmutableOperation, IndexError, IngressRef, MemoryLease, MemoryLifetime,
    ObjectRef, OperationKey, OperationStatus, WorkBudget, WorkUsage,
};
use rusqlite::{Connection, OptionalExtension as _, params, types::ValueRef};

use super::{
    IngressRow, SqliteCatalog, begin, boot_quiescence,
    boot_record::{
        MAX_BYTES, NativeBootManifest, NativeBootSourceRouteRefV1, REGISTRATION_SCRATCH_BYTES,
        SCRATCH_BYTES, charge_operation, checksum, load_publication, read_stored, validate_custody,
        with_protected_operation,
    },
    confirmed_ingress_membership, delete_ingress, exactly_one, fields_digest, object_key,
    operation_key, read_ingress, read_operation, reclaim, require_object,
    require_registered_edges_with_budget, retention, sql, uint,
    workspace_integrity::require_registered_workspace_rows_v1,
    write_operation,
};

fn protect(
    connection: &Connection,
    selected: &NativeBootManifest,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let operation = operation_key(selected.operation.key)?;
    for object in selected.all_protected_roots() {
        budget.checkpoint()?;
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let size: Option<i64> = connection
            .query_row(
                "SELECT CASE WHEN typeof(payload)='blob' AND length(payload) BETWEEN 1 AND 1048576
              AND typeof(row_digest)='blob' AND length(row_digest)=32 THEN length(payload) END
             FROM native_object_v1 WHERE object=?1",
                params![object_key(object)],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| sql(&error))?;
        let size = uint(size.ok_or(IndexError::SnapshotUnavailable)?)?;
        let _registration =
            budget.reserve_memory(REGISTRATION_SCRATCH_BYTES, MemoryLifetime::Temporary)?;
        budget.charge(WorkUsage {
            reads: 12,
            writes: 4,
            bytes: size.checked_add(1024).ok_or(IndexError::ResourceLimited)?,
            mutations: 4,
        })?;
        require_object(connection, object)?;
        reclaim::adopt_if_known(connection, object)?;
        let object = object_key(object);
        let _inserted = connection
            .execute(
                "INSERT INTO native_operation_object_v1(operation_id,object,row_digest)
             VALUES ((SELECT operation_id FROM native_operation_v1 WHERE operation=?1),?2,?3) ON CONFLICT(operation_id,object) DO NOTHING",
                params![
                    &operation,
                    &object,
                    fields_digest(b"qidx.native.opobject.v1", &(&operation, &object))?
                ],
            )
            .map_err(|error| sql(&error))?;
        let expected = fields_digest(b"qidx.native.opobject.v1", &(&operation, &object))?;
        let valid: bool = connection.query_row(
            "SELECT row_digest FROM native_operation_object_v1 WHERE operation_id=(SELECT operation_id FROM native_operation_v1 WHERE operation=?1) AND object=?2",
            params![&operation,&object], |row| Ok(matches!(row.get_ref(0)?, ValueRef::Blob(digest) if digest == expected)),
        ).map_err(|error| sql(&error))?;
        if !valid {
            return Err(IndexError::Storage(
                "boot object custody digest mismatch".into(),
            ));
        }
    }
    Ok(())
}

fn requeue_previous(
    connection: &Connection,
    previous: Option<&NativeBootManifest>,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if let Some(previous) = previous {
        let requeue = |row: &super::OperationRow, budget: &mut WorkBudget| {
            if row.immutable != previous.operation
                || row.status != OperationStatus::Published(previous.publication.identity)
            {
                return Err(IndexError::Storage(
                    "selected boot operation changed".into(),
                ));
            }
            budget.charge(WorkUsage {
                reads: 4,
                writes: 3,
                bytes: 8192,
                mutations: 3,
            })?;
            // Reuses the existing terminal ready sequence and non-reusing fence.
            write_operation(connection, row, false)
        };
        if previous.protected_source_routes.is_some() {
            with_protected_operation(connection, previous.operation.key, budget, requeue)?;
        } else {
            charge_operation(connection, previous.operation.key, budget)?;
            let row = read_operation(connection, previous.operation.key)?
                .ok_or(IndexError::SnapshotUnavailable)?;
            requeue(&row, budget)?;
        }
    }
    Ok(())
}

fn verify_repomap_source_publication(
    connection: &Connection,
    head: &NativeProtectedRepoMapHeadV1,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if let Some(source_operation) = &head.source_operation {
        with_protected_operation(connection, source_operation.key, budget, |source, _| {
            if source.immutable != *source_operation
                || source.status != OperationStatus::Published(head.source_publication.identity)
                || source.published_view.as_ref() != Some(&head.source_publication)
            {
                return Err(IndexError::DigestConflict);
            }
            Ok(())
        })?;
    } else if head.source_publication.identity.sequence != 0
        || retention::charged_exact_view(connection, head.source_publication.identity, budget)?
            != head.source_publication
    {
        return Err(IndexError::DigestConflict);
    }
    Ok(())
}

pub(super) fn validate_protected_routes(
    connection: &Connection,
    primary: OperationKey,
    route_object: ObjectRef,
    routes: &NativeProtectedSourceRoutesV1,
    require_current: bool,
    allowed_ingress: Option<IngressRef>,
    budget: &mut WorkBudget,
) -> Result<NativeBootSourceRouteRefV1, IndexError> {
    validate_protected_routes_with_publications(
        connection,
        primary,
        route_object,
        routes,
        require_current,
        allowed_ingress,
        budget,
        |_, _, _| Ok(()),
    )
}

pub(super) fn validate_protected_routes_with_publications(
    connection: &Connection,
    primary: OperationKey,
    route_object: ObjectRef,
    routes: &NativeProtectedSourceRoutesV1,
    require_current: bool,
    allowed_ingress: Option<IngressRef>,
    budget: &mut WorkBudget,
    visit_published: impl FnMut(
        ImmutableOperation,
        CatalogView,
        &mut WorkBudget,
    ) -> Result<(), IndexError>,
) -> Result<NativeBootSourceRouteRefV1, IndexError> {
    budget.checkpoint()?;
    // Current routes reach the same workspace membership check through
    // quiescence. Retained BOOT reads must also refuse unattached rows.
    if !require_current {
        require_registered_workspace_rows_v1(connection, budget)?;
    }
    let children_backing = u64::try_from(
        routes
            .children_backing_bytes()
            .map_err(|_shape| IndexError::DigestConflict)?,
    )
    .map_err(|_width| IndexError::ResourceLimited)?;
    let _route_memory = budget.reserve_memory(
        children_backing
            .checked_add(REGISTRATION_SCRATCH_BYTES)
            .ok_or(IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let (reference, payload_bytes, children) = routes
        .object_identity_with_work_v1(&mut || budget.checkpoint())
        .map_err(|cause| cause.into_error(|_| IndexError::DigestConflict))?;
    budget.charge(WorkUsage {
        bytes: u64::try_from(payload_bytes).map_err(|_| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    if reference != route_object {
        return Err(IndexError::DigestConflict);
    }
    let count = u32::try_from(routes.heads.len()).map_err(|_| IndexError::ResourceLimited)?;
    for head in &routes.heads {
        budget.checkpoint()?;
        with_protected_operation(
            connection,
            head.source_operation.key,
            budget,
            |source, _| {
                if source.immutable != head.source_operation
                    || source.status != OperationStatus::Published(head.source_publication.identity)
                    || source.published_view.as_ref() != Some(&head.source_publication)
                    || head.source_publication.source != head.publication.source
                {
                    return Err(IndexError::DigestConflict);
                }
                Ok(())
            },
        )?;
        with_protected_operation(connection, head.operation.key, budget, |operation, _| {
            if operation.immutable != head.operation
                || operation.status != OperationStatus::Published(head.publication.identity)
                || operation.published_view.as_ref() != Some(&head.publication)
            {
                return Err(IndexError::DigestConflict);
            }
            Ok(())
        })?;
        if retention::charged_exact_view(connection, head.publication.identity, budget)?
            != head.publication
        {
            return Err(IndexError::DigestConflict);
        }
        if require_current
            && retention::charged_current_view(connection, head.operation.key.workspace, budget)?
                .as_ref()
                != Some(&head.publication)
        {
            return Err(IndexError::Conflict);
        }
    }
    let primary_view = if require_current {
        let primary_publication =
            with_protected_operation(connection, primary, budget, |row, _| {
                let OperationStatus::Published(identity) = &row.status else {
                    return Err(IndexError::Conflict);
                };
                if identity.workspace != primary.workspace
                    || identity.epoch != primary.epoch
                    || identity.digest.0 == [0; 32]
                {
                    return Err(IndexError::DigestConflict);
                }
                let publication = row.published_view.as_ref().ok_or_else(|| {
                    IndexError::Storage("Published operation lacks persisted metadata".into())
                })?;
                if publication.identity != *identity {
                    return Err(IndexError::DigestConflict);
                }
                Ok(publication.clone())
            })?;
        if retention::charged_current_view(connection, primary.workspace, budget)?.as_ref()
            != Some(&primary_publication)
        {
            return Err(IndexError::Conflict);
        }
        Some(primary_publication.identity)
    } else {
        None
    };
    for head in &routes.repomap_heads {
        budget.checkpoint()?;
        verify_repomap_source_publication(connection, head, budget)?;
        with_protected_operation(connection, head.operation.key, budget, |operation, _| {
            if operation.immutable != head.operation
                || operation.status != OperationStatus::Published(head.publication.identity)
                || operation.published_view.as_ref() != Some(&head.publication)
            {
                return Err(IndexError::DigestConflict);
            }
            Ok(())
        })?;
        if retention::charged_exact_view(connection, head.publication.identity, budget)?
            != head.publication
        {
            return Err(IndexError::DigestConflict);
        }
        if require_current
            && retention::charged_current_view(connection, head.operation.key.workspace, budget)?
                .as_ref()
                != Some(&head.publication)
        {
            return Err(IndexError::Conflict);
        }
        require_object(connection, head.member_binding)?;
    }
    let registration = super::object_install::read_metadata(
        connection,
        route_object,
        super::object_install::MetadataLedger::Registered,
        || IndexError::CorruptObject("registered object metadata shape mismatch".into()),
        |bytes| {
            budget.charge(WorkUsage {
                reads: 1,
                bytes,
                ..WorkUsage::default()
            })
        },
    )?
    .ok_or_else(|| {
        IndexError::CorruptObject("referenced root or child is not registered".into())
    })?;
    require_registered_edges_with_budget(connection, route_object, &registration, Some(budget))?;
    if registration.children != children {
        return Err(IndexError::DigestConflict);
    }
    drop(registration);
    drop(children);
    drop(_route_memory);
    // Route and member validation must finish before publication observers
    // consume rows. Rollback and Source readout can then use this same scan.
    if let Some(primary_view) = primary_view {
        boot_quiescence::check_quiescent_protected_routes_with_publications(
            connection,
            primary_view,
            routes,
            allowed_ingress,
            budget,
            visit_published,
        )?;
    }
    Ok(NativeBootSourceRouteRefV1 {
        object: route_object,
        count,
        marker_anchor: None,
    })
}

struct RouteMarkerRefs {
    objects: Vec<ObjectRef>,
    _memory: Option<MemoryLease>,
}

fn route_marker_refs(
    routes: &NativeProtectedSourceRoutesV1,
    route_object: ObjectRef,
    retired_policy: Option<ObjectRef>,
    budget: &WorkBudget,
) -> Result<RouteMarkerRefs, IndexError> {
    if routes.heads.len() > MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1
        || routes.repomap_heads.len() > MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1
    {
        return Err(IndexError::DigestConflict);
    }
    let genesis_markers = routes
        .repomap_heads
        .iter()
        .filter(|head| head.source_operation.is_none())
        .count();
    let capacity = routes
        .heads
        .len()
        .checked_add(routes.repomap_heads.len())
        .and_then(|heads| heads.checked_add(genesis_markers))
        .and_then(|markers| markers.checked_add(1))
        .and_then(|markers| markers.checked_add(usize::from(retired_policy.is_some())))
        .ok_or(IndexError::ResourceLimited)?;
    let bytes = capacity
        .checked_mul(std::mem::size_of::<ObjectRef>())
        .ok_or(IndexError::ResourceLimited)?;
    let bytes = u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited)?;
    let memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
    let mut objects = Vec::new();
    objects
        .try_reserve_exact(capacity)
        .map_err(|_allocation| IndexError::ResourceLimited)?;
    objects.push(route_object);
    if let Some(policy) = retired_policy {
        objects.push(policy);
    }
    for head in &routes.heads {
        objects.push(
            quanta_index_contract::index::native_protected_view_marker_ref_v1(
                head.publication.identity,
            )
            .map_err(|_| IndexError::DigestConflict)?,
        );
    }
    for head in &routes.repomap_heads {
        objects.push(
            quanta_index_contract::index::native_protected_view_marker_ref_v1(
                head.publication.identity,
            )
            .map_err(|_| IndexError::DigestConflict)?,
        );
        if head.source_operation.is_none() {
            objects.push(
                quanta_index_contract::index::native_protected_view_marker_ref_v1(
                    head.source_publication.identity,
                )
                .map_err(|_| IndexError::DigestConflict)?,
            );
        }
    }
    objects.sort_unstable();
    objects.dedup();
    if objects.len() < 2 {
        return Err(IndexError::DigestConflict);
    }
    Ok(RouteMarkerRefs {
        objects,
        _memory: memory,
    })
}

fn validate_route_ingress(
    connection: &Connection,
    primary: OperationKey,
    ingress: IngressRef,
    route_object: ObjectRef,
    routes: &NativeProtectedSourceRoutesV1,
    retired_policy: Option<ObjectRef>,
    budget: &mut WorkBudget,
) -> Result<RouteMarkerRefs, IndexError> {
    if ingress.operation == primary
        || ingress.operation.workspace != primary.workspace
        || ingress.operation.epoch != primary.epoch
        || protected_route_operation_exists(connection, ingress.operation, budget)?
    {
        return Err(IndexError::Conflict);
    }
    require_open_protected_route_ingress(connection, ingress, budget)?;
    let markers = route_marker_refs(routes, route_object, retired_policy, budget)?;
    let objects = &markers.objects;
    let count: i64 = connection
        .query_row(
            "SELECT count(*) FROM native_ingress_object_v1 WHERE operation=?1",
            params![operation_key(ingress.operation)?],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    let actual = usize::try_from(count).map_err(|_| IndexError::DigestConflict)?;
    if (routes.native_origin.is_none() && actual != objects.len()) || actual < objects.len() {
        return Err(IndexError::Conflict);
    }
    for object in objects {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 64,
            ..WorkUsage::default()
        })?;
        if !confirmed_ingress_membership(connection, ingress.operation, *object)? {
            return Err(IndexError::Conflict);
        }
    }
    Ok(markers)
}

/// Measure the fixed route-seal ingress before loading and decoding its
/// bytes. The caller holds one SQLite snapshot for both reads.
pub(super) fn read_protected_route_ingress(
    connection: &Connection,
    key: OperationKey,
    budget: &mut WorkBudget,
) -> Result<Option<IngressRow>, IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        bytes: 128,
        ..WorkUsage::default()
    })?;
    let sizes: Option<(Option<i64>, Option<i64>, Option<i64>)> = connection
        .query_row(
            "SELECT CASE WHEN typeof(workspace)='blob' AND length(workspace) BETWEEN 1 AND 4096 THEN length(workspace) END,
                    CASE WHEN typeof(payload)='blob' AND length(payload) BETWEEN 1 AND 4096 THEN length(payload) END,
                    CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN 32 END
             FROM native_ingress_v1 WHERE operation=?1",
            params![operation_key(key)?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((workspace, payload, digest)) = sizes else {
        return Ok(None);
    };
    let workspace = uint(workspace.ok_or(IndexError::DigestConflict)?)?;
    let payload = uint(payload.ok_or(IndexError::DigestConflict)?)?;
    let digest = uint(digest.ok_or(IndexError::DigestConflict)?)?;
    let bytes = workspace
        .checked_add(payload)
        .and_then(|value| value.checked_add(digest))
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        reads: 1,
        bytes,
        ..WorkUsage::default()
    })?;
    let _row_memory = budget
        .reserve_memory(
            bytes
                .checked_mul(8)
                .and_then(|value| value.checked_add(4096))
                .ok_or(IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
        .ok_or(IndexError::ResourceLimited)?;
    let row = read_ingress(connection, key)?.ok_or(IndexError::DigestConflict)?;
    Ok(Some(row))
}

pub(super) fn require_open_protected_route_ingress(
    connection: &Connection,
    ingress: IngressRef,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let row = read_protected_route_ingress(connection, ingress.operation, budget)?
        .ok_or(IndexError::FenceLost)?;
    if row.stage != ingress {
        return Err(IndexError::FenceLost);
    }
    if row.sealed.is_some() {
        return Err(IndexError::Conflict);
    }
    Ok(())
}

pub(super) fn protected_route_operation_exists(
    connection: &Connection,
    key: OperationKey,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM native_operation_v1 WHERE operation=?1)",
            params![operation_key(key)?],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))
}

fn transfer_and_retire_route_ingress(
    connection: &Connection,
    primary: OperationKey,
    ingress: IngressRef,
    has_native_origin: bool,
    objects: &[ObjectRef],
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let ingress_key = operation_key(ingress.operation)?;
    let primary_key = operation_key(primary)?;
    for object in objects {
        budget.checkpoint()?;
        let child = object_key(*object);
        budget.charge(WorkUsage {
            reads: 2,
            writes: 2,
            bytes: 192,
            mutations: 2,
        })?;
        let expected = fields_digest(b"qidx.native.opobject.v1", &(&primary_key, &child))?;
        let _inserted = connection
            .execute(
                "INSERT INTO native_operation_object_v1(operation_id,object,row_digest)
             VALUES ((SELECT operation_id FROM native_operation_v1 WHERE operation=?1),?2,?3)
             ON CONFLICT(operation_id,object) DO NOTHING",
                params![&primary_key, &child, &expected],
            )
            .map_err(|error| sql(&error))?;
        let valid: Option<bool> = connection.query_row(
            "SELECT row_digest FROM native_operation_object_v1 WHERE operation_id=(SELECT operation_id FROM native_operation_v1 WHERE operation=?1) AND object=?2",
            params![&primary_key, &child],
            |row| Ok(matches!(row.get_ref(0)?, ValueRef::Blob(digest) if digest == expected)),
        ).optional().map_err(|error| sql(&error))?;
        if valid != Some(true) {
            return Err(IndexError::DigestConflict);
        }
        reclaim::adopt_if_known(connection, *object)?;
    }
    if has_native_origin {
        // The route now holds the complete transferred closure through its
        // registered child edges. Retire the loan with the normal bounded GC
        // path: unrelated staged objects receive no permanent BOOT custody.
        return delete_ingress(connection, ingress.operation, budget);
    }
    let mut statement = connection
        .prepare("SELECT object FROM native_ingress_intent_v1 WHERE operation=?1 ORDER BY object")
        .map_err(|error| sql(&error))?;
    let mut rows = statement
        .query(params![&ingress_key])
        .map_err(|error| sql(&error))?;
    let mut intent_count = 0usize;
    while let Some(row) = rows.next().map_err(|error| sql(&error))? {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 64,
            ..WorkUsage::default()
        })?;
        let ValueRef::Blob(object) = row.get_ref(0).map_err(|error| sql(&error))? else {
            return Err(IndexError::DigestConflict);
        };
        let digest = <[u8; 32]>::try_from(object).map_err(|_width| IndexError::DigestConflict)?;
        if objects.binary_search(&ObjectRef(Digest(digest))).is_err() {
            return Err(IndexError::DigestConflict);
        }
        intent_count = intent_count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
    }
    drop(rows);
    drop(statement);
    budget.charge(WorkUsage {
        writes: u64::try_from(intent_count + objects.len() + 1)
            .map_err(|_| IndexError::ResourceLimited)?,
        mutations: u64::try_from(intent_count + objects.len() + 1)
            .map_err(|_| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let removed_intents = connection
        .execute(
            "DELETE FROM native_ingress_intent_v1 WHERE operation=?1",
            params![&ingress_key],
        )
        .map_err(|error| sql(&error))?;
    let removed_objects = connection
        .execute(
            "DELETE FROM native_ingress_object_v1 WHERE operation=?1",
            params![&ingress_key],
        )
        .map_err(|error| sql(&error))?;
    if removed_intents != intent_count || removed_objects != objects.len() {
        return Err(IndexError::DigestConflict);
    }
    exactly_one(
        connection
            .execute(
                "DELETE FROM native_ingress_v1 WHERE operation=?1",
                params![&ingress_key],
            )
            .map_err(|error| sql(&error))?,
        "retire protected Source route ingress",
    )?;
    Ok(())
}

impl SqliteCatalog {
    /// Select an existing published manifest and protect only its boot roots.
    /// Selection is atomic with custody; it does not authenticate disk bodies.
    pub fn select_native_boot_manifest(
        &self,
        key: OperationKey,
        budget: &mut WorkBudget,
    ) -> Result<NativeBootManifest, IndexError> {
        self.select_native_boot_manifest_impl(key, budget, false, None, None, None)
    }

    /// Candidate selection requires the current Published view and quiescence,
    /// including on retry. The checks and selection run in the same write
    /// transaction before any custody mutation.
    pub fn select_native_boot_manifest_if_current(
        &self,
        key: OperationKey,
        budget: &mut WorkBudget,
    ) -> Result<NativeBootManifest, IndexError> {
        self.select_native_boot_manifest_impl(key, budget, true, None, None, None)
    }

    /// The policy object must already be registered. This stores a reference,
    /// not retirement authority; installation reauthenticates predecessor and
    /// signed activation evidence before any request gate may use it.
    pub fn select_native_boot_manifest_with_retired_policy_if_current(
        &self,
        key: OperationKey,
        policy: RetiredProjectionPolicyRefV1,
        budget: &mut WorkBudget,
    ) -> Result<NativeBootManifest, IndexError> {
        policy
            .validate_v1()
            .map_err(|reason| IndexError::InvalidInput(reason.into()))?;
        self.select_native_boot_manifest_impl(key, budget, true, None, Some(policy), None)
    }

    /// Map-only predecessor: stage just the policy object and transfer its
    /// ingress to BOOT without inventing a Source route or workspace.
    pub fn select_native_boot_manifest_with_retired_policy_ingress_if_current(
        &self,
        key: OperationKey,
        ingress: IngressRef,
        policy: RetiredProjectionPolicyRefV1,
        budget: &mut WorkBudget,
    ) -> Result<NativeBootManifest, IndexError> {
        policy
            .validate_v1()
            .map_err(|reason| IndexError::InvalidInput(reason.into()))?;
        self.select_native_boot_manifest_impl(key, budget, true, None, Some(policy), Some(ingress))
    }

    /// Select a complete, physically staged Source route set in the same
    /// transaction as its actual Published/current heads and RepoMap activations.
    /// The caller must independently authenticate the producer-signed route
    /// list and physical immutable-object bytes before invoking this method.
    pub fn select_native_boot_manifest_with_protected_routes_if_current(
        &self,
        key: OperationKey,
        route_object: ObjectRef,
        routes: &NativeProtectedSourceRoutesV1,
        ingress: IngressRef,
        budget: &mut WorkBudget,
    ) -> Result<NativeBootManifest, IndexError> {
        self.select_native_boot_manifest_impl(
            key,
            budget,
            true,
            Some((route_object, routes, ingress)),
            None,
            None,
        )
    }

    pub fn select_native_boot_manifest_with_protected_routes_and_retired_policy_if_current(
        &self,
        key: OperationKey,
        route_object: ObjectRef,
        routes: &NativeProtectedSourceRoutesV1,
        ingress: IngressRef,
        policy: RetiredProjectionPolicyRefV1,
        budget: &mut WorkBudget,
    ) -> Result<NativeBootManifest, IndexError> {
        policy
            .validate_v1()
            .map_err(|reason| IndexError::InvalidInput(reason.into()))?;
        self.select_native_boot_manifest_impl(
            key,
            budget,
            true,
            Some((route_object, routes, ingress)),
            Some(policy),
            None,
        )
    }

    fn select_native_boot_manifest_impl(
        &self,
        key: OperationKey,
        budget: &mut WorkBudget,
        require_current: bool,
        protected_routes: Option<(ObjectRef, &NativeProtectedSourceRoutesV1, IngressRef)>,
        retired_projection_policy: Option<RetiredProjectionPolicyRefV1>,
        retired_policy_ingress: Option<IngressRef>,
    ) -> Result<NativeBootManifest, IndexError> {
        budget.checkpoint()?;
        let _scratch = budget.reserve_memory(SCRATCH_BYTES, MemoryLifetime::Temporary)?;
        let mut guard = self.lock_native_budget(budget)?;
        let tx = begin(&mut guard)?;
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 4160,
            ..WorkUsage::default()
        })?;
        let previous = read_stored(&tx)?;
        if let Some(previous) = previous.as_ref() {
            validate_custody(&tx, previous, budget)?;
        }
        let (immutable, identity) = if protected_routes.is_some() {
            with_protected_operation(&tx, key, budget, |row, _| {
                let OperationStatus::Published(identity) = &row.status else {
                    return Err(IndexError::Conflict);
                };
                Ok((row.immutable, *identity))
            })?
        } else {
            charge_operation(&tx, key, budget)?;
            let row = read_operation(&tx, key)?.ok_or(IndexError::SnapshotUnavailable)?;
            let OperationStatus::Published(identity) = row.status else {
                return Err(IndexError::Conflict);
            };
            (row.immutable, identity)
        };
        let protected = protected_routes
            .map(|(object, routes, ingress)| {
                let markers = validate_route_ingress(
                    &tx,
                    key,
                    ingress,
                    object,
                    routes,
                    retired_projection_policy.map(|policy| policy.object),
                    budget,
                )?;
                let mut route = validate_protected_routes(
                    &tx,
                    key,
                    object,
                    routes,
                    true,
                    Some(ingress),
                    budget,
                )?;
                if let Some(origin) = routes.native_origin.as_ref() {
                    route.marker_anchor = retention::compact_marker_anchor_at_seal(
                        &tx,
                        object,
                        origin,
                        &markers.objects,
                        budget,
                    )?;
                }
                Ok::<_, IndexError>((route, ingress, routes.native_origin.is_some(), markers))
            })
            .transpose()?;
        let route_ref = protected.as_ref().map(|(route, ..)| *route);
        if let Some(ingress) = retired_policy_ingress {
            let policy = retired_projection_policy.ok_or(IndexError::DigestConflict)?;
            if ingress.operation == key
                || ingress.operation.workspace != key.workspace
                || ingress.operation.epoch != key.epoch
                || protected_route_operation_exists(&tx, ingress.operation, budget)?
            {
                return Err(IndexError::Conflict);
            }
            require_open_protected_route_ingress(&tx, ingress, budget)?;
            let count: i64 = tx
                .query_row(
                    "SELECT count(*) FROM native_ingress_object_v1 WHERE operation=?1",
                    params![operation_key(ingress.operation)?],
                    |row| row.get(0),
                )
                .map_err(|error| sql(&error))?;
            if count != 1 || !confirmed_ingress_membership(&tx, ingress.operation, policy.object)? {
                return Err(IndexError::Conflict);
            }
            // Retire the one-object ingress in this transaction before the
            // ordinary no-route quiescence check. Any later failure rolls back.
            transfer_and_retire_route_ingress(&tx, key, ingress, false, &[policy.object], budget)?;
        }
        let selected = if let Some(previous) = previous.as_ref().filter(|value| {
            value.operation.key == key
                && value.protected_source_routes == route_ref
                && value.retired_projection_policy == retired_projection_policy
        }) {
            if previous.operation != immutable || previous.publication.identity != identity {
                return Err(IndexError::Storage(
                    "selected boot operation changed".into(),
                ));
            }
            previous.clone()
        } else {
            let view = load_publication(&tx, identity, budget)?;
            NativeBootManifest {
                format_version: if retired_projection_policy.is_some() {
                    NativeBootManifest::FORMAT_VERSION_V2
                } else {
                    NativeBootManifest::FORMAT_VERSION_V1
                },
                operation: immutable,
                publication: view,
                protected_source_routes: route_ref,
                retired_projection_policy,
            }
        };
        if require_current {
            let current = retention::charged_current_view(&tx, key.workspace, budget)?
                .ok_or(IndexError::SnapshotUnavailable)?;
            if current != selected.publication {
                return Err(IndexError::Conflict);
            }
            if route_ref.is_none() {
                boot_quiescence::check_quiescent_rows(
                    &tx,
                    key.workspace,
                    current.identity,
                    budget,
                )?;
            }
        }
        if let Some((_, ingress, has_native_origin, markers)) = protected {
            transfer_and_retire_route_ingress(
                &tx,
                key,
                ingress,
                has_native_origin,
                &markers.objects,
                budget,
            )?;
        }
        protect(&tx, &selected, budget)?;
        if previous.as_ref() != Some(&selected) {
            requeue_previous(&tx, previous.as_ref(), budget)?;
            let payload = serde_json::to_vec(&selected)
                .map_err(|error| IndexError::Storage(format!("encode boot manifest: {error}")))?;
            if payload.is_empty() || payload.len() > MAX_BYTES {
                return Err(IndexError::ResourceLimited);
            }
            let nonce: [u8; 32] = super::capture_format::verify_stamp(&tx, &self.path)
                .map_err(|error| IndexError::Storage(error.to_string()))?;
            budget.charge(WorkUsage {
                writes: 1,
                bytes: u64::try_from(payload.len())
                    .map_err(|_error| IndexError::ResourceLimited)?
                    .checked_add(32)
                    .ok_or(IndexError::ResourceLimited)?,
                mutations: 1,
                ..WorkUsage::default()
            })?;
            exactly_one(tx.execute("UPDATE native_capture_format_v1 SET boot_manifest=?1,boot_manifest_digest=?2 WHERE singleton=1",
                params![&payload,checksum(&nonce,&payload).as_slice()]).map_err(|error| sql(&error))?, "select boot manifest")?;
        }
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(selected)
    }

    /// Read the selected logical binding; physical/source authentication is
    /// still required by the offline verifier before boot or installation.
    pub fn native_boot_manifest(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<Option<NativeBootManifest>, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 4160,
            ..WorkUsage::default()
        })?;
        let _scratch = budget.reserve_memory(SCRATCH_BYTES, MemoryLifetime::Temporary)?;
        let guard = self.lock_native_budget(budget)?;
        let selected = read_stored(&guard)?;
        if let Some(selected) = selected.as_ref() {
            validate_custody(&guard, selected, budget)?;
        }
        budget.checkpoint()?;
        drop(guard);
        Ok(selected)
    }

    /// Resolve a selected exact head on the serving Catalog connection.
    /// The caller authenticates the route object's physical payload and
    /// children; this method rechecks its selected Catalog relation while the
    /// serving connection's mutex prevents a concurrent publication.
    pub fn resolve_selected_native_protected_source_head_v1(
        &self,
        expected_manifest: &NativeBootManifest,
        routes: &NativeProtectedSourceRoutesV1,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        budget: &mut WorkBudget,
    ) -> Result<CatalogView, IndexError> {
        let guard = self.lock_native_budget(budget)?;
        if read_stored(&guard)?.as_ref() != Some(expected_manifest) {
            return Err(IndexError::DigestConflict);
        }
        validate_custody(&guard, expected_manifest, budget)?;
        let selected = expected_manifest
            .protected_source_routes
            .ok_or(IndexError::SnapshotUnavailable)?;
        if usize::try_from(selected.count).ok() != Some(routes.heads.len()) {
            return Err(IndexError::DigestConflict);
        }
        let _binding = validate_protected_routes(
            &guard,
            expected_manifest.operation.key,
            selected.object,
            routes,
            false,
            None,
            budget,
        )?;
        let publication = routes
            .heads
            .iter()
            .find(|head| &head.repo_id == repo_id && &head.revision_id == revision_id)
            .map(|head| head.publication.clone())
            .ok_or(IndexError::SnapshotUnavailable)?;
        budget.checkpoint()?;
        Ok(publication)
    }

    /// Resolve the exact imported RepoMap view selected by the protected
    /// route, including a view retained after its workspace head advances.
    pub fn resolve_selected_native_protected_repomap_head_v1(
        &self,
        expected_manifest: &NativeBootManifest,
        routes: &NativeProtectedSourceRoutesV1,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        budget: &mut WorkBudget,
    ) -> Result<CatalogView, IndexError> {
        let guard = self.lock_native_budget(budget)?;
        if read_stored(&guard)?.as_ref() != Some(expected_manifest) {
            return Err(IndexError::DigestConflict);
        }
        validate_custody(&guard, expected_manifest, budget)?;
        let selected = expected_manifest
            .protected_source_routes
            .ok_or(IndexError::SnapshotUnavailable)?;
        if usize::try_from(selected.count).ok() != Some(routes.heads.len()) {
            return Err(IndexError::DigestConflict);
        }
        let _binding = validate_protected_routes(
            &guard,
            expected_manifest.operation.key,
            selected.object,
            routes,
            false,
            None,
            budget,
        )?;
        let publication = routes
            .repomap_heads
            .iter()
            .find(|head| &head.repo_id == repo_id && &head.revision_id == revision_id)
            .map(|head| head.publication.clone())
            .ok_or(IndexError::SnapshotUnavailable)?;
        budget.checkpoint()?;
        Ok(publication)
    }

    /// Explicitly retire selection, atomically re-enabling bounded terminal
    /// cleanup. A cancelled/failed transaction retains the complete old custody.
    pub fn clear_native_boot_manifest(&self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 4160,
            ..WorkUsage::default()
        })?;
        let _scratch = budget.reserve_memory(SCRATCH_BYTES, MemoryLifetime::Temporary)?;
        let mut guard = self.lock_native_budget(budget)?;
        let tx = begin(&mut guard)?;
        let previous = read_stored(&tx)?;
        if let Some(previous) = previous.as_ref() {
            validate_custody(&tx, previous, budget)?;
        }
        if previous.is_some() {
            requeue_previous(&tx, previous.as_ref(), budget)?;
            budget.charge(WorkUsage {
                writes: 1,
                mutations: 1,
                ..WorkUsage::default()
            })?;
            exactly_one(tx.execute("UPDATE native_capture_format_v1 SET boot_manifest=NULL,boot_manifest_digest=NULL WHERE singleton=1",[])
                .map_err(|error| sql(&error))?,"clear boot manifest")?;
        }
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(guard);
        Ok(())
    }
}
