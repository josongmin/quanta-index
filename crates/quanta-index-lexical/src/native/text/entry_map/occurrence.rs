//! An authenticated occurrence block can also be one owner's complete inventory.
//! The block remains childless; its owning contribution supplies the physical edge.

use quanta_index_core::index::{
    Digest, IndexError, MapPage, MemoryLease, MemoryLifetime, ObjectRef, WorkBudget, WorkUsage,
    ports::ImmutableObjectPort,
};

use crate::native::text_occurrence::block::CheckedBlock;
use sha2::{Digest as _, Sha256};

use super::EntryMap;
use crate::native::text_spool::check_term_key;

pub(in crate::native::text) const COLD_ALIAS_MIN_TERMS: u64 = 9;
pub(in crate::native::text) const COLD_ALIAS_MAX_TERMS: u64 = 512;
pub(in crate::native::text) const COLD_INLINE_ANCHOR_KEY_BYTES: usize = super::INLINE_KEY_BYTES;

/// A producer-side commitment to one complete, ordered cold inventory.
///
/// This is a candidate only; the verifier authenticates the persisted block
/// and the ordinary entry-map path independently seals its original edit.
pub(in crate::native::text) struct ColdInventoryKeySeal {
    hash: Sha256,
}

/// Source-side commitment to the exact ordered directory descriptors. The
/// verifier derives the same transcript from independently read map rows.
pub(in crate::native::text) struct ColdDescriptorSeal {
    hash: Sha256,
}

impl ColdDescriptorSeal {
    pub(in crate::native::text) fn new() -> Self {
        let mut hash = Sha256::new();
        hash.update(b"qidx.native.text.cold-descriptors.v1\0");
        Self { hash }
    }

    pub(in crate::native::text) fn push(
        &mut self,
        fence: &[u8],
        kind: u8,
        terms: usize,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.begin(fence, kind, terms, budget)?;
        self.reference(reference);
        Ok(())
    }

    pub(in crate::native::text) fn begin(
        &mut self,
        fence: &[u8],
        kind: u8,
        terms: usize,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let len = u64::try_from(fence.len()).map_err(|_error| IndexError::ResourceLimited)?;
        let terms = u16::try_from(terms).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: len.checked_add(43).ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        self.hash.update(len.to_be_bytes());
        self.hash.update(fence);
        self.hash.update([kind]);
        self.hash.update(terms.to_be_bytes());
        Ok(())
    }

    pub(in crate::native::text) fn reference(&mut self, reference: ObjectRef) {
        self.hash.update(reference.0.0);
    }

    pub(in crate::native::text) fn finish(self) -> Digest {
        Digest(self.hash.finalize().into())
    }
}

impl ColdInventoryKeySeal {
    pub(in crate::native::text) fn new() -> Self {
        let mut hash = Sha256::new();
        hash.update(b"qidx.native.text.cold-inventory-keys.v1\0");
        Self { hash }
    }

    pub(in crate::native::text) fn push(
        &mut self,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        check_term_key(key)?;
        self.push_bounded(key, budget)
    }

    fn push_bounded(&mut self, key: &[u8], budget: &mut WorkBudget) -> Result<(), IndexError> {
        let len = u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: len.checked_add(8).ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        self.hash.update(len.to_be_bytes());
        self.hash.update(key);
        Ok(())
    }

    pub(in crate::native::text) fn finish(self) -> Digest {
        Digest(self.hash.finalize().into())
    }
}

