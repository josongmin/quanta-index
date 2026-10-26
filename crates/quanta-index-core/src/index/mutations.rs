//! Replayable, bounded owner mutation streams. A manifest commits to logical
//! items, not to the persistent map's physical page layout.
mod canonical;
mod source_change_diff;

use super::{MapPageCursor, MapPageEntry, MapValue};

use quanta_index_contract::index::{
    Digest, MAX_SOURCE_PATH_BYTES, MapRoot, ObjectRef, OwnerKey, OwnerMutation, OwnerMutationSet,
    ProducerId, SourceCut, StableOwnerId,
};
use sha2::{Digest as _, Sha256};

use super::{
    DeltaInput, IndexError, SourceChange, SourceDelta, SourceRecordOwner, WorkBudget, WorkUsage,
    owner_map_key,
    ports::{ImmutableObjectPort, PersistentMapPort},
};
use source_change_diff::SourceDiffPages;

const MUTATION_PAGE: u32 = 128;
const MUTATION_DOMAIN: &[u8] = b"quanta-index-owner-mutation-stream-v1\0";
const MAX_OWNER_MUTATION_OBJECT_BYTES: usize = 4096;

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
        self.push_inner(mutation, None)
    }

    /// Bill the logical canonical bytes before hashing; no JSON buffer is born.
    pub fn push_budgeted(
        &mut self,
        mutation: &OwnerMutation,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.push_inner(mutation, Some(budget))
    }

    fn push_inner(
        &mut self,
        mutation: &OwnerMutation,
        mut budget: Option<&mut WorkBudget>,
    ) -> Result<(), IndexError> {
        let len = canonical::length(mutation, budget.as_deref())?;
        let count = self
            .count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if let Some(work) = budget.as_deref_mut() {
            work.charge(WorkUsage {
                bytes: len,
                ..WorkUsage::default()
            })?;
        }
        let mut hasher = self.hasher.clone();
        hasher.update(owner_map_key(mutation.owner));
        hasher.update(len.to_be_bytes());
        if canonical::hash(mutation, &mut hasher, budget.as_deref())? != len {
            return Err(IndexError::BackendViolation(
                "mutation commitment length changed".into(),
            ));
        }
        self.hasher = hasher;
        self.count = count;
        Ok(())
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

/// The owner is already authenticated by the map key. Only next is a real
/// physical edge; expected is historical CAS data, including an all-zero digest.
pub fn owner_mutation_value(mutation: &OwnerMutation) -> Result<MapValue, IndexError> {
    let expected = mutation
        .expected
        .as_ref()
        .map_or(&[][..], |reference| reference.0.0.as_slice());
    MapValue::inline(expected, mutation.next)
}

fn inline_mutation(
    owner: OwnerKey,
    value: super::InlineMapValue,
) -> Result<OwnerMutation, IndexError> {
    let expected = match value.data()? {
        [] => None,
        bytes if bytes.len() == 32 => {
            Some(ObjectRef(Digest(bytes.try_into().map_err(|_error| {
                IndexError::CorruptObject("mutation CAS stamp width".into())
            })?)))
        }
        _ => return Err(IndexError::CorruptObject("mutation CAS stamp width".into())),
    };
    Ok(OwnerMutation {
        owner,
        expected,
        next: value.single_child()?,
    })
}

/// Decode one already-addressed mutation map value.
///
/// The map key authenticates `owner`; legacy objects must additionally match their registration, exact
/// canonical JSON, and physical next-child edge. No manifest scan is needed.
///
/// The caller owns the mutation-count fee; only the object branch adds decode
/// and canonical-comparison byte work on top of the object port's read fee.
pub fn decode_owner_mutation_value(
    owner: OwnerKey,
    value: MapValue,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<OwnerMutation, IndexError> {
    budget.checkpoint()?;
    match value {
        MapValue::Inline(value) => inline_mutation(owner, value),
        MapValue::Object(reference) => {
            let stored = objects.get(reference, budget)?;
            if stored.payload.len() > MAX_OWNER_MUTATION_OBJECT_BYTES
                || stored.registration.reference != reference
                // Registered bytes include the store's canonical wire framing
                // and child references. The object port verifies that framing.
                || stored.registration.bytes
                    < u64::try_from(stored.payload.len())
                        .map_err(|_error| IndexError::ResourceLimited)?
            {
                return Err(IndexError::CorruptObject(
                    "owner mutation object registration or size mismatch".into(),
                ));
            }
            let decode_bytes = u64::try_from(stored.payload.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?;
            budget.charge(WorkUsage {
                bytes: decode_bytes,
                ..WorkUsage::default()
            })?;
            let mutation: OwnerMutation = serde_json::from_slice(&stored.payload)
                .map_err(|error| IndexError::CorruptObject(format!("owner mutation: {error}")))?;
            if mutation.owner != owner
                || !canonical::matches(&mutation, &stored.payload, budget)?
                || stored.registration.children.as_slice() != mutation.next.as_slice()
            {
                return Err(IndexError::CorruptObject(
                    "owner mutation key, encoding or edges mismatch".into(),
                ));
            }
            budget.checkpoint()?;
            Ok(mutation)
        }
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

enum MutationPosition {
    Initial,
    At(MapPageEntry),
    Finished,
    Failed(IndexError),
}
impl MutationPosition {
    fn entry(&self) -> Option<&MapPageEntry> {
        match self {
            Self::At(entry) => Some(entry),
            Self::Initial | Self::Finished | Self::Failed(_) => None,
        }
    }
}

/// A cursor can be recreated from the same immutable manifest after a crash.
/// Every call checks the caller's original budget and emits at most one item.
pub struct OwnerMutationCursor<'a> {
    root: MapRoot,
    expected: Option<(u64, Digest)>,
    maps: &'a dyn PersistentMapPort,
    objects: &'a dyn ImmutableObjectPort,
    page: MapPageCursor,
    position: MutationPosition,
    more: bool,
    digest: OwnerMutationDigest,
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
            page: MapPageCursor::default(),
            position: MutationPosition::Initial,
            more: true,
            digest: OwnerMutationDigest::default(),
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
            page: MapPageCursor::default(),
            position: MutationPosition::Initial,
            more: true,
            digest: OwnerMutationDigest::default(),
        }
    }

    #[must_use]
    pub fn sealed_set(&self) -> Option<OwnerMutationSet> {
        matches!(self.position, MutationPosition::Finished).then(|| OwnerMutationSet {
            root: self.root,
            count: self.digest.count(),
            digest: self.digest.finish(),
        })
    }

    pub fn next_mutation(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<Option<OwnerMutation>, IndexError> {
        if let MutationPosition::Failed(error) = &self.position {
            return Err(error.clone());
        }
        let result = self.next_inner(budget);
        if let Err(error) = &result {
            self.position = MutationPosition::Failed(error.clone());
        }
        result
    }

    fn next_inner(&mut self, budget: &mut WorkBudget) -> Result<Option<OwnerMutation>, IndexError> {
        budget.checkpoint()?;
        if matches!(self.position, MutationPosition::Finished) {
            return Ok(None);
        }
        if self.page.is_empty() && self.more {
            // The declared remainder bounds allocation, never completeness.
            // A final one-row probe must still detect an understated count.
            let limit = match self.expected {
                Some((count, _)) => u32::try_from(
                    count
                        .saturating_sub(self.digest.count())
                        .clamp(1, u64::from(MUTATION_PAGE)),
                )
                .map_err(|_error| IndexError::ResourceLimited)?,
                None => MUTATION_PAGE,
            };
            let page = self.maps.scan(
                self.root,
                self.position.entry().map(MapPageEntry::key).transpose()?,
                limit,
                budget,
            )?;
            if page.entries.is_empty() && page.has_more {
                return Err(IndexError::BackendViolation(
                    "mutation scan returned an empty continuation page".into(),
                ));
            }
            if page.entries.len()
                > usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?
            {
                return Err(IndexError::BackendViolation(
                    "mutation scan exceeded requested page limit".into(),
                ));
            }
            self.more = page.has_more;
            self.page = page.cursor();
        }
        let Some(entry) = self.page.next() else {
            if let Some((count, digest)) = self.expected
                && (self.digest.count() != count || self.digest.finish() != digest)
            {
                return Err(IndexError::CorruptObject(
                    "owner mutation manifest count or digest mismatch".into(),
                ));
            }
            self.position = MutationPosition::Finished;
            return Ok(None);
        };
        let (key, value) = entry.get()?;
        if self
            .position
            .entry()
            .map(MapPageEntry::key)
            .transpose()?
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
        let owner = decode_owner_key(key)?;
        let mutation = decode_owner_mutation_value(owner, *value, self.objects, budget)?;
        self.digest.push_budgeted(&mutation, budget)?;
        self.position = MutationPosition::At(entry);
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
    diff: SourceDiffPages<'a>,
    failure: Option<IndexError>,
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
            diff: SourceDiffPages::new(prior.owners, next.owners, maps),
            failure: None,
        }
    }

    pub fn next_change(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<Option<SourceChange>, IndexError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.next_unchecked(budget);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }

    /// Validate the complete sealed mutation against both source roots without
    /// loading record payloads. The consumer must already own record validation;
    /// this proves owner/reference changes, not source-record shape or contents.
    pub fn next_mutation(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<Option<OwnerMutation>, IndexError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.next_mutation_unchecked(budget);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }

    fn next_unchecked(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<Option<SourceChange>, IndexError> {
        let Some(mutation) = self.next_mutation_unchecked(budget)? else {
            return Ok(None);
        };
        let prior = mutation
            .expected
            .map(|reference| {
                read_record(reference, self.objects, budget).map(|record| (reference, record))
            })
            .transpose()?;
        let next = mutation
            .next
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

    fn next_mutation_unchecked(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<Option<OwnerMutation>, IndexError> {
        budget.checkpoint()?;
        let Some(mutation) = self.mutations.next_mutation(budget)? else {
            if self.diff.peek(budget)?.is_some() {
                return Err(IndexError::CorruptObject(
                    "source roots contain a change absent from the sealed mutation".into(),
                ));
            }
            return Ok(None);
        };
        let key = owner_map_key(mutation.owner);
        let old = mutation.expected;
        let new = mutation.next;
        if old == new {
            // Equal values are omitted by structural diff. Keep the original
            // point proof for an explicit no-op mutation.
            if let Some(changed) = self.diff.peek(budget)?
                && owner_map_key(changed.owner) <= key
            {
                return Err(IndexError::CorruptObject(
                    "source roots contain an unexpected changed owner".into(),
                ));
            }
            let before = self.maps.lookup_object(self.prior.owners, &key, budget)?;
            let after = self.maps.lookup_object(self.next.owners, &key, budget)?;
            if before != old || after != new {
                return Err(IndexError::CorruptObject(
                    "source mutation does not match source roots".into(),
                ));
            }
        } else {
            let Some(changed) = self.diff.peek(budget)? else {
                return Err(IndexError::CorruptObject(
                    "sealed source mutation is absent from root diff".into(),
                ));
            };
            if changed.owner != mutation.owner || changed.expected != old || changed.next != new {
                return Err(IndexError::CorruptObject(
                    "source mutation does not match root diff".into(),
                ));
            }
            self.diff.advance()?;
        }
        Ok(Some(mutation))
    }
}

fn read_record(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<SourceRecordOwner, IndexError> {
    let stored = objects.get(reference, budget)?;
    if stored.registration.reference != reference {
        return Err(IndexError::CorruptObject(
            "source record object reference mismatch".into(),
        ));
    }
    let record =
        SourceRecordOwner::decode_json(&stored.payload, budget, super::MemoryLifetime::Retained)?;
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

/// Reads one immutable source owner at a time for first-time adapter build.
/// No synthetic mutation manifest or corpus-sized owner buffer is created.
pub struct SourceSnapshotCursor<'a> {
    source: SourceCut,
    maps: &'a dyn PersistentMapPort,
    objects: &'a dyn ImmutableObjectPort,
    page: MapPageCursor,
    after: Option<MapPageEntry>,
    more: bool,
    finished: bool,
    failure: Option<IndexError>,
}

impl<'a> SourceSnapshotCursor<'a> {
    #[must_use]
    pub fn new(
        source: SourceCut,
        maps: &'a dyn PersistentMapPort,
        objects: &'a dyn ImmutableObjectPort,
    ) -> Self {
        Self {
            source,
            maps,
            objects,
            page: MapPageCursor::default(),
            after: None,
            more: true,
            finished: false,
            failure: None,
        }
    }

    pub fn next_change(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<Option<SourceChange>, IndexError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.next_unchecked(budget);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }

    fn next_unchecked(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<Option<SourceChange>, IndexError> {
        budget.checkpoint()?;
        loop {
            if let Some(entry) = self.page.next() {
                let (key, reference) = entry.object_parts()?;
                let reference = *reference;
                budget.charge(WorkUsage {
                    reads: 1,
                    bytes: u64::try_from(key.len())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                let owner = decode_owner_key(key)?;
                self.after = Some(entry);
                let record = read_record(reference, self.objects, budget)?;
                return Ok(Some(SourceChange {
                    owner,
                    prior: None,
                    next: Some((reference, record)),
                }));
            }
            if self.finished || !self.more {
                self.finished = true;
                return Ok(None);
            }
            let page = self.maps.scan(
                self.source.owners,
                self.after.as_ref().map(MapPageEntry::key).transpose()?,
                MUTATION_PAGE,
                budget,
            )?;
            if page.entries.len()
                > usize::try_from(MUTATION_PAGE).map_err(|_error| IndexError::ResourceLimited)?
                || (page.entries.is_empty() && page.has_more)
            {
                return Err(IndexError::BackendViolation(
                    "source snapshot scan exceeded its page or did not advance".into(),
                ));
            }
            let mut prior = self.after.as_ref().map(MapPageEntry::key).transpose()?;
            for (key, _) in &page.entries {
                if prior.is_some_and(|value| key.as_slice() <= value) {
                    return Err(IndexError::BackendViolation(
                        "source snapshot scan is not strictly ordered".into(),
                    ));
                }
                let _owner = decode_owner_key(key)?;
                prior = Some(key);
            }
            self.more = page.has_more;
            self.page = page.cursor();
        }
    }
}

/// The same adapter processing loop accepts a first snapshot or a sealed
/// source delta. A patch must target the exact selected source cut.
enum SourceInputMode<'a> {
    Snapshot(Box<SourceSnapshotCursor<'a>>),
    Delta(Box<SourceChangeCursor<'a>>),
}

pub struct SourceInputCursor<'a> {
    mode: SourceInputMode<'a>,
    failure: Option<IndexError>,
}

impl<'a> SourceInputCursor<'a> {
    pub fn new(
        source: SourceCut,
        input: DeltaInput<SourceDelta>,
        maps: &'a dyn PersistentMapPort,
        objects: &'a dyn ImmutableObjectPort,
    ) -> Result<Self, IndexError> {
        match input {
            DeltaInput::Bootstrap => Ok(Self {
                mode: SourceInputMode::Snapshot(Box::new(SourceSnapshotCursor::new(
                    source, maps, objects,
                ))),
                failure: None,
            }),
            DeltaInput::Patch(delta) if delta.next == source => Ok(Self {
                mode: SourceInputMode::Delta(Box::new(SourceChangeCursor::new(
                    delta.changed,
                    delta.prior,
                    delta.next,
                    maps,
                    objects,
                ))),
                failure: None,
            }),
            DeltaInput::Patch(_) => Err(IndexError::Conflict),
        }
    }

    pub fn next_change(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<Option<SourceChange>, IndexError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = match &mut self.mode {
            SourceInputMode::Snapshot(cursor) => cursor.next_change(budget),
            SourceInputMode::Delta(cursor) => cursor.next_change(budget),
        };
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    use super::*;
    use crate::index::{
        MapBatchRequest, MapBatchWrite, MapEdit, MapPage, MapWrite, MemoryLease, MemoryLeaseOwner,
        MemoryLifetime, MemoryScope, MemoryScopePort, ObjectBatchInput, RegisteredObject,
        SourceRecord, StoredObject, WorkLimits,
        ports::{MapDiffRange, MapDiffVisitor, MapEditCursor, ObjectWriteSink},
    };

    // Preserve single evaluation and borrowed operands while reporting test failures as errors.
    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(IndexError::BackendViolation(format!(
                    "assertion failed: {}",
                    stringify!($condition)
                )));
            }
        }};
    }
    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) => {
                    if left != right {
                        return Err(IndexError::BackendViolation(format!(
                            "assertion failed: {} == {}; left: {left:?}, right: {right:?}",
                            stringify!($left),
                            stringify!($right)
                        )));
                    }
                }
            }
        }};
    }

    mod inline_values;
    mod point_value;

    type DiffGold = Vec<(Vec<u8>, Option<MapValue>, Option<MapValue>)>;

    struct Maps {
        entries: Vec<(Vec<u8>, ObjectRef)>,
        mutation_values: Option<Vec<(Vec<u8>, MapValue)>>,
        diff_gold: Option<DiffGold>,
        diff_more: Option<bool>,
        lookup_values: Vec<(MapRoot, Vec<u8>, Option<ObjectRef>)>,
        forced: Option<MapPage>,
        max_scan_limit: Option<u32>,
        scans: AtomicUsize,
        diff_pages: AtomicUsize,
        lookups: AtomicUsize,
    }

    impl PersistentMapPort for Maps {
        fn lookup_object(
            &self,
            root: MapRoot,
            key: &[u8],
            _budget: &mut WorkBudget,
        ) -> Result<Option<ObjectRef>, IndexError> {
            let value = self
                .lookup_values
                .iter()
                .find(|(candidate, candidate_key, _)| *candidate == root && candidate_key == key)
                .ok_or_else(|| IndexError::BackendViolation("unexpected source lookup".into()))?;
            let _previous = self.lookups.fetch_add(1, Ordering::Relaxed);
            Ok(value.2)
        }
        fn lookup(
            &self,
            root: MapRoot,
            key: &[u8],
            budget: &mut WorkBudget,
        ) -> Result<Option<crate::index::MapValue>, IndexError> {
            self.lookup_object(root, key, budget)
                .map(|value| value.map(Into::into))
        }

        fn scan(
            &self,
            _root: MapRoot,
            after: Option<&[u8]>,
            limit: u32,
            _budget: &mut WorkBudget,
        ) -> Result<MapPage, IndexError> {
            let _previous = self.scans.fetch_add(1, Ordering::Relaxed);
            if self.max_scan_limit.is_some_and(|cap| limit > cap) {
                return Err(IndexError::ResourceLimited);
            }
            if let Some(page) = &self.forced {
                return Ok(page.clone());
            }
            let remaining: Vec<_> = self.mutation_values.as_ref().map_or_else(
                || {
                    self.entries
                        .iter()
                        .filter(|(key, _)| after.is_none_or(|prior| key.as_slice() > prior))
                        .map(|(key, value)| (key.clone(), (*value).into()))
                        .collect()
                },
                |values| {
                    values
                        .iter()
                        .filter(|(key, _)| after.is_none_or(|prior| key.as_slice() > prior))
                        .cloned()
                        .collect()
                },
            );
            let limit = usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?;
            let has_more = remaining.len() > limit;
            Ok(MapPage::new(
                remaining.into_iter().take(limit).collect(),
                has_more,
                Vec::new(),
                None,
            ))
        }

        fn visit_diff(
            &self,
            _before: MapRoot,
            _after: MapRoot,
            range: MapDiffRange<'_>,
            visitor: &mut dyn MapDiffVisitor,
            budget: &mut WorkBudget,
        ) -> Result<bool, IndexError> {
            let gold = self.diff_gold.as_ref().ok_or_else(|| {
                IndexError::BackendViolation("test map has no structural diff gold".into())
            })?;
            let _previous = self.diff_pages.fetch_add(1, Ordering::Relaxed);
            let mut visited = 0_u32;
            let mut more = false;
            for (key, expected, next) in gold.iter().filter(|(key, _, _)| {
                key.starts_with(range.prefix)
                    && range.after.is_none_or(|after| key.as_slice() > after)
            }) {
                if visited == range.limit {
                    more = true;
                    break;
                }
                visitor.changed(key, *expected, *next, budget)?;
                visited = visited.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
            Ok(self.diff_more.unwrap_or(more))
        }

        fn apply(
            &self,
            _base: MapRoot,
            _edits: &[MapEdit],
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<MapWrite, IndexError> {
            Err(IndexError::BackendViolation(
                "snapshot apply is unexpected".into(),
            ))
        }

        fn apply_batch(
            &self,
            _requests: &[MapBatchRequest],
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<MapBatchWrite, IndexError> {
            Err(IndexError::BackendViolation(
                "snapshot apply batch is unexpected".into(),
            ))
        }

        fn apply_stream(
            &self,
            _bases: &[MapRoot],
            _edits: &mut dyn MapEditCursor,
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<crate::index::StagedMapRoots, IndexError> {
            Err(IndexError::BackendViolation(
                "snapshot apply stream is unexpected".into(),
            ))
        }
    }

    struct Objects(BTreeMap<ObjectRef, StoredObject>);

    impl ImmutableObjectPort for Objects {
        fn identify(
            &self,
            _payload: &[u8],
            _children: &[ObjectRef],
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation(
                "snapshot identify is unexpected".into(),
            ))
        }

        fn put(
            &self,
            _payload: &[u8],
            _children: &[ObjectRef],
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation(
                "snapshot put is unexpected".into(),
            ))
        }

        fn install(
            &self,
            _payload: &[u8],
            _children: &[ObjectRef],
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation(
                "snapshot install is unexpected".into(),
            ))
        }

        fn install_batch(
            &self,
            _inputs: &[ObjectBatchInput],
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<crate::index::RegisteredObjectBatch, IndexError> {
            Err(IndexError::BackendViolation(
                "snapshot install batch is unexpected".into(),
            ))
        }

        fn get(
            &self,
            reference: ObjectRef,
            _budget: &mut WorkBudget,
        ) -> Result<StoredObject, IndexError> {
            self.0
                .get(&reference)
                .cloned()
                .ok_or_else(|| IndexError::CorruptObject("source record missing".into()))
        }
    }

    fn budget() -> Result<WorkBudget, IndexError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?;
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 1_000,
                writes: 0,
                bytes: 100_000,
                mutations: 1_000,
            },
            deadline,
        ))
    }

    fn empty_maps() -> Maps {
        Maps {
            entries: Vec::new(),
            mutation_values: None,
            diff_gold: None,
            diff_more: None,
            lookup_values: Vec::new(),
            forced: None,
            max_scan_limit: None,
            scans: AtomicUsize::new(0),
            diff_pages: AtomicUsize::new(0),
            lookups: AtomicUsize::new(0),
        }
    }

    fn source(root: MapRoot) -> SourceCut {
        SourceCut {
            owners: root,
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        }
    }

    fn fixture(count: u8) -> Result<(Maps, Objects, SourceCut), IndexError> {
        let mut maps = empty_maps();
        let mut objects = Objects(BTreeMap::new());
        for ordinal in 0..count {
            let owner = OwnerKey {
                producer: ProducerId([1; 16]),
                stable_id: StableOwnerId([ordinal; 16]),
            };
            let reference = ObjectRef(Digest([ordinal; 32]));
            let body = ObjectRef(Digest([200; 32]));
            let record = SourceRecord {
                path: format!("{ordinal}.rs"),
                bytes: body,
                byte_len: 0,
                text: quanta_index_contract::index::SourceTextAdmission::Excluded,
            };
            let payload = serde_json::to_vec(&record)
                .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
            let previous = objects.0.insert(
                reference,
                StoredObject::new(
                    RegisteredObject::unscoped(
                        reference,
                        u64::try_from(payload.len())
                            .map_err(|_error| IndexError::ResourceLimited)?,
                        vec![body],
                    ),
                    payload,
                    None,
                ),
            );
            check!(previous.is_none());
            maps.entries
                .push((owner_map_key(owner).to_vec(), reference));
        }
        let root = if count == 0 {
            MapRoot(None)
        } else {
            MapRoot(Some(ObjectRef(Digest([255; 32]))))
        };
        Ok((maps, objects, source(root)))
    }

    fn patch_fixture(
        count: u8,
        noop: Option<u8>,
    ) -> Result<(Maps, Objects, SourceDelta, Vec<OwnerMutation>), IndexError> {
        let mut maps = empty_maps();
        let mut objects = Objects(BTreeMap::new());
        let prior = source(MapRoot(Some(ObjectRef(Digest([230; 32])))));
        let next = source(MapRoot(Some(ObjectRef(Digest([231; 32])))));
        let body = ObjectRef(Digest([240; 32]));
        let mut digest = OwnerMutationDigest::default();
        let mut mutations = Vec::new();
        let mut values = Vec::new();
        let mut diff = Vec::new();
        for ordinal in 0..count {
            let owner = OwnerKey {
                producer: ProducerId([1; 16]),
                stable_id: StableOwnerId([ordinal; 16]),
            };
            let old = ObjectRef(Digest(
                [ordinal.checked_add(1).ok_or(IndexError::ResourceLimited)?; 32],
            ));
            let new = if noop == Some(ordinal) {
                old
            } else {
                ObjectRef(Digest(
                    [ordinal
                        .checked_add(100)
                        .ok_or(IndexError::ResourceLimited)?; 32],
                ))
            };
            for reference in [old, new] {
                if objects.0.contains_key(&reference) {
                    continue;
                }
                let record = SourceRecord {
                    path: format!("{ordinal}.rs"),
                    bytes: body,
                    byte_len: 0,
                    text: quanta_index_contract::index::SourceTextAdmission::Excluded,
                };
                let payload = serde_json::to_vec(&record)
                    .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
                let _previous = objects.0.insert(
                    reference,
                    StoredObject::new(
                        RegisteredObject::unscoped(
                            reference,
                            u64::try_from(payload.len())
                                .map_err(|_error| IndexError::ResourceLimited)?,
                            vec![body],
                        ),
                        payload,
                        None,
                    ),
                );
            }
            let mutation = OwnerMutation {
                owner,
                expected: Some(old),
                next: Some(new),
            };
            digest.push(&mutation)?;
            let key = owner_map_key(owner).to_vec();
            values.push((key.clone(), owner_mutation_value(&mutation)?));
            if old == new {
                maps.lookup_values
                    .push((prior.owners, key.clone(), Some(old)));
                maps.lookup_values.push((next.owners, key, Some(new)));
            } else {
                diff.push((key, Some(old.into()), Some(new.into())));
            }
            mutations.push(mutation);
        }
        maps.mutation_values = Some(values);
        maps.diff_gold = Some(diff);
        Ok((
            maps,
            objects,
            SourceDelta {
                prior,
                next,
                changed: OwnerMutationSet {
                    root: MapRoot(Some(ObjectRef(Digest([232; 32])))),
                    count: u64::from(count),
                    digest: digest.finish(),
                },
            },
            mutations,
        ))
    }

    fn ample_budget() -> Result<WorkBudget, IndexError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?;
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 10_000,
                writes: 0,
                bytes: 10_000_000,
                mutations: 10_000,
            },
            deadline,
        ))
    }

    struct RecordMemory {
        capacity: u64,
        live: AtomicU64,
        invalid: AtomicBool,
    }

    impl MemoryLeaseOwner for RecordMemory {
        fn release(&self, bytes: u64) {
            if self
                .live
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                    live.checked_sub(bytes)
                })
                .is_err()
            {
                self.invalid.store(true, Ordering::SeqCst);
            }
        }
    }

    struct RecordScope(Arc<RecordMemory>);

    impl MemoryScopePort for RecordScope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if lifetime != MemoryLifetime::Retained {
                return Err(IndexError::BackendViolation(
                    "escaping source record requires retained custody".into(),
                ));
            }
            let _previous = self
                .0
                .live
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                    live.checked_add(bytes)
                        .filter(|next| *next <= self.0.capacity)
                })
                .map_err(|_live| IndexError::ResourceLimited)?;
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    fn record_budget(capacity: u64) -> Result<(WorkBudget, Arc<RecordMemory>), IndexError> {
        let memory = Arc::new(RecordMemory {
            capacity,
            live: AtomicU64::new(0),
            invalid: AtomicBool::new(false),
        });
        let mut work = budget()?;
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(RecordScope(
            memory.clone(),
        )))));
        Ok((work, memory))
    }

    fn record_decode_bytes(objects: &Objects, reference: ObjectRef) -> Result<u64, IndexError> {
        let stored = objects
            .0
            .get(&reference)
            .ok_or_else(|| IndexError::BackendViolation("missing source fixture".into()))?;
        u64::try_from(stored.payload.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_mul(8)
            .ok_or(IndexError::ResourceLimited)
    }

    #[test]
    fn snapshot_record_custody_survives_cursor_and_budget_drop() -> Result<(), IndexError> {
        let (maps, objects, cut) = fixture(1)?;
        let reference = maps
            .entries
            .first()
            .ok_or_else(|| IndexError::BackendViolation("missing fixture entry".into()))?
            .1;
        let bytes = record_decode_bytes(&objects, reference)?;
        let (mut work, memory) = record_budget(bytes)?;
        let change = {
            let mut cursor = SourceSnapshotCursor::new(cut, &maps, &objects);
            cursor
                .next_change(&mut work)?
                .ok_or_else(|| IndexError::BackendViolation("missing source change".into()))?
        };
        drop(work);
        let (_, record) = change
            .next
            .as_ref()
            .ok_or_else(|| IndexError::BackendViolation("missing source record".into()))?;
        check_eq!(record.path.as_str(), "0.rs");
        check_eq!(memory.live.load(Ordering::SeqCst), bytes);
        drop(change);
        check_eq!(memory.live.load(Ordering::SeqCst), 0);
        check!(!memory.invalid.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn simultaneous_source_records_refuse_without_refunding_live_record() -> Result<(), IndexError>
    {
        let (maps, objects, _cut) = fixture(2)?;
        let first = maps
            .entries
            .first()
            .ok_or_else(|| IndexError::BackendViolation("missing first fixture entry".into()))?
            .1;
        let second = maps
            .entries
            .get(1)
            .ok_or_else(|| IndexError::BackendViolation("missing second fixture entry".into()))?
            .1;
        let first_bytes = record_decode_bytes(&objects, first)?;
        let capacity = first_bytes
            .checked_add(record_decode_bytes(&objects, second)?)
            .and_then(|total| total.checked_sub(1))
            .ok_or(IndexError::ResourceLimited)?;
        let (mut work, memory) = record_budget(capacity)?;
        let retained = read_record(first, &objects, &mut work)?;
        check!(matches!(
            read_record(second, &objects, &mut work),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(retained.path.as_str(), "0.rs");
        check_eq!(memory.live.load(Ordering::SeqCst), first_bytes);
        drop(retained);
        check_eq!(memory.live.load(Ordering::SeqCst), 0);
        check!(!memory.invalid.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn snapshot_empty_and_129_owners_finish_without_repeated_scan() -> Result<(), IndexError> {
        let (empty, objects, cut) = fixture(0)?;
        let mut cursor = SourceSnapshotCursor::new(cut, &empty, &objects);
        check!(cursor.next_change(&mut budget()?)?.is_none());
        check!(cursor.next_change(&mut budget()?)?.is_none());
        check_eq!(empty.scans.load(Ordering::Relaxed), 1);

        let (maps, objects, cut) = fixture(129)?;
        let mut cursor = SourceInputCursor::new(cut, DeltaInput::Bootstrap, &maps, &objects)?;
        let mut observed = 0u64;
        while let Some(change) = cursor.next_change(&mut budget()?)? {
            check!(change.prior.is_none());
            check!(change.next.is_some());
            observed = observed.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        check_eq!(observed, 129);
        check_eq!(maps.scans.load(Ordering::Relaxed), 2);
        check!(cursor.next_change(&mut budget()?)?.is_none());
        check_eq!(maps.scans.load(Ordering::Relaxed), 2);
        Ok(())
    }

    #[test]
    fn snapshot_rejects_invalid_pages_and_source_records() -> Result<(), IndexError> {
        let (mut maps, mut objects, cut) = fixture(1)?;
        let valid = maps
            .entries
            .first()
            .ok_or_else(|| IndexError::BackendViolation("missing source fixture owner".into()))?
            .clone();
        for page in [
            MapPage::new(vec![], true, Vec::new(), None),
            MapPage::new(
                vec![
                    (valid.0.clone(), valid.1.into()),
                    (valid.0.clone(), valid.1.into()),
                ],
                false,
                Vec::new(),
                None,
            ),
            MapPage::new(
                vec![(valid.0.clone(), valid.1.into()); 129],
                false,
                Vec::new(),
                None,
            ),
            MapPage::new(
                vec![(b"bad".to_vec(), valid.1.into())],
                false,
                Vec::new(),
                None,
            ),
        ] {
            maps.forced = Some(page);
            let mut cursor = SourceSnapshotCursor::new(cut, &maps, &objects);
            check!(cursor.next_change(&mut budget()?).is_err());
        }
        maps.forced = None;
        let stored = objects
            .0
            .get_mut(&valid.1)
            .ok_or_else(|| IndexError::CorruptObject("missing fixture record".into()))?;
        let mut registration = stored.registration.clone();
        registration =
            RegisteredObject::unscoped(registration.reference, registration.bytes, Vec::new());
        *stored = StoredObject::new(registration, stored.payload.clone(), None);
        let mut cursor = SourceSnapshotCursor::new(cut, &maps, &objects);
        let first = cursor
            .next_change(&mut budget()?)
            .expect_err("invalid record edges");
        check!(matches!(first, IndexError::CorruptObject(_)));
        check_eq!(cursor.next_change(&mut budget()?), Err(first));
        Ok(())
    }

    #[test]
    fn snapshot_and_input_cursor_latch_budget_failure() -> Result<(), IndexError> {
        let (maps, objects, cut) = fixture(1)?;
        let mut denied = budget()?;
        denied.charge(WorkUsage {
            reads: 1_000,
            ..WorkUsage::default()
        })?;
        let mut snapshot = SourceSnapshotCursor::new(cut, &maps, &objects);
        let first = snapshot
            .next_change(&mut denied)
            .expect_err("budget denial");
        check_eq!(first, IndexError::ResourceLimited);
        check_eq!(snapshot.next_change(&mut budget()?), Err(first));

        let mut input = SourceInputCursor::new(cut, DeltaInput::Bootstrap, &maps, &objects)?;
        let second = input
            .next_change(&mut denied)
            .expect_err("input budget denial");
        check_eq!(second, IndexError::ResourceLimited);
        check_eq!(input.next_change(&mut budget()?), Err(second));

        let delta = SourceDelta {
            prior: source(MapRoot(None)),
            next: cut,
            changed: OwnerMutationSet {
                root: cut.owners,
                count: 1,
                digest: Digest([9; 32]),
            },
        };
        let mut denied_mutations = budget()?;
        denied_mutations.charge(WorkUsage {
            mutations: 1_000,
            ..WorkUsage::default()
        })?;
        let mut patch = SourceInputCursor::new(cut, DeltaInput::Patch(delta), &maps, &objects)?;
        let third = patch
            .next_change(&mut denied_mutations)
            .expect_err("patch budget denial");
        check_eq!(third, IndexError::ResourceLimited);
        check_eq!(patch.next_change(&mut budget()?), Err(third));
        Ok(())
    }

    #[test]
    fn input_cursor_rejects_patch_for_different_source() -> Result<(), IndexError> {
        let (maps, objects, cut) = fixture(1)?;
        let different = source(MapRoot(None));
        let changed = OwnerMutationSet {
            root: MapRoot(None),
            count: 0,
            digest: Digest([0; 32]),
        };
        check!(matches!(
            SourceInputCursor::new(
                cut,
                DeltaInput::Patch(SourceDelta {
                    prior: cut,
                    next: different,
                    changed,
                }),
                &maps,
                &objects,
            ),
            Err(IndexError::Conflict)
        ));
        Ok(())
    }

    #[test]
    fn patch_diff_merges_changed_and_noop_across_bounded_pages() -> Result<(), IndexError> {
        let (maps, objects, delta, expected) = patch_fixture(67, Some(33))?;
        let mut cursor =
            SourceChangeCursor::new(delta.changed, delta.prior, delta.next, &maps, &objects);
        let mut work = ample_budget()?;
        for mutation in &expected {
            let change = cursor.next_change(&mut work)?.ok_or_else(|| {
                IndexError::BackendViolation("patch diff ended before manifest".into())
            })?;
            check_eq!(change.owner, mutation.owner);
            check_eq!(
                change.prior.as_ref().map(|(reference, _)| *reference),
                mutation.expected
            );
            check_eq!(
                change.next.as_ref().map(|(reference, _)| *reference),
                mutation.next
            );
        }
        check!(cursor.next_change(&mut work)?.is_none());
        check!(cursor.next_change(&mut work)?.is_none());
        check_eq!(maps.diff_pages.load(Ordering::Relaxed), 2);
        check_eq!(maps.lookups.load(Ordering::Relaxed), 2);
        check_eq!(maps.scans.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn checked_patch_identities_do_not_load_source_records() -> Result<(), IndexError> {
        let (maps, _records, delta, expected) = patch_fixture(67, Some(33))?;
        let objects = Objects(BTreeMap::new());
        let mut cursor =
            SourceChangeCursor::new(delta.changed, delta.prior, delta.next, &maps, &objects);
        let mut work = ample_budget()?;
        for mutation in expected {
            check_eq!(cursor.next_mutation(&mut work)?, Some(mutation));
        }
        check!(cursor.next_mutation(&mut work)?.is_none());
        check!(cursor.next_change(&mut work)?.is_none());
        check_eq!(maps.diff_pages.load(Ordering::Relaxed), 2);
        check_eq!(maps.lookups.load(Ordering::Relaxed), 2);
        check_eq!(maps.scans.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[test]
    fn patch_diff_refuses_missing_mismatched_extra_and_malformed_rows() -> Result<(), IndexError> {
        for case in 0..8 {
            let (mut maps, objects, delta, _) = patch_fixture(2, None)?;
            let gold = maps
                .diff_gold
                .as_mut()
                .ok_or_else(|| IndexError::BackendViolation("patch fixture diff missing".into()))?;
            match case {
                0 => {
                    let _removed = gold.remove(0);
                }
                1 => {
                    gold.get_mut(0)
                        .ok_or_else(|| IndexError::BackendViolation("missing diff row".into()))?
                        .2 = Some(ObjectRef(Digest([199; 32])).into());
                }
                2 => gold.push((
                    owner_map_key(OwnerKey {
                        producer: ProducerId([1; 16]),
                        stable_id: StableOwnerId([3; 16]),
                    })
                    .to_vec(),
                    None,
                    Some(ObjectRef(Digest([198; 32])).into()),
                )),
                3 => gold.swap(0, 1),
                4 => {
                    let duplicate = gold
                        .first()
                        .cloned()
                        .ok_or_else(|| IndexError::BackendViolation("missing diff row".into()))?;
                    *gold.get_mut(1).ok_or_else(|| {
                        IndexError::BackendViolation("missing second diff row".into())
                    })? = duplicate;
                }
                5 => {
                    gold.get_mut(0)
                        .ok_or_else(|| IndexError::BackendViolation("missing diff row".into()))?
                        .2 = Some(MapValue::inline(b"not-an-object", [])?);
                }
                6 => {
                    gold.clear();
                    maps.diff_more = Some(true);
                }
                _ => gold.insert(
                    0,
                    (
                        b"alien-key".to_vec(),
                        None,
                        Some(ObjectRef(Digest([197; 32])).into()),
                    ),
                ),
            }
            for identities in [false, true] {
                let mut cursor = SourceChangeCursor::new(
                    delta.changed,
                    delta.prior,
                    delta.next,
                    &maps,
                    &objects,
                );
                let mut work = ample_budget()?;
                let first_error = loop {
                    let result = if identities {
                        cursor.next_mutation(&mut work).map(|row| row.map(|_| ()))
                    } else {
                        cursor.next_change(&mut work).map(|row| row.map(|_| ()))
                    };
                    match result {
                        Ok(Some(())) => {}
                        Ok(None) => {
                            return Err(IndexError::BackendViolation(
                                "malformed patch diff sealed a successful prefix".into(),
                            ));
                        }
                        Err(error) => break error,
                    }
                };
                check_eq!(cursor.next_mutation(&mut work), Err(first_error.clone()));
                check_eq!(cursor.next_change(&mut work), Err(first_error));
            }
        }
        Ok(())
    }

    #[test]
    fn sealed_small_mutation_stream_respects_one_row_reader_and_rejects_hidden_suffix()
    -> Result<(), IndexError> {
        let (mut maps, objects, delta, mutations) = patch_fixture(1, None)?;
        maps.max_scan_limit = Some(1);
        let mut cursor = OwnerMutationCursor::new(delta.changed, &maps, &objects);
        let mut work = ample_budget()?;
        check_eq!(cursor.next_mutation(&mut work)?, mutations.first().cloned());
        check!(cursor.next_mutation(&mut work)?.is_none());
        check_eq!(cursor.sealed_set(), Some(delta.changed));

        // A row-count declaration cannot authorize sealing a valid prefix.
        for declared in [0, 1] {
            let (mut maps, objects, mut delta, mutations) = patch_fixture(2, None)?;
            maps.max_scan_limit = Some(1);
            let mut digest = OwnerMutationDigest::default();
            if declared == 1 {
                digest.push(mutations.first().ok_or_else(|| {
                    IndexError::BackendViolation("missing first gold mutation".into())
                })?)?;
            }
            delta.changed.count = declared;
            delta.changed.digest = digest.finish();
            let mut cursor = OwnerMutationCursor::new(delta.changed, &maps, &objects);
            let mut work = ample_budget()?;
            if declared == 1 {
                check_eq!(cursor.next_mutation(&mut work)?, mutations.first().cloned());
            }
            let error = match cursor.next_mutation(&mut work) {
                Err(error @ IndexError::CorruptObject(_)) => error,
                other => {
                    return Err(IndexError::BackendViolation(format!(
                        "understated mutation count accepted hidden rows: {other:?}"
                    )));
                }
            };
            check_eq!(cursor.next_mutation(&mut work), Err(error));
            check!(cursor.sealed_set().is_none());
        }
        Ok(())
    }

    #[test]
    fn patch_diff_checks_manifest_eof_and_cancelled_cached_row() -> Result<(), IndexError> {
        let (maps, objects, mut delta, _) = patch_fixture(2, None)?;
        delta.changed.digest = Digest([9; 32]);
        let mut cursor =
            SourceChangeCursor::new(delta.changed, delta.prior, delta.next, &maps, &objects);
        let mut work = ample_budget()?;
        check!(cursor.next_change(&mut work)?.is_some());
        check!(cursor.next_change(&mut work)?.is_some());
        let error = cursor
            .next_change(&mut work)
            .expect_err("bad manifest digest");
        check!(matches!(error, IndexError::CorruptObject(_)));
        check_eq!(cursor.next_change(&mut work), Err(error));

        let (maps, objects, mut delta, _) = patch_fixture(2, None)?;
        delta.changed.digest = Digest([9; 32]);
        let mut cursor =
            SourceChangeCursor::new(delta.changed, delta.prior, delta.next, &maps, &objects);
        let mut work = ample_budget()?;
        check!(cursor.next_mutation(&mut work)?.is_some());
        check!(cursor.next_mutation(&mut work)?.is_some());
        let error = cursor
            .next_mutation(&mut work)
            .expect_err("identity cursor must finish the manifest digest");
        check!(matches!(error, IndexError::CorruptObject(_)));
        check_eq!(cursor.next_change(&mut ample_budget()?), Err(error));

        let (maps, objects, delta, _) = patch_fixture(2, None)?;
        let mut cursor =
            SourceChangeCursor::new(delta.changed, delta.prior, delta.next, &maps, &objects);
        let mut work = ample_budget()?;
        check!(cursor.next_mutation(&mut work)?.is_some());
        work.request().cancel_handle().cancel();
        check_eq!(cursor.next_change(&mut work), Err(IndexError::Cancelled));
        check_eq!(
            cursor.next_change(&mut ample_budget()?),
            Err(IndexError::Cancelled)
        );
        Ok(())
    }

    #[test]
    fn patch_diff_keeps_source_record_validation_and_refuses_unsupported_backend()
    -> Result<(), IndexError> {
        let (maps, mut objects, delta, _) = patch_fixture(1, None)?;
        let reference =
            objects.0.keys().next().copied().ok_or_else(|| {
                IndexError::BackendViolation("missing patch source record".into())
            })?;
        let stored = objects
            .0
            .get_mut(&reference)
            .ok_or_else(|| IndexError::BackendViolation("missing patch source record".into()))?;
        *stored = StoredObject::new(
            RegisteredObject::unscoped(
                stored.registration.reference,
                stored.registration.bytes,
                Vec::new(),
            ),
            stored.payload.clone(),
            None,
        );
        let mut cursor =
            SourceChangeCursor::new(delta.changed, delta.prior, delta.next, &maps, &objects);
        let error = cursor
            .next_change(&mut ample_budget()?)
            .expect_err("bad source edges");
        check!(matches!(error, IndexError::CorruptObject(_)));
        check_eq!(cursor.next_change(&mut ample_budget()?), Err(error));

        let (mut maps, objects, delta, _) = patch_fixture(1, None)?;
        maps.diff_gold = None;
        let mut cursor =
            SourceChangeCursor::new(delta.changed, delta.prior, delta.next, &maps, &objects);
        check!(matches!(
            cursor.next_change(&mut ample_budget()?),
            Err(IndexError::BackendViolation(_))
        ));
        Ok(())
    }

    #[test]
    fn patch_diff_reserves_bounded_backing_before_callbacks_and_releases_it()
    -> Result<(), IndexError> {
        let (maps, _objects, delta, _) = patch_fixture(1, None)?;
        let bytes = u64::try_from(
            64_usize
                .checked_mul(std::mem::size_of::<OwnerMutation>())
                .ok_or(IndexError::ResourceLimited)?,
        )
        .map_err(|_error| IndexError::ResourceLimited)?;
        let (mut denied, denied_memory) =
            record_budget(bytes.checked_sub(1).ok_or(IndexError::ResourceLimited)?)?;
        let mut diff = SourceDiffPages::new(delta.prior.owners, delta.next.owners, &maps);
        check_eq!(diff.peek(&mut denied), Err(IndexError::ResourceLimited));
        check_eq!(maps.diff_pages.load(Ordering::Relaxed), 0);
        drop(diff);
        check_eq!(denied_memory.live.load(Ordering::SeqCst), 0);
        check!(!denied_memory.invalid.load(Ordering::SeqCst));

        let (mut work, memory) = record_budget(bytes)?;
        let mut diff = SourceDiffPages::new(delta.prior.owners, delta.next.owners, &maps);
        check!(diff.peek(&mut work)?.is_some());
        check_eq!(memory.live.load(Ordering::SeqCst), bytes);
        drop(diff);
        check_eq!(memory.live.load(Ordering::SeqCst), 0);
        check!(!memory.invalid.load(Ordering::SeqCst));
        Ok(())
    }

    #[test]
    fn patch_diff_skips_identical_roots_but_checks_changed_root_empty_manifest()
    -> Result<(), IndexError> {
        let (mut maps, objects, mut delta, _) = patch_fixture(1, Some(0))?;
        delta.next.owners = delta.prior.owners;
        maps.diff_gold = None;
        let mut cursor =
            SourceChangeCursor::new(delta.changed, delta.prior, delta.next, &maps, &objects);
        let mut work = ample_budget()?;
        check!(cursor.next_change(&mut work)?.is_some());
        check!(cursor.next_change(&mut work)?.is_none());
        check_eq!(maps.diff_pages.load(Ordering::Relaxed), 0);
        check_eq!(maps.lookups.load(Ordering::Relaxed), 2);

        let (maps, objects, delta, _) = patch_fixture(0, None)?;
        let mut cursor =
            SourceChangeCursor::new(delta.changed, delta.prior, delta.next, &maps, &objects);
        check!(cursor.next_change(&mut ample_budget()?)?.is_none());
        check_eq!(maps.diff_pages.load(Ordering::Relaxed), 1);
        Ok(())
    }
}
