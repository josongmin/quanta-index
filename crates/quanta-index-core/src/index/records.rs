use quanta_index_contract::index::{
    AttemptRef, CatalogView, IndexInstanceId, IngressRef, InputBinding, InstanceSpec, MapRoot,
    MemberBinding, ObjectRef, OperationStatus, OwnerKey, OwnerMutationSet, ProducerId, SourceCut,
    SourceEvent, SourceRecord, StableOwnerId, ViewRef, WorkClaim,
};
use std::{ops::Deref, sync::Arc};

/// An admitted temporary key in the canonical source-path namespace.
pub struct SourcePathKey {
    bytes: Vec<u8>,
    _memory: Option<super::MemoryLease>,
}

impl SourcePathKey {
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    pub fn copy(path: &str, budget: &super::WorkBudget) -> Result<Self, super::IndexError> {
        if path.len() > super::MAX_SOURCE_PATH_BYTES {
            return Err(super::IndexError::CorruptObject(
                "source path key exceeds bound".into(),
            ));
        }
        let bytes = path
            .len()
            .checked_add(5)
            .ok_or(super::IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| super::IndexError::ResourceLimited)?,
            super::MemoryLifetime::Temporary,
        )?;
        let mut key = Vec::new();
        key.try_reserve_exact(bytes)
            .map_err(|_error| super::IndexError::ResourceLimited)?;
        if key.capacity() > bytes {
            return Err(super::IndexError::ResourceLimited);
        }
        key.extend_from_slice(b"path\0");
        key.extend_from_slice(path.as_bytes());
        budget.checkpoint()?;
        Ok(Self {
            bytes: key,
            _memory: memory,
        })
    }
}

/// A path's owner is fixed data, not a separately registered marker object.
pub fn path_owner_map_value(owner: OwnerKey) -> Result<super::MapValue, super::IndexError> {
    let mut bytes = [0; 32];
    for (slot, value) in bytes
        .iter_mut()
        .zip(owner.producer.0.iter().chain(&owner.stable_id.0))
    {
        *slot = *value;
    }
    super::MapValue::inline(&bytes, None)
}
pub fn owner_from_path_map_value(
    value: super::MapValue,
    budget: &super::WorkBudget,
) -> Result<OwnerKey, super::IndexError> {
    budget.checkpoint()?;
    let super::MapValue::Inline(value) = value else {
        return Err(super::IndexError::CorruptObject(
            "path owner must be inline data".into(),
        ));
    };
    let data = value.data()?;
    if !value.children()?.is_empty() || data.len() != 32 {
        return Err(super::IndexError::CorruptObject(
            "path owner data shape differs".into(),
        ));
    }
    let (producer, stable) = data
        .split_at_checked(16)
        .ok_or(super::IndexError::ResourceLimited)?;
    Ok(OwnerKey {
        producer: ProducerId(
            producer
                .try_into()
                .map_err(|_error| super::IndexError::ResourceLimited)?,
        ),
        stable_id: StableOwnerId(
            stable
                .try_into()
                .map_err(|_error| super::IndexError::ResourceLimited)?,
        ),
    })
}

/// Canonical key for owner-scoped source, coverage and missing maps. Instance
/// selection belongs to the enclosing root, not an ambiguous key prefix.
#[must_use]
pub fn producer_owner_prefix(producer: ProducerId) -> [u8; 22] {
    let mut prefix = [0; 22];
    prefix[..6].copy_from_slice(b"owner\0");
    prefix[6..].copy_from_slice(&producer.0);
    prefix
}

#[must_use]
pub fn owner_map_key(owner: OwnerKey) -> [u8; 38] {
    let mut key = [0; 38];
    key[..22].copy_from_slice(&producer_owner_prefix(owner.producer));
    key[22..].copy_from_slice(&owner.stable_id.0);
    key
}

/// Decode only the canonical owner namespace and exact fixed key width.
pub fn owner_from_map_key(key: &[u8]) -> Result<OwnerKey, super::IndexError> {
    let bytes: &[u8; 38] = key
        .try_into()
        .map_err(|_error| super::IndexError::CorruptObject("owner map key length".into()))?;
    let mut producer = [0; 16];
    producer.copy_from_slice(&bytes[6..22]);
    let mut stable_id = [0; 16];
    stable_id.copy_from_slice(&bytes[22..]);
    let owner = OwnerKey {
        producer: ProducerId(producer),
        stable_id: StableOwnerId(stable_id),
    };
    if owner_map_key(owner) != *bytes {
        return Err(super::IndexError::CorruptObject(
            "owner map key namespace".into(),
        ));
    }
    Ok(owner)
}

