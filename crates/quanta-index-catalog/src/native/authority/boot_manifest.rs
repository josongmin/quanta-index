//! Selected boot custody uses the existing operation-object relation.
//! It preserves the original request and source/profile, not old search results.

use quanta_index_contract::{
    RepoId, RevisionId,
    index::{
        MAX_NATIVE_PROTECTED_REPOMAP_HEADS_V1, MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1,
        NativeProtectedRepoMapHeadV1, NativeProtectedSourceRoutesV1,
    },
};
use quanta_index_core::index::{
    CatalogView, Digest, ImmutableOperation, IndexError, IngressRef, MemoryLease, MemoryLifetime,
    ObjectRef, OperationKey, OperationStatus, ViewRef, WorkBudget, WorkUsage,
};
use rusqlite::{Connection, OptionalExtension as _, params, types::ValueRef};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::{
    SqliteCatalog, begin, check_ingress, confirmed_ingress_membership, delete_ingress, exactly_one,
    fields_digest, int, object_key, operation_key, read_operation, read_view_row, reclaim,
    require_object, retention, sql, uint, workspace_key, write_operation,
};

const MAX_BYTES: usize = 4096;
const SCRATCH_BYTES: u64 = 16_384;
// Existing registration validation reads at most a 1 MiB payload and a
// 4096-child edge ledger. Retain its whole bounded traversal envelope here;
// this is scoped logical admission, not SQLite allocator/RSS qualification.
const REGISTRATION_SCRATCH_BYTES: u64 = 4_259_840;
const DOMAIN: &[u8] = b"qidx.native.boot-manifest.v1\0";

/// Catalog-issued binding to one original published operation. This is not a
/// producer completion seal or proof of the referenced physical object bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeBootManifest {
    pub operation: ImmutableOperation,
    /// Canonical publication metadata permits digest validation after its view
    /// row retires. Its other query roots are not protected by this selection.
    pub publication: CatalogView,
    /// An immutable, physically verified route object holds all additional
    /// source/revision heads and their original boot roots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protected_source_routes: Option<NativeBootSourceRouteRefV1>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeBootSourceRouteRefV1 {
    pub object: ObjectRef,
    pub count: u32,
}

impl NativeBootManifest {
    /// Exact roots protected by Catalog selection and authenticated at boot.
    /// Historical query projection roots deliberately remain outside this set.
    #[must_use]
    pub const fn protected_roots(&self) -> [Option<ObjectRef>; 5] {
        [
            Some(self.operation.request),
            Some(self.publication.profile),
            self.publication.source.owners.0,
            self.publication.source.paths.0,
            self.publication.source.frontiers.0,
        ]
    }

    /// Every selected root is retained through the original operation's
    /// custody row. The route object's direct immutable children cover all
    /// additional heads after physical closure verification.
    pub fn all_protected_roots(&self) -> impl Iterator<Item = ObjectRef> + '_ {
        self.protected_roots().into_iter().flatten().chain(
            self.protected_source_routes
                .iter()
                .map(|route| route.object),
        )
    }

    /// Canonical source-root identity, distinct from a mutation/event capture.
    pub fn source_cut_digest(&self) -> Result<Digest, IndexError> {
        quanta_index_contract::native_boot_source_cut_digest_v1(&self.publication.source)
            .map_err(|error| IndexError::Storage(format!("encode boot SourceCut: {error}")))
    }
}

fn checksum(nonce: &[u8], payload: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(DOMAIN);
    hash.update(nonce);
    // The nonce is fixed-width and payload is the only variable-length field.
    hash.update(payload);
    hash.finalize().into()
}

