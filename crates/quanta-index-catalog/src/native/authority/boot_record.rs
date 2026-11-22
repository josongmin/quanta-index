//! BOOT records and their bounded operation and publication reads.

use quanta_index_contract::index::{
    MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1, MAX_NATIVE_PROTECTED_SOURCE_HEADS_V1,
    NativeProtectedSourceRoutesV1,
};
use quanta_index_core::index::{
    CatalogView, Digest, ImmutableOperation, IndexError, MemoryLifetime, ObjectRef, OperationKey,
    OperationStatus, ViewRef, WorkBudget, WorkUsage,
};
use rusqlite::{Connection, OptionalExtension as _, params, types::ValueRef};
use serde::de::Error as _;
use serde::ser::SerializeStruct as _;
use sha2::{Digest as _, Sha256};

use super::{
    decode, fields_digest, int, object_key, operation_key, read_operation, read_view_row, sql,
    uint, workspace_key,
};

pub(super) const MAX_BYTES: usize = 4096;
pub(super) const SCRATCH_BYTES: u64 = 16_384;
// Existing registration validation reads at most a 1 MiB payload and a
// 4096-child edge ledger. Retain its whole bounded traversal envelope here;
// this is scoped logical admission, not SQLite allocator/RSS qualification.
pub(super) const REGISTRATION_SCRATCH_BYTES: u64 = 4_259_840;
const DOMAIN: &[u8] = b"qidx.native.boot-manifest.v1\0";

/// Catalog-issued binding to one original published operation. This is not a
/// producer completion seal or proof of the referenced physical object bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeBootManifest {
    pub operation: ImmutableOperation,
    /// Canonical publication metadata permits digest validation after its view
    /// row retires. Its other query roots are not protected by this selection.
    pub publication: CatalogView,
    /// An immutable, physically verified route object holds all additional
    /// source/revision heads and their original boot roots.
    pub protected_source_routes: Option<NativeBootSourceRouteRefV1>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeBootSourceRouteRefV1 {
    pub object: ObjectRef,
    pub count: u32,
    /// Direct marker carrier derived from the staged Native ancestry at seal.
    pub marker_anchor: Option<ObjectRef>,
}

// Both optional fields are omitted from named records but default when absent
// during decoding. Positional records retain the same trailing-field default.
struct BootFieldSeed(&'static [&'static str]);

impl<'de> serde::de::DeserializeSeed<'de> for BootFieldSeed {
    type Value = &'static str;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        struct FieldVisitor(&'static [&'static str]);

        impl serde::de::Visitor<'_> for FieldVisitor {
            type Value = &'static str;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a boot record field")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                self.0
                    .iter()
                    .copied()
                    .find(|field| *field == value)
                    .ok_or_else(|| E::unknown_field(value, self.0))
            }

            fn visit_bytes<E: serde::de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
                match std::str::from_utf8(value) {
                    Ok(name) => self.visit_str(name),
                    Err(_) => Err(E::unknown_field(&String::from_utf8_lossy(value), self.0)),
                }
            }
        }

        deserializer.deserialize_identifier(FieldVisitor(self.0))
    }
}

impl serde::Serialize for NativeBootManifest {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct(
            "NativeBootManifest",
            if self.protected_source_routes.is_some() {
                3
            } else {
                2
            },
        )?;
        state.serialize_field("operation", &self.operation)?;
        state.serialize_field("publication", &self.publication)?;
        if self.protected_source_routes.is_some() {
            state.serialize_field("protected_source_routes", &self.protected_source_routes)?;
        }
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for NativeBootManifest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &["operation", "publication", "protected_source_routes"];
        struct Record;

        impl<'de> serde::de::Visitor<'de> for Record {
            type Value = NativeBootManifest;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct NativeBootManifest")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let (mut operation, mut publication, mut protected_source_routes) =
                    (None, None, None);
                while let Some(key) = map.next_key_seed(BootFieldSeed(FIELDS))? {
                    match key {
                        "operation" => {
                            if operation.is_some() {
                                return Err(A::Error::duplicate_field(key));
                            }
                            operation = Some(map.next_value()?);
                        }
                        "publication" => {
                            if publication.is_some() {
                                return Err(A::Error::duplicate_field(key));
                            }
                            publication = Some(map.next_value()?);
                        }
                        "protected_source_routes" => {
                            if protected_source_routes.is_some() {
                                return Err(A::Error::duplicate_field(key));
                            }
                            protected_source_routes = Some(map.next_value()?);
                        }
                        _ => unreachable!("closed boot field seed"),
                    }
                }
                Ok(NativeBootManifest {
                    operation: operation.ok_or_else(|| A::Error::missing_field("operation"))?,
                    publication: publication
                        .ok_or_else(|| A::Error::missing_field("publication"))?,
                    protected_source_routes: protected_source_routes.unwrap_or_default(),
                })
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let expected = &"struct NativeBootManifest with 3 elements";
                Ok(NativeBootManifest {
                    operation: seq
                        .next_element()?
                        .ok_or_else(|| A::Error::invalid_length(0, expected))?,
                    publication: seq
                        .next_element()?
                        .ok_or_else(|| A::Error::invalid_length(1, expected))?,
                    protected_source_routes: seq.next_element()?.unwrap_or_default(),
                })
            }
        }

        deserializer.deserialize_struct("NativeBootManifest", FIELDS, Record)
    }
}

