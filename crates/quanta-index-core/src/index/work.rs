//! Canonical bounded publication work stream shared by Kernel and Catalog.
use sha2::{Digest as _, Sha256};

use super::{Digest, IndexError, MemoryLifetime, ObjectRef, WorkBudget, WorkIntent, WorkScope};

/// Work intents contain only fixed-width identities and scalar fields.
pub const MAX_WORK_ACTION_BYTES: usize = 16 * 1024;

/// Sorted, deduplicated physical edges for fixed records with at most four references.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectChildren {
    Empty,
    One([ObjectRef; 1]),
    Two([ObjectRef; 2]),
    Three([ObjectRef; 3]),
    Four([ObjectRef; 4]),
}

impl ObjectChildren {
    #[must_use]
    pub fn as_slice(&self) -> &[ObjectRef] {
        match self {
            Self::Empty => &[],
            Self::One(values) => values.as_slice(),
            Self::Two(values) => values.as_slice(),
            Self::Three(values) => values.as_slice(),
            Self::Four(values) => values.as_slice(),
        }
    }

    #[must_use]
    pub fn from_references(mut values: [Option<ObjectRef>; 4]) -> Self {
        values.sort_unstable();
        let mut previous = None;
        let unique = values.into_iter().flatten().filter(|reference| {
            let fresh = previous != Some(*reference);
            previous = Some(*reference);
            fresh
        });
        let mut references = [ObjectRef(Digest([0; 32])); 4];
        let mut count = 0usize;
        // Both the input and destination have exactly four slots; zip cannot
        // truncate a distinct input. No allocation or fallible indexing occurs.
        for (slot, reference) in references.iter_mut().zip(unique) {
            *slot = reference;
            count = count.saturating_add(1);
        }
        let [a, b, c, d] = references;
        match count {
            0 => Self::Empty,
            1 => Self::One([a]),
            2 => Self::Two([a, b]),
            3 => Self::Three([a, b, c]),
            _ => Self::Four([a, b, c, d]),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkAction {
    Retire(WorkIntent),
    Create(WorkIntent),
}

impl WorkAction {
    /// Decode one bounded work action and require its exact canonical JSON bytes.
    pub fn decode_canonical(payload: &[u8], budget: &WorkBudget) -> Result<Self, IndexError> {
        if payload.len() > MAX_WORK_ACTION_BYTES {
            return Err(IndexError::CorruptObject(
                "work action exceeds fixed-field encoding bound".into(),
            ));
        }
        let decode_bytes = payload
            .len()
            .checked_mul(8)
            .and_then(|bytes| bytes.checked_add(512))
            .ok_or(IndexError::ResourceLimited)?;
        let _decode_memory = budget.reserve_memory(
            u64::try_from(decode_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let action: WorkAction = serde_json::from_slice(payload)
            .map_err(|error| IndexError::CorruptObject(format!("work action: {error}")))?;
        let mut writer = CanonicalWorkWriter {
            expected: payload,
            offset: 0,
            budget,
            error: None,
        };
        if let Err(error) = serde_json::to_writer(&mut writer, &action) {
            return Err(writer
                .error
                .take()
                .unwrap_or_else(|| IndexError::CorruptObject(format!("work action: {error}"))));
        }
        if writer.offset != payload.len() {
            return Err(IndexError::CorruptObject(
                "work action encoding length mismatch".into(),
            ));
        }
        Ok(action)
    }

    /// Ordering is retire first, then create, and strict intent identity within
    /// each phase. It is independent of physical manifest page partition.
    pub fn key(&self) -> Result<[u8; 33], IndexError> {
        let (tag, intent) = match self {
            Self::Retire(intent) => (0, intent),
            Self::Create(intent) => (1, intent),
        };
        let digest = intent
            .identity_digest()
            .map_err(|error| IndexError::InvalidInput(format!("work intent identity: {error}")))?;
        let mut key = [0; 33];
        key[0] = tag;
        key[1..].copy_from_slice(&digest.0);
        Ok(key)
    }

    /// Retirement compares historical stamps; only created work owns current
    /// immutable input dependencies.
    #[must_use]
    pub fn children(&self) -> ObjectChildren {
        let Self::Create(intent) = self else {
            return ObjectChildren::Empty;
        };
        match intent.scope {
            WorkScope::Owner { expected, .. } => {
                ObjectChildren::from_references([expected.source, expected.projection, None, None])
            }
            WorkScope::ProjectionDomain { source, .. } => ObjectChildren::from_references([
                source.owners.0,
                source.paths.0,
                source.frontiers.0,
                None,
            ]),
        }
    }
}

/// Count and identity commitment for one frozen, attempt-fenced work stream.
/// Its producer retains the scratch owner through Catalog publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkActionSet {
    pub count: u64,
    pub digest: Digest,
}

impl WorkActionSet {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            count: 0,
            digest: WorkActionDigest::default().finish(),
        }
    }
}

pub struct WorkActionDigest {
    hash: Sha256,
    count: u64,
    previous: Option<[u8; 33]>,
}

impl Default for WorkActionDigest {
    fn default() -> Self {
        let mut hash = Sha256::new();
        hash.update(b"qidx.native.work-actions.v1\0");
        Self {
            hash,
            count: 0,
            previous: None,
        }
    }
}

impl WorkActionDigest {
    pub fn push(&mut self, action: &WorkAction) -> Result<(), IndexError> {
        let key = action.key()?;
        if self.previous.as_ref().is_some_and(|prior| prior >= &key) {
            return Err(IndexError::InvalidInput(
                "work actions must be sorted and unique".into(),
            ));
        }
        let count = self
            .count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        self.hash.update(key);
        self.previous = Some(key);
        self.count = count;
        Ok(())
    }

    #[must_use]
    pub fn count(&self) -> u64 {
        self.count
    }

    #[must_use]
    pub fn finish(&self) -> Digest {
        let mut hash = self.hash.clone();
        hash.update(self.count.to_be_bytes());
        Digest(hash.finalize().into())
    }
}

quanta_index_contract::native_serde_enum!(WorkAction { units { } newtypes { Retire = 0 => WorkIntent, Create = 1 => WorkIntent } records { } });
quanta_index_contract::native_serde_struct!(WorkActionSet {
    count: u64,
    digest: Digest
});

/// Compare canonical JSON directly against borrowed immutable bytes.
struct CanonicalWorkWriter<'a> {
    expected: &'a [u8],
    offset: usize,
    budget: &'a WorkBudget,
    error: Option<IndexError>,
}

impl std::io::Write for CanonicalWorkWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Err(error) = self.budget.checkpoint() {
            self.error = Some(error);
            return Err(std::io::Error::other("work validation cancelled"));
        }
        let end = self
            .offset
            .checked_add(bytes.len())
            .ok_or_else(|| std::io::Error::other("work validation offset overflow"))?;
        if self.expected.get(self.offset..end) != Some(bytes) {
            return Err(std::io::Error::other("work JSON is not canonical"));
        }
        self.offset = end;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{
        CollectionId, IndexInstanceId, MapRoot, MemoryLease, MemoryLeaseOwner, MemoryScope,
        MemoryScopePort, ProjectionId, SlotId, TenantId, WorkLimits, WorkspaceKey, WorktreeId,
    };
    use std::{
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    fn require(value: bool) -> Result<(), IndexError> {
        if value {
            Ok(())
        } else {
            Err(IndexError::BackendViolation(
                "work wire oracle mismatch".into(),
            ))
        }
    }
    fn intent() -> WorkIntent {
        WorkIntent {
            workspace: WorkspaceKey {
                tenant: TenantId([255; 16]),
                collection: CollectionId([255; 16]),
                worktree: WorktreeId([255; 16]),
                slot: SlotId([255; 16]),
            },
            epoch: u64::MAX,
            instance: IndexInstanceId([255; 16]),
            scope: WorkScope::ProjectionDomain {
                projection: ProjectionId([255; 16]),
                source: super::super::SourceCut {
                    owners: MapRoot(Some(ObjectRef(Digest([255; 32])))),
                    paths: MapRoot(Some(ObjectRef(Digest([254; 32])))),
                    frontiers: MapRoot(Some(ObjectRef(Digest([253; 32])))),
                },
                producer_policy: Digest([255; 32]),
                dependency_context: Digest([255; 32]),
            },
        }
    }
    fn budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    #[test]
    fn fixed_keys_and_work_digest_match_preexisting_json_hash_contract()
    -> Result<(), Box<dyn std::error::Error>> {
        let intent = intent();
        let bytes = serde_json::to_vec(&intent)?;
        let mut hash = Sha256::new();
        hash.update(b"quanta-index-owner-work-v1\0");
        hash.update(&bytes);
        let identity = Digest(hash.finalize().into());
        require(intent.identity_digest()? == identity)?;
        let mut expected = Sha256::new();
        expected.update(b"qidx.native.work-actions.v1\0");
        let mut actual = WorkActionDigest::default();
        for (tag, action) in [
            (0_u8, WorkAction::Retire(intent)),
            (1, WorkAction::Create(intent)),
        ] {
            let mut key = vec![tag];
            key.extend_from_slice(&identity.0);
            require(action.key()?.as_slice() == key)?;
            expected.update(&key);
            actual.push(&action)?;
            let payload = serde_json::to_vec(&action)?;
            require(payload.len() <= MAX_WORK_ACTION_BYTES)?;
            require(WorkAction::decode_canonical(&payload, &budget()?)? == action)?;
        }
        expected.update(2_u64.to_be_bytes());
        require(actual.finish() == Digest(expected.finalize().into()))?;
        require(actual.push(&WorkAction::Create(intent)).is_err())?;
        Ok(())
    }

    #[test]
    fn inline_four_children_match_independent_set_for_all_nullable_duplicate_shapes()
    -> Result<(), IndexError> {
        let choices = [
            None,
            Some(ObjectRef(Digest([0; 32]))),
            Some(ObjectRef(Digest([1; 32]))),
            Some(ObjectRef(Digest([2; 32]))),
            Some(ObjectRef(Digest([3; 32]))),
        ];
        for a in choices {
            for b in choices {
                for c in choices {
                    for d in choices {
                        let input = [a, b, c, d];
                        let gold: std::collections::BTreeSet<_> =
                            input.into_iter().flatten().collect();
                        let actual = ObjectChildren::from_references(input);
                        require(actual.as_slice().iter().copied().eq(gold.into_iter()))?;
                    }
                }
            }
        }
        Ok(())
    }

    #[test]
    fn inline_child_sets_keep_null_sort_and_duplicate_contract() -> Result<(), IndexError> {
        let a = ObjectRef(Digest([1; 32]));
        let b = ObjectRef(Digest([2; 32]));
        let c = ObjectRef(Digest([3; 32]));
        for (input, gold) in [
            ([None, None, None], vec![]),
            ([Some(a), None, None], vec![a]),
            ([None, Some(a), None], vec![a]),
            ([None, None, Some(a)], vec![a]),
            ([Some(b), Some(a), None], vec![a, b]),
            ([Some(b), None, Some(a)], vec![a, b]),
            ([None, Some(b), Some(a)], vec![a, b]),
            ([Some(c), Some(a), Some(b)], vec![a, b, c]),
            ([Some(a), Some(a), Some(a)], vec![a]),
            ([Some(b), Some(a), Some(a)], vec![a, b]),
        ] {
            let [a, b, c] = input;
            require(ObjectChildren::from_references([a, b, c, None]).as_slice() == gold)?;
        }
        require(
            WorkAction::Retire(intent())
                .children()
                .as_slice()
                .is_empty(),
        )
    }

    #[test]
    fn canonical_decode_rejects_whitespace_and_admits_parser_before_decode()
    -> Result<(), Box<dyn std::error::Error>> {
        struct Ledger(AtomicU64);
        impl MemoryLeaseOwner for Ledger {
            fn release(&self, bytes: u64) {
                let _prior = self.0.fetch_sub(bytes, Ordering::SeqCst);
            }
        }
        struct Scope {
            ledger: Arc<Ledger>,
            deny: bool,
        }
        impl MemoryScopePort for Scope {
            fn reserve(
                &self,
                bytes: u64,
                lifetime: MemoryLifetime,
            ) -> Result<MemoryLease, IndexError> {
                if self.deny || lifetime != MemoryLifetime::Temporary {
                    return Err(IndexError::ResourceLimited);
                }
                let _prior = self.ledger.0.fetch_add(bytes, Ordering::SeqCst);
                Ok(MemoryLease::new(bytes, self.ledger.clone()))
            }
        }
        let action = WorkAction::Create(intent());
        let payload = serde_json::to_vec(&action)?;
        let ledger = Arc::new(Ledger(AtomicU64::new(0)));
        let mut work = budget()?;
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope {
            ledger: ledger.clone(),
            deny: true,
        }))));
        require(matches!(
            WorkAction::decode_canonical(b"malformed", &work),
            Err(IndexError::ResourceLimited)
        ))?;
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope {
            ledger: ledger.clone(),
            deny: false,
        }))));
        require(WorkAction::decode_canonical(&payload, &work)? == action)?;
        let mut noncanonical = vec![b' '];
        noncanonical.extend_from_slice(&payload);
        require(matches!(
            WorkAction::decode_canonical(&noncanonical, &work),
            Err(IndexError::CorruptObject(_))
        ))?;
        require(ledger.0.load(Ordering::SeqCst) == 0)?;
        work.request().cancel_handle().cancel();
        require(matches!(
            WorkAction::decode_canonical(&payload, &work),
            Err(IndexError::Cancelled)
        ))?;
        Ok(())
    }
}