/// Canonical key for the selected member or coverage root. Construction is
/// allocation-free; only consumers that actually own a row need an owned copy.
#[must_use]
pub fn member_map_key(instance: IndexInstanceId) -> [u8; 23] {
    let mut key = [0; 23];
    for (target, byte) in key.iter_mut().zip(b"member\0".iter().chain(&instance.0)) {
        *target = *byte;
    }
    key
}

/// One decoded source record and its live path/parser allocation custody.
///
/// Borrow the canonical record; moving the owner preserves its lease. Deep
/// cloning requires a new admission rather than an implicit `Clone` operation.
#[derive(Debug)]
pub struct SourceRecordOwner {
    record: SourceRecord,
    // Release decoded data before its live allocation charge.
    _memory: Option<super::MemoryLease>,
}

impl SourceRecordOwner {
    /// Admit conservative JSON backing before decode.
    ///
    /// Retain the allocation through use.
    /// The 8x allowance is logical accounting, not a physical allocator/RSS cap.
    pub fn decode_json(
        payload: &[u8],
        budget: &super::WorkBudget,
        lifetime: super::MemoryLifetime,
    ) -> Result<Self, super::IndexError> {
        budget.checkpoint()?;
        let bytes = u64::try_from(payload.len())
            .map_err(|_error| super::IndexError::ResourceLimited)?
            .checked_mul(8)
            .ok_or(super::IndexError::ResourceLimited)?;
        let memory = if bytes == 0 {
            budget.checkpoint()?;
            None
        } else {
            budget.reserve_memory(bytes, lifetime)?
        };
        let record = serde_json::from_slice(payload)
            .map_err(|error| super::IndexError::CorruptObject(format!("source record: {error}")));
        budget.checkpoint()?;
        Ok(Self {
            record: record?,
            _memory: memory,
        })
    }

    /// Transfer the existing decoded path to retained output custody.
    ///
    /// Admit the path capacity while the original decode grant is still live.
    /// The returned lease must remain paired with the returned record. Failure
    /// drops the record before refunding its original grant; no path is cloned.
    pub fn into_retained_parts(
        self,
        budget: &super::WorkBudget,
    ) -> Result<(SourceRecord, Option<super::MemoryLease>), super::IndexError> {
        budget.checkpoint()?;
        // Keep this exhaustive: a new record field must revisit retained custody.
        let SourceRecord {
            path,
            bytes: _,
            byte_len: _,
            text: _,
        } = &self.record;
        let bytes =
            u64::try_from(path.capacity()).map_err(|_error| super::IndexError::ResourceLimited)?;
        let memory = if bytes == 0 {
            None
        } else {
            budget.reserve_memory(bytes, super::MemoryLifetime::Retained)?
        };
        budget.checkpoint()?;
        let Self {
            record,
            _memory: original_memory,
        } = self;
        // The replacement grant already covers the only owned record field.
        drop(original_memory);
        Ok((record, memory))
    }

    /// Explicit standalone/test construction without registered live accounting.
    #[must_use]
    pub fn new_unscoped(record: SourceRecord) -> Self {
        Self {
            record,
            _memory: None,
        }
    }
}

impl Deref for SourceRecordOwner {
    type Target = SourceRecord;

    fn deref(&self) -> &Self::Target {
        &self.record
    }
}

impl PartialEq for SourceRecordOwner {
    fn eq(&self, other: &Self) -> bool {
        self.record == other.record
    }
}

impl Eq for SourceRecordOwner {}

#[cfg(test)]
mod source_record_owner_tests {
    use super::*;
    use crate::index::{
        Digest, IndexError, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope,
        MemoryScopePort, SourceTextAdmission, WorkBudget, WorkLimits,
    };
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    const WIRE: &[u8] = br#"{"path":"src/lib.rs","bytes":[7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7],"byte_len":3,"text":"Excluded"}"#;

