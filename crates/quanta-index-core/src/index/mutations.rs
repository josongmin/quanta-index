//! Replayable, bounded owner mutation streams. A manifest commits to logical
//! items, not to the persistent map's physical page layout.
use std::collections::VecDeque;

use quanta_index_contract::index::{
    Digest, MAX_SOURCE_PATH_BYTES, MapRoot, ObjectRef, OwnerKey, OwnerMutation, OwnerMutationSet,
    ProducerId, SourceCut, SourceRecord, StableOwnerId,
};
use sha2::{Digest as _, Sha256};

use super::{
    IndexError, SourceChange, WorkBudget, WorkUsage, owner_map_key,
    ports::{ImmutableObjectPort, PersistentMapPort},
};

const MUTATION_PAGE: u32 = 128;
const MUTATION_DOMAIN: &[u8] = b"quanta-index-owner-mutation-stream-v1\0";

/// The same accumulator is used while staging and while replaying a sealed
/// manifest. The digest is independent of persistent-map page boundaries.
#[derive(Clone)]
pub struct OwnerMutationDigest {
    hasher: Sha256,
    count: u64,
}

impl Default for OwnerMutationDigest {
    fn default() -> Self {
        let mut hasher = Sha256::new();
        hasher.update(MUTATION_DOMAIN);
        Self { hasher, count: 0 }
    }
}

impl OwnerMutationDigest {
    pub fn push(&mut self, mutation: &OwnerMutation) -> Result<(), IndexError> {
        let key = owner_map_key(mutation.owner);
        let bytes = serde_json::to_vec(mutation).map_err(|error| {
            IndexError::InvalidInput(format!("owner mutation encoding: {error}"))
        })?;
        self.push_encoded(&key, &bytes)
    }

    pub fn push_encoded(&mut self, key: &[u8], bytes: &[u8]) -> Result<(), IndexError> {
        let len = u64::try_from(bytes.len()).map_err(|_error| IndexError::ResourceLimited)?;
        self.count = self
            .count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        self.hasher.update(key);
        self.hasher.update(len.to_be_bytes());
        self.hasher.update(bytes);
        Ok(())
    }

    #[must_use]
    pub fn count(&self) -> u64 {
        self.count
    }

    #[must_use]
    pub fn finish(&self) -> Digest {
        let mut hasher = self.hasher.clone();
        hasher.update(self.count.to_be_bytes());
        Digest(hasher.finalize().into())
    }
}

fn decode_owner_key(key: &[u8]) -> Result<OwnerKey, IndexError> {
    let bytes: &[u8; 38] = key
        .try_into()
        .map_err(|_error| IndexError::CorruptObject("owner mutation map key length".into()))?;
    if bytes.get(..6) != Some(b"owner\0".as_slice()) {
        return Err(IndexError::CorruptObject(
            "owner mutation map key namespace".into(),
        ));
    }
    let mut producer = [0; 16];
    producer.copy_from_slice(
        bytes
            .get(6..22)
            .ok_or_else(|| IndexError::CorruptObject("owner mutation producer key".into()))?,
    );
    let mut stable_id = [0; 16];
    stable_id.copy_from_slice(
        bytes
            .get(22..38)
            .ok_or_else(|| IndexError::CorruptObject("owner mutation stable owner key".into()))?,
    );
    Ok(OwnerKey {
        producer: ProducerId(producer),
        stable_id: StableOwnerId(stable_id),
    })
}

/// A cursor can be recreated from the same immutable manifest after a crash.
/// Every call checks the caller's original budget and emits at most one item.
pub struct OwnerMutationCursor<'a> {
    root: MapRoot,
    expected: Option<(u64, Digest)>,
    maps: &'a dyn PersistentMapPort,
    objects: &'a dyn ImmutableObjectPort,
    page: VecDeque<(Vec<u8>, ObjectRef)>,
    after: Option<Vec<u8>>,
    more: bool,
    digest: OwnerMutationDigest,
    finished: bool,
}

impl<'a> OwnerMutationCursor<'a> {
    #[must_use]
    pub fn new(
        set: OwnerMutationSet,
        maps: &'a dyn PersistentMapPort,
        objects: &'a dyn ImmutableObjectPort,
    ) -> Self {
        Self {
            root: set.root,
            expected: Some((set.count, set.digest)),
            maps,
            objects,
            page: VecDeque::new(),
            after: None,
            more: true,
            digest: OwnerMutationDigest::default(),
            finished: false,
        }
    }

    /// Validates an unsealed staged map to compute its canonical descriptor.
    /// It never treats a prefix as a complete manifest.
    #[must_use]
    pub fn unsealed(
        root: MapRoot,
        maps: &'a dyn PersistentMapPort,
        objects: &'a dyn ImmutableObjectPort,
    ) -> Self {
        Self {
            root,
            expected: None,
            maps,
            objects,
            page: VecDeque::new(),
            after: None,
            more: true,
            digest: OwnerMutationDigest::default(),
            finished: false,
        }
    }

    #[must_use]
    pub fn sealed_set(&self) -> Option<OwnerMutationSet> {
        self.finished.then(|| OwnerMutationSet {
            root: self.root,
            count: self.digest.count(),
            digest: self.digest.finish(),
        })
    }