fn open_cold_inline(
    reference: ObjectRef,
    terms: u64,
    anchor: &EntryMap,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<CheckedBlock, IndexError> {
    budget.checkpoint()?;
    if !(COLD_ALIAS_MIN_TERMS..=COLD_ALIAS_MAX_TERMS).contains(&terms) {
        return Err(IndexError::InvalidInput(
            "cold inventory alias term count is out of bounds".into(),
        ));
    }
    let EntryMap::Small(anchor) = anchor else {
        return Err(IndexError::CorruptObject(
            "cold inventory alias anchor is not inline".into(),
        ));
    };
    anchor.validate()?;
    if anchor.len() != 1 {
        return Err(IndexError::CorruptObject(
            "cold inventory alias anchor has wrong cardinality".into(),
        ));
    }
    let (anchor_key, anchor_ref) = anchor.entry(0)?.ok_or_else(|| {
        IndexError::CorruptObject("cold inventory alias anchor is missing".into())
    })?;
    if anchor_ref != reference {
        return Err(IndexError::CorruptObject(
            "cold inventory alias anchor reference differs".into(),
        ));
    }
    let stored = objects.get(reference, budget)?;
    if stored.registration.reference != reference || !stored.registration.children.is_empty() {
        return Err(IndexError::CorruptObject(
            "cold inventory alias stored identity differs".into(),
        ));
    }
    let checked = CheckedBlock::parse(stored, budget)?;
    if u64::try_from(checked.term_count()).map_err(|_error| IndexError::ResourceLimited)? != terms {
        return Err(IndexError::CorruptObject(
            "cold inventory alias term count differs".into(),
        ));
    }
    if checked.key(0)? != anchor_key {
        return Err(IndexError::CorruptObject(
            "cold inventory alias first key differs".into(),
        ));
    }
    Ok(checked)
}

/// Validate the independently sealed inline block while streaming disposable
/// diff edits. The first count retains its separate read admission; no second
/// whole-DATA admission is needed between the initial read and this visitor.
pub(in crate::native::text) fn visit_cold_inline(
    root: EntryMap,
    reference: ObjectRef,
    terms: u64,
    key_digest: Digest,
    anchor: &EntryMap,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
    mut visit: impl FnMut(&[u8], ObjectRef, u64, &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<(EntryMap, CheckedBlock), IndexError> {
    if root != EntryMap::OccurrenceBlock(reference) {
        return Err(IndexError::CorruptObject(
            "cold inline inventory root differs".into(),
        ));
    }
    let checked = open_cold_inline(reference, terms, anchor, objects, budget)?;
    let mut keys = ColdInventoryKeySeal::new();
    for slot in 0..checked.term_count() {
        budget.checkpoint()?;
        let key = checked.key(slot)?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        keys.push(key, budget)?;
        if slot == 0 {
            let retained = objects.read_retained(checked.object(), budget)?;
            budget.checkpoint()?;
            if retained != *checked.object() {
                return Err(IndexError::CorruptObject(
                    "retained occurrence count data changed".into(),
                ));
            }
        }
        let count = checked
            .position_count_if_at(slot, key, budget)?
            .ok_or_else(|| {
                IndexError::CorruptObject("cold inline inventory count key differs".into())
            })?;
        visit(key, reference, count, budget)?;
    }
    budget.checkpoint()?;
    if keys.finish() != key_digest {
        return Err(IndexError::CorruptObject(
            "cold inventory alias key seal differs".into(),
        ));
    }
    Ok((root, checked))
}

/// Walk the persisted directory and compare the independently constructed
/// source seals at EOF. A visitor may only stage disposable effects.
pub(in crate::native::text) fn visit_cold_directory(
    root: EntryMap,
    terms: u64,
    anchors: u64,
    first_reference: ObjectRef,
    key_digest: Digest,
    descriptor_digest: Digest,
    objects: &dyn ImmutableObjectPort,
    maps: &dyn quanta_index_core::index::ports::PersistentMapPort,
    budget: &mut WorkBudget,
    mut visit: impl FnMut(&[u8], ObjectRef, u64, &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<EntryMap, IndexError> {
    let EntryMap::InventoryDirectory(reference) = root else {
        return Err(IndexError::CorruptObject(
            "cold alias directory root differs".into(),
        ));
    };
    if anchors == 0
        || terms < COLD_ALIAS_MIN_TERMS
        || (anchors == 1 && terms > COLD_ALIAS_MAX_TERMS)
        || (anchors > 1 && terms <= COLD_ALIAS_MAX_TERMS)
    {
        return Err(IndexError::InvalidInput(
            "cold alias directory envelope differs".into(),
        ));
    }
    let mut keys = ColdInventoryKeySeal::new();
    let mut descriptors = ColdDescriptorSeal::new();
    let mut actual_terms = 0_u64;
    let mut actual_anchors = 0_u64;
    super::directory::visit_descriptors(objects, maps, reference, budget, |rows, budget| {
        if actual_anchors == 0 && rows.reference != first_reference {
            return Err(IndexError::CorruptObject(
                "cold alias first child differs".into(),
            ));
        }
        descriptors.push(rows.fence, rows.kind, rows.terms, rows.reference, budget)?;
        actual_anchors = actual_anchors
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if actual_anchors > anchors {
            return Err(IndexError::CorruptObject(
                "cold alias directory has excess descriptors".into(),
            ));
        }
        for slot in 0..rows.terms {
            budget.checkpoint()?;
            let key = rows.key(slot)?;
            keys.push(key, budget)?;
            actual_terms = actual_terms
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if actual_terms > terms {
                return Err(IndexError::CorruptObject(
                    "cold alias directory has excess terms".into(),
                ));
            }
            let count = rows.position_count(slot, key, budget)?;
            visit(key, rows.reference, count, budget)?;
        }
        Ok(())
    })?;
    if actual_terms != terms
        || actual_anchors != anchors
        || keys.finish() != key_digest
        || descriptors.finish() != descriptor_digest
    {
        return Err(IndexError::CorruptObject(
            "cold alias persisted directory differs from source".into(),
        ));
    }
    Ok(root)
}

fn corrupt(reason: &str) -> IndexError {
    IndexError::CorruptObject(reason.into())
}

fn read(
    objects: &dyn ImmutableObjectPort,
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<CheckedBlock, IndexError> {
    budget.checkpoint()?;
    let object = objects.get(reference, budget)?;
    if object.registration.reference != reference || !object.registration.children.is_empty() {
        return Err(corrupt("occurrence inventory identity or edges differ"));
    }
    CheckedBlock::parse(object, budget)
}

fn page(
    checked: &CheckedBlock,
    reference: ObjectRef,
    ordinal: usize,
    limit: usize,
    budget: &mut WorkBudget,
) -> Result<MapPage, IndexError> {
    budget.checkpoint()?;
    let count = checked.term_count();
    let remaining = count.checked_sub(ordinal).ok_or_else(|| {
        IndexError::BackendViolation("occurrence inventory cursor passed final row".into())
    })?;
    let capacity = remaining.min(limit);
    let allocation = budget.reserve_memory(
        MapPage::allocation_bytes(capacity)?,
        MemoryLifetime::Retained,
    )?;
    let mut leases: Vec<MemoryLease> = Vec::new();
    leases
        .try_reserve_exact(capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    for slot in ordinal
        ..ordinal
            .checked_add(capacity)
            .ok_or(IndexError::ResourceLimited)?
    {
        budget.checkpoint()?;
        let key = checked.key(slot)?;
        let bytes = u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        let memory = if bytes == 0 {
            None
        } else {
            budget.reserve_memory(bytes, MemoryLifetime::Retained)?
        };
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(key.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        owned.extend_from_slice(key);
        entries.push((owned, reference.into()));
        leases.extend(memory);
    }
    let end = ordinal
        .checked_add(capacity)
        .ok_or(IndexError::ResourceLimited)?;
    Ok(MapPage::new(entries, end < count, leases, allocation))
}

pub(super) fn lookup(
    objects: &dyn ImmutableObjectPort,
    reference: ObjectRef,
    key: &[u8],
    budget: &mut WorkBudget,
) -> Result<Option<ObjectRef>, IndexError> {
    budget.checkpoint()?;
    budget.charge(WorkUsage {
        bytes: u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let checked = read(objects, reference, budget)?;
    let ordinal = checked.lower_bound(key, budget)?;
    Ok((ordinal < checked.term_count() && checked.key(ordinal)? == key).then_some(reference))
}

pub(super) fn scan(
    objects: &dyn ImmutableObjectPort,
    reference: ObjectRef,
    after: Option<&[u8]>,
    limit: u32,
    budget: &mut WorkBudget,
) -> Result<MapPage, IndexError> {
    budget.checkpoint()?;
    if limit == 0 || limit > 4096 {
        return Err(IndexError::InvalidInput(
            "entry scan limit exceeds bound".into(),
        ));
    }
    let checked = read(objects, reference, budget)?;
    let ordinal = if let Some(after) = after {
        let lower = checked.lower_bound(after, budget)?;
        if lower < checked.term_count() && checked.key(lower)? == after {
            lower.checked_add(1).ok_or(IndexError::ResourceLimited)?
        } else {
            lower
        }
    } else {
        0
    };
    page(
        &checked,
        reference,
        ordinal,
        usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?,
        budget,
    )
}

/// One validated block is retained across bounded inventory pages. A failed
/// retained read, charge or allocation never advances the cursor.
#[derive(Clone, Copy, Eq, PartialEq)]
enum InventoryReadMode {
    Unset,
    Page,
    Row,
}

pub(in crate::native::text) struct PackedInventoryCursor<'a> {
    objects: &'a dyn ImmutableObjectPort,
    reference: ObjectRef,
    checked: CheckedBlock,
    ordinal: usize,
    mode: InventoryReadMode,
    next_count_ordinal: usize,
    count_verified: bool,
}

impl<'a> PackedInventoryCursor<'a> {
    pub(in crate::native::text) fn open(
        objects: &'a dyn ImmutableObjectPort,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        let checked = read(objects, reference, budget)?;
        Ok(Self {
            objects,
            reference,
            checked,
            ordinal: 0,
            mode: InventoryReadMode::Unset,
            next_count_ordinal: 0,
            count_verified: false,
        })
    }

    pub(in crate::native::text) fn reference(&self) -> ObjectRef {
        self.reference
    }

    pub(in crate::native::text) fn count_verified(&self) -> bool {
        self.count_verified
    }

    pub(in crate::native::text) fn into_validated_block(self) -> Option<CheckedBlock> {
        self.count_verified.then_some(self.checked)
    }

    /// Visit one row in the single authenticated block held for this diff.
    /// Its allocation stays admitted for the entire cursor; each row still
    /// charges key work. Refusals leave the ordinal unchanged. Page readers
    /// retain their separate admission policy. The two modes cannot mix.
    pub(in crate::native::text) fn visit_next<T>(
        &mut self,
        budget: &mut WorkBudget,
        visit: impl FnOnce(&[u8], ObjectRef, &mut WorkBudget) -> Result<T, IndexError>,
    ) -> Result<Option<T>, IndexError> {
        budget.checkpoint()?;
        if self.mode == InventoryReadMode::Page {
            return Err(IndexError::BackendViolation(
                "occurrence inventory cursor read modes differ".into(),
            ));
        }
        self.mode = InventoryReadMode::Row;
        if self.ordinal == self.checked.term_count() {
            return Ok(None);
        }
        let key = self.checked.key(self.ordinal)?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let value = visit(key, self.reference, budget)?;
        self.ordinal = self
            .ordinal
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(Some(value))
    }

    /// Reuse this Diff cursor's authenticated DATA for an exact block member.
    /// The first count still crosses the adapter's logical read and refusal
    /// boundary; later counts share only this cursor's bounded lifetime.
    pub(in crate::native::text) fn validated_count(
        &mut self,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<u64, IndexError> {
        budget.checkpoint()?;
        if !self.count_verified {
            let original = self.checked.object();
            let retained = self.objects.read_retained(original, budget)?;
            budget.checkpoint()?;
            if retained != *original {
                return Err(corrupt("retained occurrence count data changed"));
            }
        }
        let (count, ordinal) =
            match self
                .checked
                .position_count_if_at(self.next_count_ordinal, key, budget)?
            {
                Some(count) => (count, self.next_count_ordinal),
                None => self.checked.position_count_with_ordinal(key, budget)?,
            };
        self.next_count_ordinal = ordinal.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        self.count_verified = true;
        Ok(count)
    }

    pub(in crate::native::text) fn next_page(
        &mut self,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        budget.checkpoint()?;
        if limit == 0 || limit > 64 {
            return Err(IndexError::InvalidInput(
                "occurrence inventory cursor page exceeds bound".into(),
            ));
        }
        if self.mode == InventoryReadMode::Row {
            return Err(IndexError::BackendViolation(
                "occurrence inventory cursor read modes differ".into(),
            ));
        }
        self.mode = InventoryReadMode::Page;
        if self.ordinal != 0 {
            let original = self.checked.object();
            let retained = self.objects.read_retained(original, budget)?;
            if !std::ptr::eq(&raw const *retained, &raw const **original) && retained != *original {
                return Err(corrupt("retained occurrence inventory data changed"));
            }
        }
        let result = page(
            &self.checked,
            self.reference,
            self.ordinal,
            usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?,
            budget,
        )?;
        self.ordinal = self
            .ordinal
            .checked_add(result.entries.len())
            .ok_or(IndexError::ResourceLimited)?;
        Ok(result)
    }
}

#[cfg(test)]
mod cursor_tests {
    use std::{
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        time::{Duration, Instant},
    };

    use quanta_index_core::index::{
        Digest, ObjectBatchInput, RegisteredObject, RegisteredObjectBatch, StoredObject,
        WorkLimits, ports::ObjectWriteSink,
    };

    use super::*;

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(IndexError::BackendViolation(format!(
                    "test assertion failed at {}:{}: {}",
                    file!(),
                    line!(),
                    stringify!($condition)
                )));
            }
        }};
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) if *left == *right => {}
                (left, right) => {
                    return Err(IndexError::BackendViolation(format!(
                        "test equality failed at {}:{}: {left:?} != {right:?}",
                        file!(),
                        line!()
                    )));
                }
            }
        }};
    }

    struct Objects {
        object: StoredObject,
        retained: AtomicUsize,
        refuse: AtomicBool,
    }

    impl ImmutableObjectPort for Objects {
        fn identify(
            &self,
            _payload: &[u8],
            _children: &[ObjectRef],
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("unused test identify".into()))
        }

        fn put(
            &self,
            _payload: &[u8],
            _children: &[ObjectRef],
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("unused test put".into()))
        }

        fn install(
            &self,
            _payload: &[u8],
            _children: &[ObjectRef],
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("unused test install".into()))
        }

        fn install_batch(
            &self,
            _inputs: &[ObjectBatchInput],
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<RegisteredObjectBatch, IndexError> {
            Err(IndexError::BackendViolation("unused test batch".into()))
        }

        fn get(
            &self,
            reference: ObjectRef,
            budget: &mut WorkBudget,
        ) -> Result<StoredObject, IndexError> {
            budget.checkpoint()?;
            if reference != self.object.registration.reference {
                return Err(IndexError::CorruptObject("unknown test object".into()));
            }
            Ok(self.object.clone())
        }

        fn read_retained(
            &self,
            object: &StoredObject,
            budget: &mut WorkBudget,
        ) -> Result<StoredObject, IndexError> {
            budget.checkpoint()?;
            let _previous = self.retained.fetch_add(1, Ordering::Relaxed);
            if self.refuse.load(Ordering::Relaxed) {
                return Err(IndexError::ResourceLimited);
            }
            self.get(object.registration.reference, budget)
        }
    }

    fn objects() -> Result<Objects, IndexError> {
        // Fixed QITXOC01 wire: 65 sorted five-byte keys share one position vector.
        let mut payload = b"QITXOC01".to_vec();
        payload.extend_from_slice(&65_u16.to_be_bytes());
        payload.extend_from_slice(&1_u16.to_be_bytes());
        payload.extend_from_slice(&0_u32.to_be_bytes());
        for ordinal in 0..65_u8 {
            payload.extend_from_slice(&5_u16.to_be_bytes());
            payload.push(0);
            payload.extend_from_slice(format!("t{ordinal:03}").as_bytes());
            payload.extend_from_slice(&0_u16.to_be_bytes());
        }
        payload.extend_from_slice(&1_u16.to_be_bytes());
        payload.extend_from_slice(&0_u32.to_be_bytes());
        let reference = ObjectRef(Digest([7; 32]));
        let bytes = u64::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?;
        let registration = RegisteredObject::unscoped(reference, bytes, Vec::new());
        Ok(Objects {
            object: StoredObject::new(registration, payload, None),
            retained: AtomicUsize::new(0),
            refuse: AtomicBool::new(false),
        })
    }

    fn budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 100_000,
                writes: 100_000,
                bytes: 100_000,
                mutations: 100_000,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    #[test]
    fn rows_follow_wire_order_and_charge_exact_key_bytes() -> Result<(), IndexError> {
        let objects = objects()?;
        let mut work = budget()?;
        let mut cursor = PackedInventoryCursor::open(
            &objects,
            objects.object.registration.reference,
            &mut work,
        )?;
        for ordinal in 0..65_u8 {
            let row =
                cursor.visit_next(&mut work, |key, reference, _| Ok((key.to_vec(), reference)))?;
            check_eq!(
                row,
                Some((
                    [vec![0], format!("t{ordinal:03}").into_bytes()].concat(),
                    objects.object.registration.reference
                ))
            );
        }
        check_eq!(cursor.visit_next(&mut work, |_, _, _| Ok(()))?, None);
        check_eq!(objects.retained.load(Ordering::Relaxed), 0);
        check_eq!(work.usage().bytes, 325);
        Ok(())
    }

    #[test]
    fn row_failure_retries_same_key_at_block_continuation() -> Result<(), IndexError> {
        let objects = objects()?;
        let mut work = budget()?;
        let mut cursor = PackedInventoryCursor::open(
            &objects,
            objects.object.registration.reference,
            &mut work,
        )?;
        for _ in 0..64 {
            check!(cursor.visit_next(&mut work, |_, _, _| Ok(()))?.is_some());
        }
        let mut denied = WorkBudget::new(
            quanta_index_core::index::WorkLimits {
                reads: 0,
                writes: 0,
                bytes: 4,
                mutations: 0,
            },
            work.request().deadline(),
        );
        check_eq!(
            cursor.visit_next(&mut denied, |_, _, _| Ok(())),
            Err(IndexError::ResourceLimited)
        );
        objects.refuse.store(true, Ordering::Relaxed);
        check_eq!(
            cursor.visit_next(&mut work, |key, _, _| Ok(key.to_vec())),
            Ok(Some(b"\0t064".to_vec()))
        );
        check_eq!(objects.retained.load(Ordering::Relaxed), 0);
        check_eq!(cursor.visit_next(&mut work, |_, _, _| Ok(()))?, None);
        Ok(())
    }

    #[test]
    fn cancellation_and_mode_mixing_fail_closed() -> Result<(), IndexError> {
        let objects = objects()?;
        let mut work = budget()?;
        let mut row = PackedInventoryCursor::open(
            &objects,
            objects.object.registration.reference,
            &mut work,
        )?;
        check!(row.visit_next(&mut work, |_, _, _| Ok(()))?.is_some());
        check!(matches!(
            row.next_page(64, &mut work),
            Err(IndexError::BackendViolation(_))
        ));
        let mut page = PackedInventoryCursor::open(
            &objects,
            objects.object.registration.reference,
            &mut work,
        )?;
        check_eq!(page.next_page(1, &mut work)?.entries.len(), 1);
        check!(matches!(
            page.visit_next(&mut work, |_, _, _| Ok(())),
            Err(IndexError::BackendViolation(_))
        ));
        let mut cancelled = budget()?;
        cancelled.request().cancel_handle().cancel();
        check!(matches!(
            row.visit_next(&mut cancelled, |_, _, _| Ok(())),
            Err(IndexError::Cancelled)
        ));
        check_eq!(
            row.visit_next(&mut work, |key, _, _| Ok(key.to_vec()))?,
            Some(b"\0t001".to_vec())
        );
        Ok(())
    }
}