/// Borrow `SQLite`'s fixed/bounded fields; never copy an unbounded stored BLOB.
pub(super) fn read_stored(
    connection: &Connection,
) -> Result<Option<NativeBootManifest>, IndexError> {
    connection
        .query_row(
            "SELECT root_nonce,boot_manifest,boot_manifest_digest
         FROM native_capture_format_v1 WHERE singleton=1",
            [],
            |row| {
                let nonce = row.get_ref(0)?;
                let payload = row.get_ref(1)?;
                let digest = row.get_ref(2)?;
                let ValueRef::Blob(nonce) = nonce else {
                    return Ok(Err(IndexError::Storage("boot nonce type mismatch".into())));
                };
                if nonce.len() != 32 || nonce == [0; 32] {
                    return Ok(Err(IndexError::Storage("boot nonce shape mismatch".into())));
                }
                if matches!((payload, digest), (ValueRef::Null, ValueRef::Null)) {
                    return Ok(Ok(None));
                }
                let (ValueRef::Blob(payload), ValueRef::Blob(digest)) = (payload, digest) else {
                    return Ok(Err(IndexError::Storage(
                        "boot manifest type mismatch".into(),
                    )));
                };
                if payload.is_empty() || payload.len() > MAX_BYTES || digest.len() != 32 {
                    return Ok(Err(IndexError::Storage(
                        "boot manifest shape mismatch".into(),
                    )));
                }
                if digest != checksum(nonce, payload) {
                    return Ok(Err(IndexError::Storage(
                        "boot manifest digest mismatch".into(),
                    )));
                }
                let selected: NativeBootManifest = match serde_json::from_slice(payload) {
                    Ok(value) => value,
                    Err(error) => {
                        return Ok(Err(IndexError::Storage(format!(
                            "decode boot manifest: {error}"
                        ))));
                    }
                };
                if selected.protected_source_routes.is_some_and(|route| {
                    usize::try_from(route.count)
                        .ok()
                        .is_none_or(|count| count > MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1)
                }) {
                    return Ok(Err(IndexError::Storage(
                        "boot Source route count is invalid".into(),
                    )));
                }
                if selected.operation.key.workspace != selected.publication.identity.workspace
                    || selected.operation.key.epoch != selected.publication.identity.epoch
                {
                    return Ok(Err(IndexError::Storage(
                        "boot publication scope mismatch".into(),
                    )));
                }
                if selected
                    .publication
                    .identity_digest()
                    .map_err(|error| IndexError::Storage(error.to_string()))
                    != Ok(selected.publication.identity.digest)
                {
                    return Ok(Err(IndexError::Storage(
                        "boot publication identity digest mismatch".into(),
                    )));
                }
                Ok(Ok(Some(selected)))
            },
        )
        .map_err(|error| sql(&error))?
}

pub(super) fn protected_roots(
    connection: &Connection,
    key: OperationKey,
    budget: &mut WorkBudget,
) -> Result<[Option<ObjectRef>; 6], IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        bytes: 4160,
        ..WorkUsage::default()
    })?;
    let _scratch = budget.reserve_memory(SCRATCH_BYTES, MemoryLifetime::Temporary)?;
    let selected = read_stored(connection)?.filter(|selected| selected.operation.key == key);
    if let Some(selected) = selected.as_ref() {
        validate_custody(connection, selected, budget)?;
    }
    Ok(selected.map_or([None; 6], |selected| {
        let mut roots = [None; 6];
        roots[..5].copy_from_slice(&selected.protected_roots());
        roots[5] = selected.protected_source_routes.map(|route| route.object);
        roots
    }))
}