    pub fn next_mutation(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<Option<OwnerMutation>, IndexError> {
        budget.checkpoint()?;
        if self.finished {
            return Ok(None);
        }
        if self.page.is_empty() && self.more {
            let page = self
                .maps
                .scan(self.root, self.after.as_deref(), MUTATION_PAGE, budget)?;
            if page.entries.is_empty() && page.has_more {
                return Err(IndexError::BackendViolation(
                    "mutation scan returned an empty continuation page".into(),
                ));
            }
            if page.entries.len()
                > usize::try_from(MUTATION_PAGE).map_err(|_error| IndexError::ResourceLimited)?
            {
                return Err(IndexError::BackendViolation(
                    "mutation scan exceeded requested page limit".into(),
                ));
            }
            self.page = page.entries.into();
            self.more = page.has_more;
        }
        let Some((key, reference)) = self.page.pop_front() else {
            if let Some((count, digest)) = self.expected
                && (self.digest.count() != count || self.digest.finish() != digest)
            {
                return Err(IndexError::CorruptObject(
                    "owner mutation manifest count or digest mismatch".into(),
                ));
            }
            self.finished = true;
            return Ok(None);
        };
        if self
            .after
            .as_deref()
            .is_some_and(|previous| previous >= key.as_slice())
        {
            return Err(IndexError::CorruptObject(
                "owner mutations are not strictly sorted".into(),
            ));
        }
        if self
            .expected
            .is_some_and(|(count, _)| self.digest.count() >= count)
        {
            return Err(IndexError::CorruptObject(
                "owner mutation manifest has excess entries".into(),
            ));
        }
        budget.charge(WorkUsage {
            mutations: 1,
            ..WorkUsage::default()
        })?;
        let owner = decode_owner_key(&key)?;
        let stored = self.objects.get(reference, budget)?;
        if stored.registration.reference != reference {
            return Err(IndexError::CorruptObject(
                "owner mutation object reference mismatch".into(),
            ));
        }
        let mutation: OwnerMutation = serde_json::from_slice(&stored.payload)
            .map_err(|error| IndexError::CorruptObject(format!("owner mutation: {error}")))?;
        let canonical = serde_json::to_vec(&mutation)
            .map_err(|error| IndexError::CorruptObject(format!("owner mutation: {error}")))?;
        if mutation.owner != owner
            || canonical != stored.payload
            || stored.registration.children != mutation.object_children()
        {
            return Err(IndexError::CorruptObject(
                "owner mutation key, encoding or edges mismatch".into(),
            ));
        }
        self.digest.push_encoded(&key, &stored.payload)?;
        self.after = Some(key);
        Ok(Some(mutation))
    }
}

/// Reconstructs one complete source contribution at a time from immutable
/// roots and the same sealed owner mutation stream.
pub struct SourceChangeCursor<'a> {
    mutations: OwnerMutationCursor<'a>,
    prior: SourceCut,
    next: SourceCut,
    maps: &'a dyn PersistentMapPort,
    objects: &'a dyn ImmutableObjectPort,
}

impl<'a> SourceChangeCursor<'a> {
    #[must_use]
    pub fn new(
        set: OwnerMutationSet,
        prior: SourceCut,
        next: SourceCut,
        maps: &'a dyn PersistentMapPort,
        objects: &'a dyn ImmutableObjectPort,
    ) -> Self {
        Self {
            mutations: OwnerMutationCursor::new(set, maps, objects),
            prior,
            next,
            maps,
            objects,
        }
    }

    pub fn next_change(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<Option<SourceChange>, IndexError> {
        let Some(mutation) = self.mutations.next_mutation(budget)? else {
            return Ok(None);
        };
        let key = owner_map_key(mutation.owner);
        let old = self.maps.lookup(self.prior.owners, &key, budget)?;
        let new = self.maps.lookup(self.next.owners, &key, budget)?;
        if old != mutation.expected || new != mutation.next {
            return Err(IndexError::CorruptObject(
                "source mutation does not match source roots".into(),
            ));
        }
        let prior = old
            .map(|reference| {
                read_record(reference, self.objects, budget).map(|record| (reference, record))
            })
            .transpose()?;
        let next = new
            .map(|reference| {
                read_record(reference, self.objects, budget).map(|record| (reference, record))
            })
            .transpose()?;
        Ok(Some(SourceChange {
            owner: mutation.owner,
            prior,
            next,
        }))
    }
}

fn read_record(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<SourceRecord, IndexError> {
    let stored = objects.get(reference, budget)?;
    if stored.registration.reference != reference {
        return Err(IndexError::CorruptObject(
            "source record object reference mismatch".into(),
        ));
    }
    let record: SourceRecord = serde_json::from_slice(&stored.payload)
        .map_err(|error| IndexError::CorruptObject(format!("source record: {error}")))?;
    if record.path.is_empty()
        || record.path.len() > MAX_SOURCE_PATH_BYTES
        || record.path.starts_with('/')
        || record.path.contains('\0')
        || record
            .path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || stored.registration.children.as_slice() != [record.bytes]
    {
        return Err(IndexError::CorruptObject(
            "source record path or edges invalid".into(),
        ));
    }
    Ok(record)
}
