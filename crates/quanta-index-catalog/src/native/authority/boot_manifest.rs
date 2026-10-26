//! Selected boot custody uses the existing operation-object relation.
//! It preserves the original request and source/profile, not old search results.

use quanta_index_core::index::{
    CatalogView, Digest, ImmutableOperation, IndexError, MemoryLifetime, ObjectRef, OperationKey,
    OperationStatus, ViewRef, WorkBudget, WorkUsage,
};
use rusqlite::{Connection, OptionalExtension as _, params, types::ValueRef};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::{
    SqliteCatalog, begin, exactly_one, fields_digest, int, object_key, operation_key,
    read_operation, read_view_row, reclaim, require_object, sql, uint, workspace_key,
    write_operation,
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
) -> Result<[Option<ObjectRef>; 5], IndexError> {
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
    Ok(selected.map_or([None; 5], |selected| selected.protected_roots()))
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
    for object in selected.protected_roots().into_iter().flatten() {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 64,
            ..WorkUsage::default()
        })?;
        let object = object_key(object);
        let expected = fields_digest(b"qidx.native.opobject.v1", &(&operation, &object))?;
        let valid: Option<bool> = connection.query_row(
            "SELECT row_digest FROM native_operation_object_v1 WHERE operation=?1 AND object=?2",
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
        .is_some_and(|selected| selected.protected_roots().contains(&Some(object)))
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
    for object in selected.protected_roots().into_iter().flatten() {
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
                "INSERT INTO native_operation_object_v1(operation,object,row_digest)
             VALUES (?1,?2,?3) ON CONFLICT(operation,object) DO NOTHING",
                params![
                    &operation,
                    &object,
                    fields_digest(b"qidx.native.opobject.v1", &(&operation, &object))?
                ],
            )
            .map_err(|error| sql(&error))?;
        let expected = fields_digest(b"qidx.native.opobject.v1", &(&operation, &object))?;
        let valid: bool = connection.query_row(
            "SELECT row_digest FROM native_operation_object_v1 WHERE operation=?1 AND object=?2",
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

impl SqliteCatalog {
    /// Select an existing published manifest and protect only its boot roots.
    /// Selection is atomic with custody; it does not authenticate disk bodies.
    pub fn select_native_boot_manifest(
        &self,
        key: OperationKey,
        budget: &mut WorkBudget,
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
        let selected = if let Some(previous) =
            previous.as_ref().filter(|value| value.operation.key == key)
        {
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
            }
        };
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