pub(super) fn validate_custody(
    connection: &Connection,
    selected: &NativeBootManifest,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    charge_operation(connection, selected.operation.key, budget)?;
    let original = read_operation(connection, selected.operation.key)?
        .ok_or(IndexError::SnapshotUnavailable)?;
    if original.immutable != selected.operation
        || original.status != OperationStatus::Published(selected.publication.identity)
    {
        return Err(IndexError::Storage(
            "selected boot original receipt mismatch".into(),
        ));
    }
    let operation = operation_key(selected.operation.key)?;
    for object in selected.all_protected_roots() {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 64,
            ..WorkUsage::default()
        })?;
        let object = object_key(object);
        let expected = fields_digest(b"qidx.native.opobject.v1", &(&operation, &object))?;
        let valid: Option<bool> = connection.query_row(
            "SELECT row_digest FROM native_operation_object_v1 WHERE operation_id=(SELECT operation_id FROM native_operation_v1 WHERE operation=?1) AND object=?2",
            params![&operation,&object], |row| Ok(matches!(row.get_ref(0)?,ValueRef::Blob(digest) if digest == expected)),
        ).optional().map_err(|error| sql(&error))?;
        if valid != Some(true) {
            return Err(IndexError::Storage(
                "selected boot root custody missing or corrupt".into(),
            ));
        }
    }
    Ok(())
}

/// Normal references were already searched. A root named by the authenticated
/// selection must never become an orphan merely because its custody row vanished.
pub(super) fn refuse_selected_orphan(
    connection: &Connection,
    object: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        bytes: 4160,
        ..WorkUsage::default()
    })?;
    let _scratch = budget.reserve_memory(SCRATCH_BYTES, MemoryLifetime::Temporary)?;
    if read_stored(connection)?
        .is_some_and(|selected| selected.all_protected_roots().any(|root| root == object))
    {
        return Err(IndexError::Storage(
            "selected boot root lost its custody relation".into(),
        ));
    }
    Ok(())
}

pub(super) fn charge_operation(
    connection: &Connection,
    key: OperationKey,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let sizes: Option<(i64, i64, i64)> = connection.query_row(
        "SELECT CASE WHEN typeof(workspace)='blob' AND length(workspace) BETWEEN 1 AND 4096 THEN length(workspace) END,
          CASE WHEN typeof(payload)='blob' AND length(payload) BETWEEN 1 AND 4096 THEN length(payload) END,
          CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN 32 END
         FROM native_operation_v1 WHERE operation=?1",
        params![operation_key(key)?], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).optional().map_err(|error| sql(&error))?;
    let (workspace, payload, digest) = sizes.ok_or(IndexError::SnapshotUnavailable)?;
    let workspace = uint(workspace)?;
    let payload = uint(payload)?;
    let digest = uint(digest)?;
    let bytes = workspace
        .checked_add(payload)
        .and_then(|bytes| bytes.checked_add(digest))
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        reads: 1,
        bytes,
        ..WorkUsage::default()
    })
}

pub(super) fn load_publication(
    connection: &Connection,
    identity: ViewRef,
    budget: &mut WorkBudget,
) -> Result<super::CatalogView, IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let scope = workspace_key(identity.workspace)?;
    let sizes: Option<(i64,i64)> = connection.query_row(
        "SELECT CASE WHEN typeof(payload)='blob' AND length(payload) BETWEEN 1 AND 4096 THEN length(payload) END,
          CASE WHEN typeof(identity_digest)='blob' AND length(identity_digest)=32
                    AND typeof(row_digest)='blob' AND length(row_digest)=32 THEN 64 END
         FROM native_view_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3",
        params![&scope,int(identity.epoch)?,int(identity.sequence)?],
        |row| Ok((row.get(0)?,row.get(1)?)),
    ).optional().map_err(|error| sql(&error))?;
    let (payload, digests) = sizes.ok_or(IndexError::SnapshotUnavailable)?;
    let (roots, valid): (i64, Option<i64>) = connection
        .query_row(
            "SELECT count(*),min(CASE WHEN typeof(object)='blob' AND length(object)=32
                 AND typeof(row_digest)='blob' AND length(row_digest)=32 THEN 1 ELSE 0 END)
             FROM (SELECT object,row_digest FROM native_view_root_v1
          WHERE workspace=?1 AND epoch=?2 AND sequence=?3 LIMIT 8)",
            params![&scope, int(identity.epoch)?, int(identity.sequence)?],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| sql(&error))?;
    if !(1..=7).contains(&roots) || valid != Some(1) {
        return Err(IndexError::Storage(
            "boot publication root count mismatch".into(),
        ));
    }
    // The initialization transaction prevents the preflighted row set changing.
    let reads = uint(roots)?
        .checked_add(2)
        .ok_or(IndexError::ResourceLimited)?;
    let payload = uint(payload)?;
    let digests = uint(digests)?;
    let roots = uint(roots)?
        .checked_mul(64)
        .ok_or(IndexError::ResourceLimited)?;
    let bytes = payload
        .checked_add(digests)
        .and_then(|bytes| bytes.checked_add(roots))
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        reads,
        bytes,
        ..WorkUsage::default()
    })?;
    read_view_row(connection, identity)
}

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
        charge_operation(connection, previous.operation.key, budget)?;
        let row = read_operation(connection, previous.operation.key)?
            .ok_or(IndexError::SnapshotUnavailable)?;
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
        write_operation(connection, &row, false)?;
    }
    Ok(())
}

