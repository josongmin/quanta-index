use quanta_index_contract::index::{
    AttemptRef, CatalogView, IndexInstanceId, IngressRef, InputBinding, InstanceSpec, MapRoot,
    MemberBinding, ObjectRef, OperationStatus, OwnerKey, OwnerMutationSet, ProducerId, SourceCut,
    SourceEvent, SourceRecord, SourceTextAdmission, StableOwnerId, ViewRef, WorkClaim,
};
use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    NativeIdentityConstructionErrorV1, NativeNormalizationErrorV1, RepoId,
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

/// A key inside one path's repository map. An empty suffix denotes the
/// original owner whose source record has no repository identity.
pub struct SourcePathRepositoryKey {
    bytes: [u8; 5 + RepoId::MAX_UTF8_BYTES_V1],
    len: usize,
}

impl SourcePathRepositoryKey {
    pub fn new(repository: Option<&RepoId>) -> Result<Self, super::IndexError> {
        const PREFIX: &[u8; 5] = b"repo\0";
        let name = repository.map_or("", RepoId::as_str).as_bytes();
        let mut bytes = [0; 5 + RepoId::MAX_UTF8_BYTES_V1];
        bytes
            .first_chunk_mut::<5>()
            .ok_or(super::IndexError::ResourceLimited)?
            .copy_from_slice(PREFIX);
        let len = PREFIX
            .len()
            .checked_add(name.len())
            .ok_or(super::IndexError::ResourceLimited)?;
        bytes
            .get_mut(PREFIX.len()..len)
            .ok_or(super::IndexError::ResourceLimited)?
            .copy_from_slice(name);
        Ok(Self { bytes, len })
    }

    pub fn as_slice(&self) -> Result<&[u8], super::IndexError> {
        self.bytes
            .get(..self.len)
            .ok_or(super::IndexError::ResourceLimited)
    }
}

/// The value of an existing `path\0<relative path>` key. Keeping the
/// repository map in the value preserves the path key's ordering and limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourcePathOwners {
    /// The original 32-byte owner record has no repository identity.
    LegacyOwner(OwnerKey),
    /// A nonempty repository-to-owner map reachable from the selected cut.
    Repositories(ObjectRef),
}

impl SourcePathOwners {
    const REPOSITORIES_TAG: &'static [u8] = b"source-path-repositories/v1";

    pub fn encode(self) -> Result<super::MapValue, super::IndexError> {
        match self {
            Self::LegacyOwner(owner) => {
                let mut bytes = [0; 32];
                for (slot, value) in bytes
                    .iter_mut()
                    .zip(owner.producer.0.iter().chain(&owner.stable_id.0))
                {
                    *slot = *value;
                }
                super::MapValue::inline(&bytes, None)
            }
            Self::Repositories(root) => super::MapValue::inline(Self::REPOSITORIES_TAG, Some(root)),
        }
    }

    pub fn decode(
        value: super::MapValue,
        budget: &super::WorkBudget,
    ) -> Result<Self, super::IndexError> {
        budget.checkpoint()?;
        let super::MapValue::Inline(value) = value else {
            return Err(super::IndexError::CorruptObject(
                "path owner must be inline data".into(),
            ));
        };
        let data = value.data()?;
        match value.children()? {
            [root] if data == Self::REPOSITORIES_TAG => Ok(Self::Repositories(*root)),
            [] if data.len() == 32 => {
                let (producer, stable) = data
                    .split_at_checked(16)
                    .ok_or(super::IndexError::ResourceLimited)?;
                Ok(Self::LegacyOwner(OwnerKey {
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
                }))
            }
            _ => Err(super::IndexError::CorruptObject(
                "path owner data shape differs".into(),
            )),
        }
    }
}

pub fn path_owner_map_value(owner: OwnerKey) -> Result<super::MapValue, super::IndexError> {
    SourcePathOwners::LegacyOwner(owner).encode()
}