impl serde::Serialize for NativeBootSourceRouteRefV1 {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct(
            "NativeBootSourceRouteRefV1",
            if self.marker_anchor.is_some() { 3 } else { 2 },
        )?;
        state.serialize_field("object", &self.object)?;
        state.serialize_field("count", &self.count)?;
        if self.marker_anchor.is_some() {
            state.serialize_field("marker_anchor", &self.marker_anchor)?;
        }
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for NativeBootSourceRouteRefV1 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        const FIELDS: &[&str] = &["object", "count", "marker_anchor"];
        struct Record;

        impl<'de> serde::de::Visitor<'de> for Record {
            type Value = NativeBootSourceRouteRefV1;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("struct NativeBootSourceRouteRefV1")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let (mut object, mut count, mut marker_anchor) = (None, None, None);
                while let Some(key) = map.next_key_seed(BootFieldSeed(FIELDS))? {
                    match key {
                        "object" => {
                            if object.is_some() {
                                return Err(A::Error::duplicate_field(key));
                            }
                            object = Some(map.next_value()?);
                        }
                        "count" => {
                            if count.is_some() {
                                return Err(A::Error::duplicate_field(key));
                            }
                            count = Some(map.next_value()?);
                        }
                        "marker_anchor" => {
                            if marker_anchor.is_some() {
                                return Err(A::Error::duplicate_field(key));
                            }
                            marker_anchor = Some(map.next_value()?);
                        }
                        _ => unreachable!("closed boot field seed"),
                    }
                }
                Ok(NativeBootSourceRouteRefV1 {
                    object: object.ok_or_else(|| A::Error::missing_field("object"))?,
                    count: count.ok_or_else(|| A::Error::missing_field("count"))?,
                    marker_anchor: marker_anchor.unwrap_or_default(),
                })
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let expected = &"struct NativeBootSourceRouteRefV1 with 3 elements";
                Ok(NativeBootSourceRouteRefV1 {
                    object: seq
                        .next_element()?
                        .ok_or_else(|| A::Error::invalid_length(0, expected))?,
                    count: seq
                        .next_element()?
                        .ok_or_else(|| A::Error::invalid_length(1, expected))?,
                    marker_anchor: seq.next_element()?.unwrap_or_default(),
                })
            }
        }

        deserializer.deserialize_struct("NativeBootSourceRouteRefV1", FIELDS, Record)
    }
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

/// Infallible by construction because the nonce and serialized payload are already in memory.
pub(super) fn checksum(nonce: &[u8], payload: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(DOMAIN);
    hash.update(nonce);
    // The nonce is fixed-width and payload is the only variable-length field.
    hash.update(payload);
    hash.finalize().into()
}