    #[derive(Default)]
    struct State {
        live: AtomicU64,
        peak: AtomicU64,
        releases: AtomicU64,
        retained_bytes: AtomicU64,
        deny_retained: AtomicBool,
    }
    impl MemoryLeaseOwner for State {
        fn release(&self, bytes: u64) {
            let _prior = self.live.fetch_sub(bytes, Ordering::SeqCst);
            let _prior = self.releases.fetch_add(1, Ordering::SeqCst);
        }
    }
    struct Scope(Arc<State>);
    impl MemoryScopePort for Scope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if lifetime == MemoryLifetime::Retained {
                self.0.retained_bytes.store(bytes, Ordering::SeqCst);
                if self.0.deny_retained.load(Ordering::SeqCst) {
                    return Err(IndexError::ResourceLimited);
                }
            }
            let prior = self
                .0
                .live
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                    live.checked_add(bytes)
                })
                .map_err(|_prior| IndexError::ResourceLimited)?;
            let live = prior
                .checked_add(bytes)
                .ok_or(IndexError::ResourceLimited)?;
            let _prior = self.0.peak.fetch_max(live, Ordering::SeqCst);
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }
    fn work() -> Result<(WorkBudget, Arc<State>), IndexError> {
        let state = Arc::new(State::default());
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(state.clone())))));
        Ok((budget, state))
    }

    #[test]
    fn retained_transfer_keeps_fixed_record_and_overlapping_charge_without_clone()
    -> Result<(), IndexError> {
        let (budget, state) = work()?;
        let decoded = SourceRecordOwner::decode_json(WIRE, &budget, MemoryLifetime::Temporary)?;
        let address = decoded.path.as_ptr();
        let capacity =
            u64::try_from(decoded.path.capacity()).map_err(|_error| IndexError::ResourceLimited)?;
        let decode_bytes =
            u64::try_from(WIRE.len()).map_err(|_error| IndexError::ResourceLimited)? * 8;
        if state.live.load(Ordering::SeqCst) != decode_bytes {
            return Err(IndexError::BackendViolation(
                "source decode grant differs".into(),
            ));
        }
        let (record, memory) = decoded.into_retained_parts(&budget)?;
        if record.path != "src/lib.rs"
            || record.bytes != ObjectRef(Digest([7; 32]))
            || record.byte_len != 3
            || record.text != SourceTextAdmission::Excluded
            || record.path.as_ptr() != address
            || state.live.load(Ordering::SeqCst) != capacity
            || state.retained_bytes.load(Ordering::SeqCst) != capacity
            || state.peak.load(Ordering::SeqCst) != decode_bytes + capacity
            || state.releases.load(Ordering::SeqCst) != 1
        {
            return Err(IndexError::BackendViolation(
                "source transfer value, pointer or overlap changed".into(),
            ));
        }
        drop(budget);
        if state.live.load(Ordering::SeqCst) != capacity || record.path != "src/lib.rs" {
            return Err(IndexError::BackendViolation(
                "source output lost custody after execution".into(),
            ));
        }
        drop(record);
        drop(memory);
        if state.live.load(Ordering::SeqCst) != 0 || state.releases.load(Ordering::SeqCst) != 2 {
            return Err(IndexError::BackendViolation(
                "source output refund differs".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn retained_transfer_denial_and_original_cancel_refund_decode_owner() -> Result<(), IndexError>
    {
        for cancelled in [false, true] {
            let (budget, state) = work()?;
            let decoded = SourceRecordOwner::decode_json(WIRE, &budget, MemoryLifetime::Temporary)?;
            state.deny_retained.store(true, Ordering::SeqCst);
            if cancelled {
                budget.request().cancel_handle().cancel();
            }
            let result = decoded.into_retained_parts(&budget);
            let expected = if cancelled {
                IndexError::Cancelled
            } else {
                IndexError::ResourceLimited
            };
            if !matches!(result, Err(error) if error == expected)
                || state.live.load(Ordering::SeqCst) != 0
                || state.releases.load(Ordering::SeqCst) != 1
                || (cancelled && state.retained_bytes.load(Ordering::SeqCst) != 0)
            {
                return Err(IndexError::BackendViolation(
                    "source transfer refusal leaked or renewed control".into(),
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn malformed_decode_and_cancel_preserve_original_control_and_refund() -> Result<(), IndexError>
    {
        let (budget, state) = work()?;
        if !matches!(
            SourceRecordOwner::decode_json(b"not-json", &budget, MemoryLifetime::Temporary),
            Err(IndexError::CorruptObject(_))
        ) || state.live.load(Ordering::SeqCst) != 0
            || state.releases.load(Ordering::SeqCst) != 1
        {
            return Err(IndexError::BackendViolation(
                "malformed source retained its decode grant".into(),
            ));
        }
        budget.request().cancel_handle().cancel();
        if !matches!(
            SourceRecordOwner::decode_json(b"not-json", &budget, MemoryLifetime::Temporary),
            Err(IndexError::Cancelled)
        ) || state.releases.load(Ordering::SeqCst) != 1
        {
            return Err(IndexError::BackendViolation(
                "source decoder replaced cancelled control".into(),
            ));
        }
        Ok(())
    }
}

/// A source owner's complete old/new contribution and decoded path custody.
#[derive(Debug, Eq, PartialEq)]
pub struct SourceChange {
    pub owner: OwnerKey,
    pub prior: Option<(ObjectRef, SourceRecordOwner)>,
    pub next: Option<(ObjectRef, SourceRecordOwner)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceDelta {
    pub prior: SourceCut,
    pub next: SourceCut,
    pub changed: OwnerMutationSet,
}

use super::RegisteredObject;

/// The one catalog fence that may reserve a managed physical installation.
///
/// A managed store never accepts `ObjectWriteSink` callbacks as custody proof;
/// it submits this scope to its paired catalog and checks the exact object.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectInstallScope {
    Ingress(IngressRef),
    Attempt(AttemptRef),
    Bootstrap(BootstrapRef),
}

/// Durable, catalog-fenced custody for genesis before the first view exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootstrapRef {
    pub workspace: super::WorkspaceKey,
    pub epoch: u64,
    pub fence: u64,
    pub profile: ObjectRef,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredObject {
    inner: Arc<StoredObjectData>,
}

/// Payload and registration retain separate allocation custody. Cloning
/// `StoredObject` shares both; an extracted registration retains its own charge.
#[derive(Debug)]
pub struct StoredObjectData {
    pub registration: RegisteredObject,
    pub payload: Vec<u8>,
    // Last field drops after the charged data, never before its allocation.
    memory: Option<super::MemoryLease>,
}

impl StoredObject {
    #[must_use]
    pub fn new(
        registration: RegisteredObject,
        payload: Vec<u8>,
        memory: Option<super::MemoryLease>,
    ) -> Self {
        Self {
            inner: Arc::new(StoredObjectData {
                registration,
                payload,
                memory,
            }),
        }
    }

    /// Declared data charge includes requested buffer capacity, the shared
    /// value, and an Arc-header allowance. This is not allocator/RSS accounting:
    /// lease/ledger metadata and allocator overhead need separate coverage.
    pub fn allocation_bytes(payload: usize, children: usize) -> Result<u64, super::IndexError> {
        Self::payload_allocation_bytes(payload)?
            .checked_add(RegisteredObject::allocation_bytes(children)?)
            .ok_or(super::IndexError::ResourceLimited)
    }

    pub fn payload_allocation_bytes(payload: usize) -> Result<u64, super::IndexError> {
        let bytes = payload
            .checked_add(std::mem::size_of::<StoredObjectData>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<[usize; 2]>()))
            .ok_or(super::IndexError::ResourceLimited)?;
        u64::try_from(bytes).map_err(|_error| super::IndexError::ResourceLimited)
    }

    #[must_use]
    pub fn memory_bytes(&self) -> Option<u64> {
        let payload = self.inner.memory.as_ref().map(super::MemoryLease::bytes);
        let registration = self.inner.registration.memory_bytes();
        if payload.is_none() && registration.is_none() {
            None
        } else {
            payload.unwrap_or(0).checked_add(registration.unwrap_or(0))
        }
    }
}

impl Deref for StoredObject {
    type Target = StoredObjectData;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl PartialEq for StoredObjectData {
    fn eq(&self, other: &Self) -> bool {
        self.registration == other.registration && self.payload == other.payload
    }
}

impl Eq for StoredObjectData {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MapEdit {
    pub key: Vec<u8>,
    pub expected: Option<super::MapValue>,
    pub next: Option<super::MapValue>,
}
impl MapEdit {
    #[must_use]
    pub fn objects(key: Vec<u8>, expected: Option<ObjectRef>, next: Option<ObjectRef>) -> Self {
        Self {
            key,
            expected: expected.map(Into::into),
            next: next.map(Into::into),
        }
    }
}

/// Staged object references retain allocation custody after execution ends.
#[derive(Debug)]
pub struct StagedObjectReferences {
    references: Vec<ObjectRef>,
    // Drop backing before releasing retained capacity.
    memory: Option<super::MemoryLease>,
}

impl StagedObjectReferences {
    pub fn from_parts(
        references: Vec<ObjectRef>,
        memory: Option<super::MemoryLease>,
    ) -> Result<Self, super::IndexError> {
        let result = Self { references, memory };
        let bytes = result
            .references
            .len()
            .checked_mul(std::mem::size_of::<ObjectRef>())
            .ok_or(super::IndexError::ResourceLimited)?;
        let bytes = u64::try_from(bytes).map_err(|_error| super::IndexError::ResourceLimited)?;
        if result
            .memory
            .as_ref()
            .is_some_and(|lease| lease.bytes() < bytes)
        {
            return Err(super::IndexError::BackendViolation(
                "object references exceed retained allocation custody".into(),
            ));
        }
        Ok(result)
    }
}

impl Deref for StagedObjectReferences {
    type Target = [ObjectRef];

    fn deref(&self) -> &Self::Target {
        &self.references
    }
}

/// Consuming references keeps their backing charged until iterator drop.
#[derive(Debug)]
pub struct StagedObjectReferenceIter {
    references: std::vec::IntoIter<ObjectRef>,
    _memory: Option<super::MemoryLease>,
}

impl Iterator for StagedObjectReferenceIter {
    type Item = ObjectRef;

    fn next(&mut self) -> Option<Self::Item> {
        self.references.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.references.size_hint()
    }
}

impl ExactSizeIterator for StagedObjectReferenceIter {}

impl IntoIterator for StagedObjectReferences {
    type Item = ObjectRef;
    type IntoIter = StagedObjectReferenceIter;

    fn into_iter(self) -> Self::IntoIter {
        StagedObjectReferenceIter {
            references: self.references.into_iter(),
            _memory: self.memory,
        }
    }
}

impl<'a> IntoIterator for &'a StagedObjectReferences {
    type Item = &'a ObjectRef;
    type IntoIter = std::slice::Iter<'a, ObjectRef>;
    fn into_iter(self) -> Self::IntoIter {
        self.references.iter()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MapWrite {
    pub root: MapRoot,
}

/// An edit in one logical multi-root transaction. Ordinal and key ordering
/// spans the complete cursor, including producer page boundaries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrdinalMapEdit {
    inner: Arc<OrdinalMapEditData>,
}

/// The final shared cursor position owns its key and allocation charge.
#[derive(Debug)]
pub struct OrdinalMapEditData {
    pub root: u32,
    pub edit: MapEdit,
    memory: Option<super::MemoryLease>,
}

impl OrdinalMapEdit {
    /// Standalone caller-owned input. Native producers use `copy_key` instead.
    #[must_use]
    pub fn unscoped(root: u32, edit: MapEdit) -> Self {
        Self {
            inner: Arc::new(OrdinalMapEditData {
                root,
                edit,
                memory: None,
            }),
        }
    }

    pub fn allocation_bytes(key_bytes: usize) -> Result<u64, super::IndexError> {
        let bytes = key_bytes
            .checked_add(std::mem::size_of::<OrdinalMapEditData>())
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<[usize; 2]>()))
            .ok_or(super::IndexError::ResourceLimited)?;
        u64::try_from(bytes).map_err(|_error| super::IndexError::ResourceLimited)
    }

    /// Future retained capacity is admitted before key or shared value birth.
    pub fn copy_key(
        root: u32,
        key: &[u8],
        expected: Option<super::MapValue>,
        next: Option<super::MapValue>,
        budget: &super::WorkBudget,
    ) -> Result<Self, super::IndexError> {
        Self::copy_key_parts(root, &[key], expected, next, budget)
    }

    pub fn copy_object_key(
        root: u32,
        key: &[u8],
        expected: Option<ObjectRef>,
        next: Option<ObjectRef>,
        budget: &super::WorkBudget,
    ) -> Result<Self, super::IndexError> {
        Self::copy_key(
            root,
            key,
            expected.map(Into::into),
            next.map(Into::into),
            budget,
        )
    }
    pub fn copy_object_key_parts(
        root: u32,
        parts: &[&[u8]],
        expected: Option<ObjectRef>,
        next: Option<ObjectRef>,
        budget: &super::WorkBudget,
    ) -> Result<Self, super::IndexError> {
        Self::copy_key_parts(
            root,
            parts,
            expected.map(Into::into),
            next.map(Into::into),
            budget,
        )
    }
    pub fn copy_key_parts(
        root: u32,
        parts: &[&[u8]],
        expected: Option<super::MapValue>,
        next: Option<super::MapValue>,
        budget: &super::WorkBudget,
    ) -> Result<Self, super::IndexError> {
        let length = parts.iter().try_fold(0usize, |length, part| {
            length
                .checked_add(part.len())
                .ok_or(super::IndexError::ResourceLimited)
        })?;
        let memory = budget.reserve_memory(
            Self::allocation_bytes(length)?,
            super::MemoryLifetime::Retained,
        )?;
        let mut copied = Vec::new();
        copied
            .try_reserve_exact(length)
            .map_err(|_error| super::IndexError::ResourceLimited)?;
        for part in parts {
            copied.extend_from_slice(part);
        }
        budget.checkpoint()?;
        Ok(Self {
            inner: Arc::new(OrdinalMapEditData {
                root,
                edit: MapEdit {
                    key: copied,
                    expected,
                    next,
                },
                memory,
            }),
        })
    }

    #[must_use]
    pub fn memory_bytes(&self) -> Option<u64> {
        self.inner.memory.as_ref().map(super::MemoryLease::bytes)
    }
}

impl Deref for OrdinalMapEdit {
    type Target = OrdinalMapEditData;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl PartialEq for OrdinalMapEditData {
    fn eq(&self, other: &Self) -> bool {
        self.root == other.root && self.edit == other.edit
    }
}
impl Eq for OrdinalMapEditData {}

#[cfg(test)]
mod ordinal_edit_tests {
    use super::*;
    use crate::index::{
        Digest, IndexError, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope,
        MemoryScopePort, WorkBudget, WorkLimits,
    };
    use std::{
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    struct Releases {
        count: AtomicU64,
    }
    impl MemoryLeaseOwner for Releases {
        fn release(&self, _bytes: u64) {
            let _prior = self.count.fetch_add(1, Ordering::SeqCst);
        }
    }
    struct Scope {
        limit: u64,
        owner: Arc<Releases>,
    }
    impl MemoryScopePort for Scope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if bytes > self.limit || lifetime != MemoryLifetime::Retained {
                return Err(IndexError::ResourceLimited);
            }
            Ok(MemoryLease::new(bytes, self.owner.clone()))
        }
    }
    fn work(limit: u64) -> Result<(WorkBudget, Arc<Releases>), IndexError> {
        let owner = Arc::new(Releases {
            count: AtomicU64::new(0),
        });
        let mut work = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope {
            limit,
            owner: owner.clone(),
        }))));
        Ok((work, owner))
    }
    #[test]
    fn shared_position_keeps_one_charge_and_fixed_key_value_until_final_drop()
    -> Result<(), IndexError> {
        let bytes = OrdinalMapEdit::allocation_bytes(7)?;
        let (work, owner) = work(bytes)?;
        let next = ObjectRef(Digest([81; 32]));
        let position =
            OrdinalMapEdit::copy_object_key_parts(2, &[b"path\0", b"ab"], None, Some(next), &work)?;
        let clone = position.clone();
        if position.root != 2
            || position.edit.key != b"path\0ab"
            || position.edit.next != Some(next.into())
            || position.memory_bytes() != Some(bytes)
            || position.edit.key.as_ptr() != clone.edit.key.as_ptr()
        {
            return Err(IndexError::BackendViolation(
                "owned edit value or shared backing changed".into(),
            ));
        }
        drop(position);
        if owner.count.load(Ordering::SeqCst) != 0 {
            return Err(IndexError::BackendViolation(
                "shared key refunded early".into(),
            ));
        }
        drop(clone);
        if owner.count.load(Ordering::SeqCst) != 1 {
            return Err(IndexError::BackendViolation(
                "shared key final refund differs".into(),
            ));
        }
        Ok(())
    }
    #[test]
    fn future_floor_denial_and_cancellation_refuse_key_birth() -> Result<(), IndexError> {
        let (work, owner) = work(0)?;
        if !matches!(
            OrdinalMapEdit::copy_key(0, b"key", None, None, &work),
            Err(IndexError::ResourceLimited)
        ) || owner.count.load(Ordering::SeqCst) != 0
        {
            return Err(IndexError::BackendViolation(
                "key floor denial changed".into(),
            ));
        }
        let request = crate::RequestBudgetV1::for_duration(Duration::from_secs(30));
        request.cancel_handle().cancel();
        let cancelled = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            request,
        );
        if !matches!(
            OrdinalMapEdit::copy_key(0, b"key", None, None, &cancelled),
            Err(IndexError::Cancelled)
        ) {
            return Err(IndexError::BackendViolation(
                "cancelled key allocation succeeded".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MapBatchRequest {
    pub base: MapRoot,
    pub edits: Vec<MapEdit>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MapBatchWrite {
    /// One output root per input request, in the same order.
    pub roots: super::StagedMapRoots,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectBatchInput {
    pub payload: Vec<u8>,
    pub children: Vec<ObjectRef>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MapPage {
    inner: Arc<MapPageData>,
}

/// Immutable page data shares its reservation through clones and cursors.
#[derive(Debug)]
pub struct MapPageData {
    pub entries: Vec<(Vec<u8>, super::MapValue)>,
    pub has_more: bool,
    // Release custody only after all data allocations have been dropped.
    _memory: Vec<super::MemoryLease>,
    // Entry/key data and the lease-handle vector die before this capacity charge.
    _allocation_memory: Option<super::MemoryLease>,
}

impl MapPage {
    /// Key charges belong to `memory`; vector/value capacity belongs to
    /// `allocation_memory`, which is released after the key-lease vector.
    /// None is only an explicit standalone allocation contract.
    #[must_use]
    pub fn new(
        entries: Vec<(Vec<u8>, super::MapValue)>,
        has_more: bool,
        memory: Vec<super::MemoryLease>,
        allocation_memory: Option<super::MemoryLease>,
    ) -> Self {
        Self {
            inner: Arc::new(MapPageData {
                entries,
                has_more,
                _memory: memory,
                _allocation_memory: allocation_memory,
            }),
        }
    }

    /// Declared capacity for entries, lease handles and the shared page value.
    /// Keys are admitted separately. Allocator and lease-owner metadata are not
    /// claimed as an exact physical RSS bound.
    pub fn allocation_bytes(capacity: usize) -> Result<u64, super::IndexError> {
        let bytes = capacity
            .checked_mul(std::mem::size_of::<(Vec<u8>, super::MapValue)>())
            .and_then(|bytes| {
                capacity
                    .checked_mul(std::mem::size_of::<super::MemoryLease>())
                    .and_then(|handles| bytes.checked_add(handles))
            })
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<MapPageData>()))
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<[usize; 2]>()))
            .ok_or(super::IndexError::ResourceLimited)?;
        u64::try_from(bytes).map_err(|_error| super::IndexError::ResourceLimited)
    }

    #[must_use]
    pub fn cursor(self) -> MapPageCursor {
        let remaining = 0..self.entries.len();
        MapPageCursor {
            page: Some(self),
            remaining,
        }
    }
}

impl Deref for MapPage {
    type Target = MapPageData;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl PartialEq for MapPageData {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries && self.has_more == other.has_more
    }
}
impl Eq for MapPageData {}

/// A position retains the original page instead of copying its key.
#[derive(Clone, Debug)]
pub struct MapPageEntry {
    page: MapPage,
    index: usize,
}

impl MapPageEntry {
    pub fn get(&self) -> Result<&(Vec<u8>, super::MapValue), super::IndexError> {
        self.page.entries.get(self.index).ok_or_else(|| {
            super::IndexError::BackendViolation("map page position is outside its page".into())
        })
    }

    pub fn object_parts(&self) -> Result<(&Vec<u8>, &ObjectRef), super::IndexError> {
        let (key, value) = self.get()?;
        Ok((key, value.as_object_ref()?))
    }

    pub fn key(&self) -> Result<&[u8], super::IndexError> {
        self.get().map(|(key, _reference)| key.as_slice())
    }
}

/// Iteration retains the immutable page and its charge, including popped keys.
#[derive(Default)]
pub struct MapPageCursor {
    page: Option<MapPage>,
    remaining: std::ops::Range<usize>,
}

impl MapPageCursor {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.remaining.is_empty()
    }
}

impl Iterator for MapPageCursor {
    type Item = MapPageEntry;
    fn next(&mut self) -> Option<Self::Item> {
        let index = self.remaining.next()?;
        Some(MapPageEntry {
            page: self.page.as_ref()?.clone(),
            index,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdmissionOutcome {
    New(AttemptRef),
    Existing(OperationStatus),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedPublication {
    pub attempt: AttemptRef,
    pub source_mode: super::SourceOperationMode,
    pub expected: ViewRef,
    pub next: CatalogView,
    pub work_actions: super::WorkActionSet,
    pub complete_work: Option<WorkClaim>,
    pub source_events: Vec<SourceEvent>,
    /// Projection outbox events share stream authority but never advance source.
    pub projection_events: Vec<SourceEvent>,
    pub expected_source: Option<SourceCut>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TicketRecord {
    pub lease: super::ReadTicketLease,
    pub view: CatalogView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanContext {
    pub instance: InstanceSpec,
    pub source: SourceCut,
    pub input: InputBinding,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BaseMember {
    Absent,
    Present(Box<MemberBinding>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeltaInput<D> {
    Bootstrap,
    Patch(D),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanDecision<P> {
    Reuse(Box<MemberBinding>),
    Incremental(P),
    RebuildRequired,
    MaintenanceRequired,
}

#[cfg(test)]
mod staged_source_tests {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };

    use super::super::{Digest, IndexError, MemoryLease, MemoryLeaseOwner};
    use super::{ObjectRef, StagedObjectReferences};

    #[derive(Default)]
    struct Releases {
        count: AtomicU64,
        bytes: AtomicU64,
    }

    impl MemoryLeaseOwner for Releases {
        fn release(&self, bytes: u64) {
            let _previous_count = self.count.fetch_add(1, Ordering::Relaxed);
            let _previous_bytes = self.bytes.fetch_add(bytes, Ordering::Relaxed);
        }
    }

    #[test]
    fn staged_references_keep_backing_charged_until_consuming_iterator_drop()
    -> Result<(), IndexError> {
        let releases = Arc::new(Releases::default());
        let bytes = u64::try_from(std::mem::size_of::<ObjectRef>())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let lease = MemoryLease::new(bytes, releases.clone());
        let reference = ObjectRef(Digest([51; 32]));
        let references = StagedObjectReferences::from_parts(vec![reference], Some(lease))?;
        let mut iterator = references.into_iter();
        if iterator.next() != Some(reference)
            || iterator.next().is_some()
            || releases.count.load(Ordering::Relaxed) != 0
        {
            return Err(IndexError::BackendViolation(
                "source iterator released live backing early".into(),
            ));
        }
        drop(iterator);
        if releases.count.load(Ordering::Relaxed) != 1
            || releases.bytes.load(Ordering::Relaxed) != bytes
        {
            return Err(IndexError::BackendViolation(
                "source iterator final release differs".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn staged_references_refuse_underfunded_custody_and_release_once() -> Result<(), IndexError> {
        let releases = Arc::new(Releases::default());
        let lease = MemoryLease::new(1, releases.clone());
        let result =
            StagedObjectReferences::from_parts(vec![ObjectRef(Digest([52; 32]))], Some(lease));
        if !matches!(result, Err(IndexError::BackendViolation(_)))
            || releases.count.load(Ordering::Relaxed) != 1
            || releases.bytes.load(Ordering::Relaxed) != 1
        {
            return Err(IndexError::BackendViolation(
                "underfunded source custody escaped or leaked".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod fixed_key_tests {
    use super::{member_map_key, owner_from_map_key, owner_map_key, producer_owner_prefix};
    use quanta_index_contract::index::{IndexInstanceId, OwnerKey, ProducerId, StableOwnerId};

    #[test]
    fn fixed_lookup_keys_preserve_canonical_bytes() -> Result<(), super::super::IndexError> {
        let producer = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let stable = [
            240, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255,
        ];
        // Fixed persisted-key golds, independent of the constructors' iteration.
        let owner_gold = [
            111, 119, 110, 101, 114, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 240,
            241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255,
        ];
        let member_gold = [
            109, 101, 109, 98, 101, 114, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
        ];
        let owner_key: [u8; 38] = owner_map_key(OwnerKey {
            producer: ProducerId(producer),
            stable_id: StableOwnerId(stable),
        });
        let member_key: [u8; 23] = member_map_key(IndexInstanceId(producer));
        let prefix: [u8; 22] = producer_owner_prefix(ProducerId(producer));
        if owner_key != owner_gold || member_key != member_gold {
            return Err(super::super::IndexError::BackendViolation(
                "fixed lookup key differs from canonical persisted bytes".into(),
            ));
        }
        let expected_owner = OwnerKey {
            producer: ProducerId(producer),
            stable_id: StableOwnerId(stable),
        };
        if prefix.as_slice() != &owner_gold[..22]
            || owner_from_map_key(&owner_gold)? != expected_owner
        {
            return Err(super::super::IndexError::BackendViolation(
                "source owner prefix or inverse differs from fixed key".into(),
            ));
        }
        let mut wrong_namespace = owner_gold;
        wrong_namespace[0] = b'x';
        if !matches!(
            owner_from_map_key(&wrong_namespace),
            Err(super::super::IndexError::CorruptObject(_))
        ) || !matches!(
            owner_from_map_key(&owner_gold[..37]),
            Err(super::super::IndexError::CorruptObject(_))
        ) {
            return Err(super::super::IndexError::BackendViolation(
                "malformed source owner key was accepted".into(),
            ));
        }
        Ok(())
    }
}