pub fn owner_from_path_map_value(
    value: super::MapValue,
    budget: &super::WorkBudget,
) -> Result<OwnerKey, super::IndexError> {
    match SourcePathOwners::decode(value, budget)? {
        SourcePathOwners::LegacyOwner(owner) => Ok(owner),
        SourcePathOwners::Repositories(_) => Err(super::IndexError::InvalidInput(
            "source path requires repository-aware selection".into(),
        )),
    }
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

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceRecordWire {
    path: String,
    bytes: ObjectRef,
    byte_len: u64,
    text: SourceTextAdmission,
    #[serde(default)]
    source_repo_id: Option<String>,
    #[serde(default)]
    language: Option<Box<str>>,
}

impl SourceRecordOwner {
    /// Check whether the decode grant can also fund a shared wrapper after
    /// parsing. Standalone records have no live memory authority.
    #[must_use]
    pub fn covers_shared_backing(&self, wrapper_bytes: usize) -> bool {
        let SourceRecord {
            path,
            bytes: _,
            byte_len: _,
            text: _,
            source_repo_id,
            language,
        } = &self.record;
        self._memory.as_ref().is_none_or(|memory| {
            path.capacity()
                .checked_add(
                    source_repo_id
                        .as_ref()
                        .map_or(0, |repo| repo.backing_capacity()),
                )
                .and_then(|bytes| {
                    bytes.checked_add(language.as_ref().map_or(0, |code| code.as_str().len()))
                })
                .and_then(|bytes| bytes.checked_add(wrapper_bytes))
                .and_then(|bytes| u64::try_from(bytes).ok())
                .is_some_and(|bytes| bytes <= memory.bytes())
        })
    }

    /// Admit conservative JSON backing before decode.
    ///
    /// Retain the allocation through use.
    /// The 8x allowance covers parser backing, not a physical allocator/RSS cap.
    /// Repository NFC scratch receives separate temporary grants during decode.
    pub fn decode_json(
        payload: &[u8],
        budget: &mut super::WorkBudget,
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
        let record = (|| -> Result<SourceRecord, super::IndexError> {
            let wire: SourceRecordWire = serde_json::from_slice(payload).map_err(|error| {
                super::IndexError::CorruptObject(format!("source record: {error}"))
            })?;
            let source_repo_id = wire
                .source_repo_id
                .map(|value| {
                    let mut admission = super::repository_key::RepositoryKeyAdmission::new(budget);
                    RepoId::try_from_owned_with_native_admission_v1(value, &mut admission).map_err(
                        |error| match error {
                            NativeIdentityConstructionErrorV1::Validation(cause) => {
                                super::IndexError::CorruptObject(format!("source record: {cause}"))
                            }
                            NativeIdentityConstructionErrorV1::Normalization(
                                NativeNormalizationErrorV1::Admission(cause),
                            ) => cause,
                            NativeIdentityConstructionErrorV1::Normalization(
                                NativeNormalizationErrorV1::NativeAllocationFailed
                                | NativeNormalizationErrorV1::ArithmeticOverflow
                                | NativeNormalizationErrorV1::UnsupportedNativeSortBacking,
                            ) => super::IndexError::ResourceLimited,
                            NativeIdentityConstructionErrorV1::Normalization(cause) => {
                                super::IndexError::BackendViolation(format!(
                                    "source record repository validation: {cause}"
                                ))
                            }
                            NativeIdentityConstructionErrorV1::Copy(_) => {
                                super::IndexError::BackendViolation(
                                    "owned source repository validation attempted a copy".into(),
                                )
                            }
                        },
                    )
                })
                .transpose()?;
            let language = wire
                .language
                .map(|code| {
                    LanguageCode::try_from_boxed_v1(code).map_err(|error| {
                        super::IndexError::CorruptObject(format!("source record: {error}"))
                    })
                })
                .transpose()?;
            Ok(SourceRecord {
                path: wire.path,
                bytes: wire.bytes,
                byte_len: wire.byte_len,
                text: wire.text,
                source_repo_id,
                language,
            })
        })();
        budget.checkpoint()?;
        Ok(Self {
            record: record?,
            _memory: memory,
        })
    }

    /// Transfer the existing decoded path to retained output custody.
    ///
    /// Admit the path, repository and language backing while the original decode
    /// grant is still live.
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
            source_repo_id,
            language,
        } = &self.record;
        let bytes = path
            .capacity()
            .checked_add(
                source_repo_id
                    .as_ref()
                    .map_or(0, |repo| repo.backing_capacity()),
            )
            .and_then(|bytes| {
                bytes.checked_add(language.as_ref().map_or(0, |code| code.as_str().len()))
            })
            .and_then(|bytes| u64::try_from(bytes).ok())
            .ok_or(super::IndexError::ResourceLimited)?;
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
        // The replacement grant already covers the owned path and repository.
        drop(original_memory);
        Ok((record, memory))
    }

    /// Transfer only the path when the public result carries no repository ID.
    pub fn into_retained_path(
        self,
        budget: &super::WorkBudget,
    ) -> Result<(String, Option<super::MemoryLease>), super::IndexError> {
        budget.checkpoint()?;
        let bytes = u64::try_from(self.record.path.capacity())
            .map_err(|_error| super::IndexError::ResourceLimited)?;
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
        let SourceRecord {
            path,
            source_repo_id,
            language,
            bytes: _,
            byte_len: _,
            text: _,
        } = record;
        drop(source_repo_id);
        drop(language);
        drop(original_memory);
        Ok((path, memory))
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
        max_live: AtomicU64,
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
                    let next = live.checked_add(bytes)?;
                    let limit = self.0.max_live.load(Ordering::SeqCst);
                    (limit == 0 || next <= limit).then_some(next)
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
        work_with_bytes(1_000_000)
    }

    fn work_with_bytes(bytes: u64) -> Result<(WorkBudget, Arc<State>), IndexError> {
        let state = Arc::new(State::default());
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes,
                mutations: 100,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(state.clone())))));
        Ok((budget, state))
    }

    fn wire_with_repository(repository: &str) -> Result<Vec<u8>, IndexError> {
        let mut wire = WIRE
            .strip_suffix(b"}")
            .ok_or_else(|| IndexError::BackendViolation("source fixture terminator".into()))?
            .to_vec();
        wire.extend_from_slice(b",\"source_repo_id\":");
        wire.extend_from_slice(
            &serde_json::to_vec(repository)
                .map_err(|error| IndexError::BackendViolation(error.to_string()))?,
        );
        wire.push(b'}');
        Ok(wire)
    }

    #[test]
    fn source_path_owner_codec_keeps_legacy_rows_and_binds_repository_children()
    -> Result<(), IndexError> {
        let (budget, _) = work()?;
        let legacy = OwnerKey {
            producer: ProducerId([13; 16]),
            stable_id: StableOwnerId([17; 16]),
        };
        let mut expected = [13; 32];
        for byte in expected.iter_mut().skip(16) {
            *byte = 17;
        }
        let old_wire = crate::index::MapValue::inline(&expected, None)?;
        if path_owner_map_value(legacy)? != old_wire
            || owner_from_path_map_value(old_wire, &budget)? != legacy
        {
            return Err(IndexError::BackendViolation(
                "legacy path row changed".into(),
            ));
        }

        let root = ObjectRef(Digest([71; 32]));
        let value = SourcePathOwners::Repositories(root).encode()?;
        if value.children()? != [root]
            || SourcePathOwners::decode(value, &budget)? != SourcePathOwners::Repositories(root)
            || !matches!(
                owner_from_path_map_value(value, &budget),
                Err(IndexError::InvalidInput(_))
            )
        {
            return Err(IndexError::BackendViolation(
                "repository path row changed".into(),
            ));
        }
        for corrupt in [
            crate::index::MapValue::inline(SourcePathOwners::REPOSITORIES_TAG, None)?,
            crate::index::MapValue::inline(b"unknown", Some(root))?,
        ] {
            if !matches!(
                SourcePathOwners::decode(corrupt, &budget),
                Err(IndexError::CorruptObject(_))
            ) {
                return Err(IndexError::BackendViolation(
                    "invalid repository path row".into(),
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn source_path_repository_keys_keep_unscoped_and_maximum_ids_distinct() -> Result<(), IndexError>
    {
        let legacy = SourcePathRepositoryKey::new(None)?;
        let first =
            RepoId::new("corp-a").map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        let second =
            RepoId::new("corp-b").map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        let max = RepoId::new("x".repeat(RepoId::MAX_UTF8_BYTES_V1))
            .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        if legacy.as_slice()? != b"repo\0"
            || SourcePathRepositoryKey::new(Some(&first))?.as_slice()? != b"repo\0corp-a"
            || SourcePathRepositoryKey::new(Some(&second))?.as_slice()? != b"repo\0corp-b"
            || SourcePathRepositoryKey::new(Some(&max))?.as_slice()?.len()
                != 5 + RepoId::MAX_UTF8_BYTES_V1
        {
            return Err(IndexError::BackendViolation(
                "source repository key changed".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn retained_transfer_keeps_fixed_record_and_overlapping_charge_without_clone()
    -> Result<(), IndexError> {
        let (mut budget, state) = work()?;
        let decoded = SourceRecordOwner::decode_json(WIRE, &mut budget, MemoryLifetime::Temporary)?;
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
    fn path_only_transfer_refunds_repository_backing() -> Result<(), IndexError> {
        let (mut budget, state) = work()?;
        let mut record: SourceRecord = serde_json::from_slice(WIRE)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        record.source_repo_id = Some(
            RepoId::new("authored-repository")
                .map_err(|error| IndexError::BackendViolation(error.to_string()))?,
        );
        record.language = Some(
            LanguageCode::new("rust")
                .map_err(|error| IndexError::BackendViolation(error.into()))?,
        );
        let wire = serde_json::to_vec(&record)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let decoded =
            SourceRecordOwner::decode_json(&wire, &mut budget, MemoryLifetime::Temporary)?;
        let pointer = decoded.path.as_ptr();
        let path_bytes =
            u64::try_from(decoded.path.capacity()).map_err(|_error| IndexError::ResourceLimited)?;
        let (path, memory) = decoded.into_retained_path(&budget)?;
        if path != "src/lib.rs"
            || path.as_ptr() != pointer
            || state.live.load(Ordering::SeqCst) != path_bytes
            || state.retained_bytes.load(Ordering::SeqCst) != path_bytes
        {
            return Err(IndexError::BackendViolation(
                "path transfer retained repository backing or copied the path".into(),
            ));
        }
        drop(path);
        drop(memory);
        if state.live.load(Ordering::SeqCst) != 0 {
            return Err(IndexError::BackendViolation(
                "path storage did not refund".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn retained_transfer_denial_and_original_cancel_refund_decode_owner() -> Result<(), IndexError>
    {
        for cancelled in [false, true] {
            let (mut budget, state) = work()?;
            let decoded =
                SourceRecordOwner::decode_json(WIRE, &mut budget, MemoryLifetime::Temporary)?;
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
        let (mut budget, state) = work()?;
        if !matches!(
            SourceRecordOwner::decode_json(b"not-json", &mut budget, MemoryLifetime::Temporary),
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
            SourceRecordOwner::decode_json(b"not-json", &mut budget, MemoryLifetime::Temporary),
            Err(IndexError::Cancelled)
        ) || state.releases.load(Ordering::SeqCst) != 1
        {
            return Err(IndexError::BackendViolation(
                "source decoder replaced cancelled control".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn repository_decode_admits_nfc_scratch_and_refunds_it() -> Result<(), IndexError> {
        let repository = format!("q{}", "\u{0301}".repeat(20));
        let wire = wire_with_repository(&repository)?;
        let decode_bytes = u64::try_from(wire.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(8)
            .ok_or(IndexError::ResourceLimited)?;
        let (mut budget, state) = work()?;
        let decoded = SourceRecordOwner::decode_json(&wire, &mut budget, MemoryLifetime::Retained)?;
        if decoded.source_repo_id.as_ref().map(RepoId::as_str) != Some(repository.as_str())
            || state.live.load(Ordering::SeqCst) != decode_bytes
            || state.peak.load(Ordering::SeqCst) <= decode_bytes
            || budget.usage().bytes == 0
        {
            return Err(IndexError::BackendViolation(
                "source repository NFC scratch was not admitted or refunded".into(),
            ));
        }
        drop(decoded);
        if state.live.load(Ordering::SeqCst) != 0 {
            return Err(IndexError::BackendViolation(
                "source repository decode grant was not refunded".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn repository_decode_refusals_keep_original_work_and_memory_cause() -> Result<(), IndexError> {
        let repository = format!("q{}", "\u{0301}".repeat(20));
        let wire = wire_with_repository(&repository)?;
        let decode_bytes = u64::try_from(wire.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(8)
            .ok_or(IndexError::ResourceLimited)?;
        let (mut memory_budget, memory_state) = work()?;
        memory_state.max_live.store(
            decode_bytes
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?,
            Ordering::SeqCst,
        );
        if !matches!(
            SourceRecordOwner::decode_json(&wire, &mut memory_budget, MemoryLifetime::Retained),
            Err(IndexError::ResourceLimited)
        ) || memory_state.live.load(Ordering::SeqCst) != 0
            || memory_state.peak.load(Ordering::SeqCst) != decode_bytes
        {
            return Err(IndexError::BackendViolation(
                "source repository NFC memory refusal lost its decode grant".into(),
            ));
        }
        let (mut successful_budget, successful_state) = work()?;
        let successful = SourceRecordOwner::decode_json(
            &wire,
            &mut successful_budget,
            MemoryLifetime::Retained,
        )?;
        drop(successful);
        if successful_state.live.load(Ordering::SeqCst) != 0 {
            return Err(IndexError::BackendViolation(
                "source repository reference decode grant was not refunded".into(),
            ));
        }
        let near_complete_work = successful_budget
            .usage()
            .bytes
            .checked_sub(1)
            .ok_or(IndexError::ResourceLimited)?;
        let (mut work_budget, work_state) = work_with_bytes(near_complete_work)?;
        if !matches!(
            SourceRecordOwner::decode_json(&wire, &mut work_budget, MemoryLifetime::Retained),
            Err(IndexError::ResourceLimited)
        ) || work_state.live.load(Ordering::SeqCst) != 0
            || work_state.peak.load(Ordering::SeqCst) <= decode_bytes
        {
            return Err(IndexError::BackendViolation(
                "source repository NFC work refusal lost its decode grant".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn repository_decode_rejects_noncanonical_and_preserves_strict_wire() -> Result<(), IndexError>
    {
        let malformed = format!("q{}", "\u{0344}".repeat(255));
        let malformed_wire = wire_with_repository(&malformed)?;
        let (mut budget, state) = work()?;
        if !matches!(
            SourceRecordOwner::decode_json(&malformed_wire, &mut budget, MemoryLifetime::Temporary),
            Err(IndexError::CorruptObject(_))
        ) || state.live.load(Ordering::SeqCst) != 0
            || budget.usage().bytes == 0
        {
            return Err(IndexError::BackendViolation(
                "noncanonical repository escaped source decode admission".into(),
            ));
        }
        let strict_prefix = WIRE
            .strip_suffix(b"}")
            .ok_or_else(|| IndexError::BackendViolation("source fixture terminator".into()))?;
        let mut null_wire = strict_prefix.to_vec();
        null_wire.extend_from_slice(b",\"source_repo_id\":null}");
        let decoded =
            SourceRecordOwner::decode_json(&null_wire, &mut budget, MemoryLifetime::Temporary)?;
        if decoded.source_repo_id.is_some() {
            return Err(IndexError::BackendViolation(
                "null source repository changed legacy semantics".into(),
            ));
        }
        drop(decoded);
        let mut unknown_wire = strict_prefix.to_vec();
        unknown_wire.extend_from_slice(b",\"unknown\":1}");
        let mut duplicate_wire = strict_prefix.to_vec();
        duplicate_wire.extend_from_slice(b",\"source_repo_id\":\"a\",\"source_repo_id\":\"b\"}");
        let mut invalid_language = strict_prefix.to_vec();
        invalid_language.extend_from_slice(b",\"language\":\"Rust\"}");
        let mut duplicate_language = strict_prefix.to_vec();
        duplicate_language.extend_from_slice(b",\"language\":null,\"language\":\"rust\"}");
        for wire in [
            b"{}".to_vec(),
            unknown_wire,
            duplicate_wire,
            invalid_language,
            duplicate_language,
        ] {
            if !matches!(
                SourceRecordOwner::decode_json(&wire, &mut budget, MemoryLifetime::Temporary),
                Err(IndexError::CorruptObject(_))
            ) || state.live.load(Ordering::SeqCst) != 0
            {
                return Err(IndexError::BackendViolation(
                    "source record strict wire was weakened".into(),
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn retained_language_uses_canonical_backing_and_legacy_record_remains_absent()
    -> Result<(), IndexError> {
        let (mut budget, state) = work()?;
        let old = SourceRecordOwner::decode_json(WIRE, &mut budget, MemoryLifetime::Temporary)?;
        if old.language.is_some() {
            return Err(IndexError::BackendViolation(
                "old source gained language".into(),
            ));
        }
        drop(old);
        let prefix = WIRE
            .strip_suffix(b"}")
            .ok_or_else(|| IndexError::BackendViolation("source fixture terminator".into()))?;
        let mut wire = prefix.to_vec();
        wire.extend_from_slice(b",\"language\":\"rust\"}");
        let decoded =
            SourceRecordOwner::decode_json(&wire, &mut budget, MemoryLifetime::Temporary)?;
        let expected = decoded
            .path
            .capacity()
            .checked_add(
                decoded
                    .language
                    .as_ref()
                    .map_or(0, |code| code.as_str().len()),
            )
            .ok_or(IndexError::ResourceLimited)?;
        if !decoded.covers_shared_backing(0) || decoded.covers_shared_backing(wire.len() * 8 + 1) {
            return Err(IndexError::BackendViolation(
                "language shared backing bound changed".into(),
            ));
        }
        let (record, lease) = decoded.into_retained_parts(&budget)?;
        if record.language.as_ref().map(LanguageCode::as_str) != Some("rust")
            || state.retained_bytes.load(Ordering::SeqCst)
                != u64::try_from(expected).map_err(|_| IndexError::ResourceLimited)?
        {
            return Err(IndexError::BackendViolation(
                "language retained backing was not funded".into(),
            ));
        }
        drop(record);
        drop(lease);
        if state.live.load(Ordering::SeqCst) != 0 {
            return Err(IndexError::BackendViolation(
                "language backing refund was lost".into(),
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

/// One shared cursor position retains only the canonical key and value wires.
#[derive(Debug)]
struct OrdinalMapEditData {
    root: u32,
    key_len: u32,
    expected_len: u16,
    next_len: u16,
    wire: Vec<u8>,
    memory: Option<super::MemoryLease>,
}

impl OrdinalMapEdit {
    fn wire_len(value: Option<super::MapValue>) -> Result<u16, super::IndexError> {
        value.map_or(Ok(0), |value| {
            u16::try_from(value.encoded_len()?).map_err(|_error| super::IndexError::ResourceLimited)
        })
    }

    fn total_len(
        key_bytes: usize,
        expected_len: u16,
        next_len: u16,
    ) -> Result<usize, super::IndexError> {
        key_bytes
            .checked_add(usize::from(expected_len))
            .and_then(|bytes| bytes.checked_add(usize::from(next_len)))
            .ok_or(super::IndexError::ResourceLimited)
    }

    fn append_value(
        wire: &mut Vec<u8>,
        value: Option<super::MapValue>,
        length: u16,
    ) -> Result<(), super::IndexError> {
        if let Some(value) = value {
            let encoded = value.encoded()?;
            let bytes = encoded.as_slice()?;
            if bytes.len() != usize::from(length) {
                return Err(super::IndexError::BackendViolation(
                    "map value wire length changed during cursor construction".into(),
                ));
            }
            wire.extend_from_slice(bytes);
        } else if length != 0 {
            return Err(super::IndexError::BackendViolation(
                "absent map value has a cursor wire".into(),
            ));
        }
        Ok(())
    }

    /// Standalone caller-owned input. Native producers use `copy_key` instead.
    pub fn unscoped(root: u32, edit: MapEdit) -> Result<Self, super::IndexError> {
        let key_len =
            u32::try_from(edit.key.len()).map_err(|_error| super::IndexError::ResourceLimited)?;
        let expected_len = Self::wire_len(edit.expected)?;
        let next_len = Self::wire_len(edit.next)?;
        let total = Self::total_len(edit.key.len(), expected_len, next_len)?;
        let mut wire = edit.key;
        wire.try_reserve_exact(total - wire.len())
            .map_err(|_error| super::IndexError::ResourceLimited)?;
        Self::append_value(&mut wire, edit.expected, expected_len)?;
        Self::append_value(&mut wire, edit.next, next_len)?;
        if wire.len() != total {
            return Err(super::IndexError::BackendViolation(
                "cursor wire length differs from canonical values".into(),
            ));
        }
        Ok(Self {
            inner: Arc::new(OrdinalMapEditData {
                root,
                key_len,
                expected_len,
                next_len,
                wire,
                memory: None,
            }),
        })
    }

    pub fn allocation_bytes(
        key_bytes: usize,
        expected: Option<super::MapValue>,
        next: Option<super::MapValue>,
    ) -> Result<u64, super::IndexError> {
        let _key_len =
            u32::try_from(key_bytes).map_err(|_error| super::IndexError::ResourceLimited)?;
        let total = Self::total_len(key_bytes, Self::wire_len(expected)?, Self::wire_len(next)?)?;
        let bytes = total
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
        budget.checkpoint()?;
        let length = parts.iter().try_fold(0usize, |length, part| {
            length
                .checked_add(part.len())
                .ok_or(super::IndexError::ResourceLimited)
        })?;
        let key_len = u32::try_from(length).map_err(|_error| super::IndexError::ResourceLimited)?;
        let expected_len = Self::wire_len(expected)?;
        let next_len = Self::wire_len(next)?;
        let total = Self::total_len(length, expected_len, next_len)?;
        let memory = budget.reserve_memory(
            Self::allocation_bytes(length, expected, next)?,
            super::MemoryLifetime::Retained,
        )?;
        let mut wire = Vec::new();
        wire.try_reserve_exact(total)
            .map_err(|_error| super::IndexError::ResourceLimited)?;
        // The allocator may grant more than requested; refuse that backing
        // before copying bytes because the retained lease funds exactly `total`.
        if wire.capacity() != total {
            return Err(super::IndexError::ResourceLimited);
        }
        for part in parts {
            wire.extend_from_slice(part);
        }
        budget.checkpoint()?;
        Self::append_value(&mut wire, expected, expected_len)?;
        Self::append_value(&mut wire, next, next_len)?;
        if wire.len() != total {
            return Err(super::IndexError::BackendViolation(
                "cursor wire length differs from canonical values".into(),
            ));
        }
        Ok(Self {
            inner: Arc::new(OrdinalMapEditData {
                root,
                key_len,
                expected_len,
                next_len,
                wire,
                memory,
            }),
        })
    }

    #[must_use]
    pub fn root(&self) -> u32 {
        self.inner.root
    }

    #[must_use]
    pub fn key(&self) -> &[u8] {
        &self.inner.wire[..self.inner.key_len as usize]
    }

    #[must_use]
    pub fn expected_wire(&self) -> Option<&[u8]> {
        let length = usize::from(self.inner.expected_len);
        if length == 0 {
            return None;
        }
        let start = self.inner.key_len as usize;
        Some(&self.inner.wire[start..start + length])
    }

    #[must_use]
    pub fn next_wire(&self) -> Option<&[u8]> {
        let length = usize::from(self.inner.next_len);
        if length == 0 {
            return None;
        }
        let start = self.inner.key_len as usize + usize::from(self.inner.expected_len);
        Some(&self.inner.wire[start..start + length])
    }

    pub fn expected(&self) -> Result<Option<super::MapValue>, super::IndexError> {
        self.expected_wire()
            .map(super::MapValue::decode)
            .transpose()
    }

    pub fn next(&self) -> Result<Option<super::MapValue>, super::IndexError> {
        self.next_wire().map(super::MapValue::decode).transpose()
    }

    #[must_use]
    pub fn memory_bytes(&self) -> Option<u64> {
        self.inner.memory.as_ref().map(super::MemoryLease::bytes)
    }
}

impl PartialEq for OrdinalMapEditData {
    fn eq(&self, other: &Self) -> bool {
        self.root == other.root
            && self.key_len == other.key_len
            && self.expected_len == other.expected_len
            && self.next_len == other.next_len
            && self.wire == other.wire
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
        let next = ObjectRef(Digest([81; 32]));
        let bytes = OrdinalMapEdit::allocation_bytes(7, None, Some(next.into()))?;
        let (work, owner) = work(bytes)?;
        let position =
            OrdinalMapEdit::copy_object_key_parts(2, &[b"path\0", b"ab"], None, Some(next), &work)?;
        let clone = position.clone();
        let next_wire = super::super::MapValue::from(next).encoded()?;
        if position.root() != 2
            || position.key() != b"path\0ab"
            || position.expected_wire().is_some()
            || position.next()? != Some(next.into())
            || position.next_wire() != Some(next_wire.as_slice()?)
            || position.memory_bytes() != Some(bytes)
            || position.key().as_ptr() != clone.key().as_ptr()
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
    fn object_next_fits_compact_original_budget() -> Result<(), IndexError> {
        let key = b"path\0ab";
        let limit = 128_u64
            .checked_add(u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?)
            .and_then(|bytes| bytes.checked_add(33))
            .ok_or(IndexError::ResourceLimited)?;
        let (work, owner) = work(limit)?;
        let next = ObjectRef(Digest([82; 32]));
        let position = OrdinalMapEdit::copy_object_key(2, key, None, Some(next), &work)?;
        let clone = position.clone();
        if position.root() != 2
            || position.key() != key
            || position.expected()?.is_some()
            || position.next()? != Some(next.into())
            || position.memory_bytes().is_none_or(|bytes| bytes > limit)
            || position.key().as_ptr() != clone.key().as_ptr()
        {
            return Err(IndexError::BackendViolation(
                "compact cursor position changed values or shared backing".into(),
            ));
        }
        drop(position);
        if owner.count.load(Ordering::SeqCst) != 0 {
            return Err(IndexError::BackendViolation(
                "compact cursor backing refunded early".into(),
            ));
        }
        drop(clone);
        if owner.count.load(Ordering::SeqCst) != 1 {
            return Err(IndexError::BackendViolation(
                "compact cursor backing was not refunded once".into(),
            ));
        }
        Ok(())
    }
    #[test]
    fn inline_five_children_and_empty_inline_keep_canonical_wires() -> Result<(), IndexError> {
        let first = ObjectRef(Digest([1; 32]));
        let second = ObjectRef(Digest([2; 32]));
        let third = ObjectRef(Digest([3; 32]));
        let fourth = ObjectRef(Digest([4; 32]));
        let full = super::super::MapValue::inline(&[9; 32], [first, second, first, third, fourth])?;
        let empty = super::super::MapValue::inline(&[], [])?;
        let full_wire = full.encoded()?;
        let empty_wire = empty.encoded()?;
        let bytes = OrdinalMapEdit::allocation_bytes(3, Some(empty), Some(full))?;
        let (work, _owner) = work(bytes)?;
        let position = OrdinalMapEdit::copy_key(7, b"key", Some(empty), Some(full), &work)?;
        let same = OrdinalMapEdit::unscoped(
            7,
            MapEdit {
                key: b"key".to_vec(),
                expected: Some(empty),
                next: Some(full),
            },
        )?;
        let absent = OrdinalMapEdit::unscoped(
            7,
            MapEdit {
                key: b"key".to_vec(),
                expected: None,
                next: Some(full),
            },
        )?;
        let actual_next = position.next()?.ok_or(IndexError::ResourceLimited)?;
        if position != same
            || position == absent
            || position.memory_bytes() != Some(bytes)
            || position.expected()? != Some(empty)
            || position.next()? != Some(full)
            || position.expected_wire() != Some(empty_wire.as_slice()?)
            || position.next_wire() != Some(full_wire.as_slice()?)
            || actual_next.children()? != [first, second, first, third, fourth]
        {
            return Err(IndexError::BackendViolation(
                "compact cursor changed inline wire, child order, or equality".into(),
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