fn verify_repomap_source_publication(
    connection: &Connection,
    head: &NativeProtectedRepoMapHeadV1,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if let Some(source_operation) = &head.source_operation {
        charge_operation(connection, source_operation.key, budget)?;
        let source = read_operation(connection, source_operation.key)?
            .ok_or(IndexError::SnapshotUnavailable)?;
        if source.immutable != *source_operation
            || source.status != OperationStatus::Published(head.source_publication.identity)
            || source.published_view.as_ref() != Some(&head.source_publication)
        {
            return Err(IndexError::DigestConflict);
        }
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
    let payload_bytes = u64::try_from(
        routes
            .encoded_payload_len()
            .map_err(|_| IndexError::DigestConflict)?,
    )
    .map_err(|_| IndexError::ResourceLimited)?;
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
    budget.charge(WorkUsage {
        bytes: payload_bytes,
        ..WorkUsage::default()
    })?;
    if routes
        .object_ref()
        .map_err(|_| IndexError::DigestConflict)?
        != route_object
    {
        return Err(IndexError::DigestConflict);
    }
    let count = u32::try_from(routes.heads.len()).map_err(|_| IndexError::ResourceLimited)?;
    for head in &routes.heads {
        budget.checkpoint()?;
        charge_operation(connection, head.source_operation.key, budget)?;
        let source = read_operation(connection, head.source_operation.key)?
            .ok_or(IndexError::SnapshotUnavailable)?;
        if source.immutable != head.source_operation
            || source.status != OperationStatus::Published(head.source_publication.identity)
            || source.published_view.as_ref() != Some(&head.source_publication)
            || head.source_publication.source != head.publication.source
        {
            return Err(IndexError::DigestConflict);
        }
        charge_operation(connection, head.operation.key, budget)?;
        let operation = read_operation(connection, head.operation.key)?
            .ok_or(IndexError::SnapshotUnavailable)?;
        if operation.immutable != head.operation
            || operation.status != OperationStatus::Published(head.publication.identity)
            || operation.published_view.as_ref() != Some(&head.publication)
            || retention::charged_exact_view(connection, head.publication.identity, budget)?
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
    if require_current {
        charge_operation(connection, primary, budget)?;
        let (_primary_operation, primary_publication) =
            super::read_only::read_published_operation(connection, primary, budget)?;
        if retention::charged_current_view(connection, primary.workspace, budget)?.as_ref()
            != Some(&primary_publication)
        {
            return Err(IndexError::Conflict);
        }
        super::read_only::check_quiescent_protected_routes(
            connection,
            primary_publication.identity,
            routes,
            allowed_ingress,
            budget,
        )?;
    }
    for head in &routes.repomap_heads {
        budget.checkpoint()?;
        verify_repomap_source_publication(connection, head, budget)?;
        charge_operation(connection, head.operation.key, budget)?;
        let operation = read_operation(connection, head.operation.key)?
            .ok_or(IndexError::SnapshotUnavailable)?;
        if operation.immutable != head.operation
            || operation.status != OperationStatus::Published(head.publication.identity)
            || operation.published_view.as_ref() != Some(&head.publication)
            || retention::charged_exact_view(connection, head.publication.identity, budget)?
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
    require_object(connection, route_object)?;
    let registration = super::object_install::read_metadata(
        connection,
        route_object,
        super::object_install::MetadataLedger::Registered,
        || IndexError::DigestConflict,
        |bytes| {
            budget.charge(WorkUsage {
                reads: 1,
                bytes,
                ..WorkUsage::default()
            })
        },
    )?
    .ok_or(IndexError::SnapshotUnavailable)?;
    if registration.children != routes.children().map_err(|_| IndexError::DigestConflict)? {
        return Err(IndexError::DigestConflict);
    }
    Ok(NativeBootSourceRouteRefV1 {
        object: route_object,
        count,
    })
}

struct RouteMarkerRefs {
    objects: Vec<ObjectRef>,
    _memory: Option<MemoryLease>,
}

fn route_marker_refs(
    routes: &NativeProtectedSourceRoutesV1,
    route_object: ObjectRef,
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
    budget: &mut WorkBudget,
) -> Result<RouteMarkerRefs, IndexError> {
    if ingress.operation == primary
        || ingress.operation.workspace != primary.workspace
        || ingress.operation.epoch != primary.epoch
        || read_operation(connection, ingress.operation)?.is_some()
        || check_ingress(connection, ingress)?.sealed.is_some()
    {
        return Err(IndexError::Conflict);
    }
    let markers = route_marker_refs(routes, route_object, budget)?;
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
        self.select_native_boot_manifest_impl(key, budget, false, None)
    }

    /// Candidate selection requires the current Published view and quiescence,
    /// including on retry. The checks and selection run in the same write
    /// transaction before any custody mutation.
    pub fn select_native_boot_manifest_if_current(
        &self,
        key: OperationKey,
        budget: &mut WorkBudget,
    ) -> Result<NativeBootManifest, IndexError> {
        self.select_native_boot_manifest_impl(key, budget, true, None)
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
        )
    }

    fn select_native_boot_manifest_impl(
        &self,
        key: OperationKey,
        budget: &mut WorkBudget,
        require_current: bool,
        protected_routes: Option<(ObjectRef, &NativeProtectedSourceRoutesV1, IngressRef)>,
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
        charge_operation(&tx, key, budget)?;
        let row = read_operation(&tx, key)?.ok_or(IndexError::SnapshotUnavailable)?;
        let OperationStatus::Published(identity) = row.status else {
            return Err(IndexError::Conflict);
        };
        let protected = protected_routes
            .map(|(object, routes, ingress)| {
                let markers = validate_route_ingress(&tx, key, ingress, object, routes, budget)?;
                let route = validate_protected_routes(
                    &tx,
                    key,
                    object,
                    routes,
                    true,
                    Some(ingress),
                    budget,
                )?;
                Ok::<_, IndexError>((route, ingress, routes.native_origin.is_some(), markers))
            })
            .transpose()?;
        let route_ref = protected.as_ref().map(|(route, ..)| *route);
        let selected = if let Some(previous) = previous.as_ref().filter(|value| {
            value.operation.key == key && value.protected_source_routes == route_ref
        }) {
            if previous.operation != row.immutable || previous.publication.identity != identity {
                return Err(IndexError::Storage(
                    "selected boot operation changed".into(),
                ));
            }
            previous.clone()
        } else {
            let view = load_publication(&tx, identity, budget)?;
            NativeBootManifest {
                operation: row.immutable,
                publication: view,
                protected_source_routes: route_ref,
            }
        };
        if require_current {
            let current = retention::charged_current_view(&tx, key.workspace, budget)?
                .ok_or(IndexError::SnapshotUnavailable)?;
            if current != selected.publication {
                return Err(IndexError::Conflict);
            }
            if route_ref.is_none() {
                super::read_only::check_quiescent_rows(
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