/// Bound the fields inside SQLite before selecting BLOBs into its row buffer.
pub(super) fn read_stored(
    connection: &Connection,
) -> Result<Option<NativeBootManifest>, IndexError> {
    connection
        .query_row(
            "SELECT CASE
                      WHEN typeof(root_nonce) != 'blob' THEN 0
                      WHEN length(root_nonce) != 32 THEN X''
                      ELSE root_nonce
                    END,
                    CASE
                      WHEN boot_manifest IS NULL THEN NULL
                      WHEN typeof(boot_manifest) != 'blob' THEN 0
                      WHEN length(boot_manifest) NOT BETWEEN 1 AND ?1 THEN X''
                      ELSE boot_manifest
                    END,
                    CASE
                      WHEN boot_manifest_digest IS NULL THEN NULL
                      WHEN typeof(boot_manifest_digest) != 'blob' THEN 0
                      WHEN length(boot_manifest_digest) != 32 THEN X''
                      ELSE boot_manifest_digest
                    END
             FROM native_capture_format_v1 WHERE singleton=1",
            params![i64::try_from(MAX_BYTES).map_err(|_width| IndexError::ResourceLimited)?],
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
                if selected.protected_source_routes.is_some_and(|route| {
                    route
                        .marker_anchor
                        .is_some_and(|anchor| anchor == route.object || anchor.0 == Digest([0; 32]))
                }) {
                    return Ok(Err(IndexError::Storage(
                        "boot Source route marker anchor is invalid".into(),
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
    let validate = |original: &super::OperationRow| {
        if original.immutable != selected.operation
            || original.status != OperationStatus::Published(selected.publication.identity)
        {
            return Err(IndexError::Storage(
                "selected boot original receipt mismatch".into(),
            ));
        }
        Ok(())
    };
    if selected.protected_source_routes.is_some() {
        with_protected_operation(connection, selected.operation.key, budget, |row, _| {
            validate(row)
        })?;
    } else {
        charge_operation(connection, selected.operation.key, budget)?;
        let original = read_operation(connection, selected.operation.key)?
            .ok_or(IndexError::SnapshotUnavailable)?;
        validate(&original)?;
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

/// Normal references were already searched. A lost reference from the
/// authenticated selection is corruption, so GC must preserve the evidence.
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
    let Some(selected) = read_stored(connection)? else {
        return Ok(());
    };
    if selected.all_protected_roots().any(|root| root == object) {
        return Err(IndexError::Storage(
            "selected boot root lost its custody relation".into(),
        ));
    }
    if selected
        .protected_source_routes
        .and_then(|route| route.marker_anchor)
        == Some(object)
    {
        return Err(IndexError::Storage(
            "selected boot marker anchor lost its ancestry relation".into(),
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

/// Read a protected route operation under a loan for its measured stored row.
/// The caller's transaction keeps the length preflight and row read on one
/// SQLite snapshot. Unprotected BOOT operations retain the fixed 4 KiB bound.
pub(super) fn with_protected_operation<T>(
    connection: &Connection,
    key: OperationKey,
    budget: &mut WorkBudget,
    visit: impl FnOnce(&super::OperationRow, &mut WorkBudget) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let sizes: Option<(Option<i64>, Option<i64>, Option<i64>)> = {
        let _key_memory = budget
            .reserve_memory(SCRATCH_BYTES, MemoryLifetime::Temporary)?
            .ok_or(IndexError::ResourceLimited)?;
        connection
            .query_row(
                "SELECT CASE WHEN typeof(workspace)='blob' AND length(workspace) BETWEEN 1 AND 4096 THEN length(workspace) END,
                        CASE WHEN typeof(payload)='blob' AND length(payload) BETWEEN 1 AND ?2 THEN length(payload) END,
                        CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN 32 END
                 FROM native_operation_v1 WHERE operation=?1",
                params![
                    operation_key(key)?,
                    i64::try_from(MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1)
                        .map_err(|_| IndexError::ResourceLimited)?,
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| sql(&error))?
    };
    let (workspace, payload, digest) = sizes.ok_or(IndexError::SnapshotUnavailable)?;
    let workspace = uint(workspace.ok_or(IndexError::DigestConflict)?)?;
    let payload = uint(payload.ok_or(IndexError::ResourceLimited)?)?;
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
    // The operation has fixed fields plus Vec<SourceEvent>. The canonical
    // event JSON is wider than its in-memory fields; four payload lengths
    // cover SQL materialization and decoded vector growth. The callback must
    // fund any additional allocations it makes.
    let backing = payload
        .checked_mul(4)
        .and_then(|value| value.checked_add(workspace))
        .and_then(|value| value.checked_add(4096))
        .ok_or(IndexError::ResourceLimited)?;
    let _row_memory = budget
        .reserve_memory(backing, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let row = read_operation(connection, key)?.ok_or(IndexError::SnapshotUnavailable)?;
    visit(&row, budget)
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

// Preserve the caller's type and shape diagnostics while preventing an
// oversized stored key from being loaded into SQLite's result buffer.
pub(super) const SCAN_NATIVE_OPERATION_KEYS: &str = "SELECT CASE
    WHEN typeof(operation) != 'blob' THEN 0
    WHEN length(operation) NOT BETWEEN 1 AND 4096 THEN X''
    ELSE operation END
    FROM native_operation_v1 ORDER BY operation_id";

pub(super) fn decode_scanned_operation_key(bytes: &[u8]) -> Result<OperationKey, IndexError> {
    let key: OperationKey = decode(bytes)?;
    if operation_key(key)?.as_slice() != bytes {
        return Err(IndexError::DigestConflict);
    }
    Ok(key)
}

// Hold the measured operation loan through the quiescence or publication callback.
pub(super) fn with_quiescent_operation_v1<T>(
    connection: &Connection,
    key: OperationKey,
    routes: Option<&NativeProtectedSourceRoutesV1>,
    budget: &mut WorkBudget,
    visit: impl FnOnce(&super::OperationRow, &mut WorkBudget) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    if routes.is_some() {
        return with_protected_operation(connection, key, budget, visit);
    }
    charge_operation(connection, key, budget)?;
    let operation = read_operation(connection, key)?.ok_or(IndexError::SnapshotUnavailable)?;
    visit(&operation, budget)
}
