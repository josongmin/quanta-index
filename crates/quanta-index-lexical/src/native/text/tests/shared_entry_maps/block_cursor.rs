//! Fixed dictionary wires prove one authenticated read across 64-row pages.

use super::*;

struct RetainedObjects<'a> {
    inner: &'a CanonicalObjects,
    fund_object: bool,
    physical_gets: AtomicUsize,
    retained_reads: AtomicUsize,
    refuse_retained: AtomicBool,
    change_retained: AtomicBool,
}

impl<'a> RetainedObjects<'a> {
    fn new(inner: &'a CanonicalObjects) -> Self {
        Self {
            inner,
            fund_object: false,
            physical_gets: AtomicUsize::new(0),
            retained_reads: AtomicUsize::new(0),
            refuse_retained: AtomicBool::new(false),
            change_retained: AtomicBool::new(false),
        }
    }

    fn with_funded_object(mut self) -> Self {
        self.fund_object = true;
        self
    }
}

impl ImmutableObjectPort for RetainedObjects<'_> {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.inner.identify(payload, children, budget)
    }

    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.inner.put(payload, children, budget)
    }

    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.inner.install(payload, children, sink, budget)
    }

    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
        self.inner.install_batch(inputs, sink, budget)
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        let _previous = self.physical_gets.fetch_add(1, Ordering::Relaxed);
        let stored = self.inner.get(reference, budget)?;
        if !self.fund_object {
            return Ok(stored);
        }
        // Model the file port's retained DATA grant before duplicating its
        // payload and child backing; the lease dies after the new DATA.
        let bytes = StoredObject::allocation_bytes(
            stored.payload.len(),
            stored.registration.children.len(),
        )?;
        let memory =
            budget.reserve_memory(bytes, quanta_index_core::index::MemoryLifetime::Retained)?;
        Ok(StoredObject::new(
            stored.registration.clone(),
            stored.payload.clone(),
            memory,
        ))
    }

    fn read_retained(
        &self,
        object: &StoredObject,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        budget.checkpoint()?;
        let _previous = self.retained_reads.fetch_add(1, Ordering::Relaxed);
        if self.refuse_retained.swap(false, Ordering::Relaxed) {
            return Err(IndexError::ResourceLimited);
        }
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        budget.charge(WorkUsage {
            bytes: object.registration.bytes,
            ..WorkUsage::default()
        })?;
        let decoded = object
            .registration
            .children
            .len()
            .checked_mul(std::mem::size_of::<ObjectRef>())
            .and_then(|children| children.checked_add(object.payload.len()))
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(decoded).map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        if self.change_retained.swap(false, Ordering::Relaxed) {
            let mut payload = object.payload.clone();
            *payload.last_mut().ok_or(IndexError::ResourceLimited)? = 0;
            return Ok(StoredObject::new(
                object.registration.clone(),
                payload,
                None,
            ));
        }
        Ok(object.clone())
    }
}

struct DenyOnePageKey {
    memory: Arc<TokenMemory>,
    calls: AtomicUsize,
}

impl quanta_index_core::index::MemoryScopePort for DenyOnePageKey {
    fn reserve(
        &self,
        bytes: u64,
        _lifetime: quanta_index_core::index::MemoryLifetime,
    ) -> Result<quanta_index_core::index::MemoryLease, IndexError> {
        // Page backing and the first key succeed; the second key refuses once.
        if self.calls.fetch_add(1, Ordering::Relaxed) == 2 {
            return Err(IndexError::ResourceLimited);
        }
        token_memory_grant(&self.memory, bytes)
    }
}

fn block_wire(rows: u16) -> Result<(Vec<u8>, Vec<ObjectRef>), IndexError> {
    let mut payload = b"QITXEM02".to_vec();
    payload.extend_from_slice(&rows.to_be_bytes());
    payload.extend_from_slice(&rows.min(250).to_be_bytes());
    let mut children = Vec::new();
    for ordinal in 0..rows {
        payload.extend_from_slice(&2_u16.to_be_bytes());
        payload.extend_from_slice(&ordinal.to_be_bytes());
        let selector = ordinal
            .checked_rem(250)
            .ok_or(IndexError::ResourceLimited)?;
        payload.extend_from_slice(&selector.to_be_bytes());
        if ordinal < 250 {
            let child = ordinal.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            let child = u8::try_from(child).map_err(|_error| IndexError::ResourceLimited)?;
            children.push(reference(child));
        }
    }
    Ok((payload, children))
}

fn child_at_row(children: &[ObjectRef], ordinal: u16) -> Result<ObjectRef, IndexError> {
    children
        .get(usize::from(ordinal % 250))
        .copied()
        .ok_or(IndexError::ResourceLimited)
}

#[test]
fn positional_cursor_reads_once_across_65_and_512_rows() -> Result<(), IndexError> {
    for rows in [65_u16, 512] {
        let objects = CanonicalObjects::default();
        let maps = ModelMaps::default();
        let (wire, children) = block_wire(rows)?;
        let root = objects.insert_wire(&wire, &children, &mut entry_budget()?)?;
        let retained = RetainedObjects::new(&objects);
        let mut work = entry_budget()?;
        let before = work.usage().reads;
        let mut cursor = MapEntries::new(&retained, &maps, EntryMap::Block(root));
        for ordinal in 0..rows {
            let entry = cursor.next(&mut work)?.ok_or_else(|| {
                IndexError::BackendViolation("positional cursor ended early".into())
            })?;
            let (key, child) = entry.object_parts()?;
            check_eq!(key.as_slice(), ordinal.to_be_bytes().as_slice());
            check_eq!(*child, child_at_row(&children, ordinal)?);
        }
        check!(cursor.next(&mut work)?.is_none());
        check_eq!(retained.physical_gets.load(Ordering::Relaxed), 1);
        let pages = usize::from(rows).div_ceil(64);
        check_eq!(retained.retained_reads.load(Ordering::Relaxed), pages - 1);
        check_eq!(
            work.usage().reads.checked_sub(before),
            Some(u64::try_from(pages).map_err(|_error| IndexError::ResourceLimited)?)
        );
        check_eq!(maps.scans.load(Ordering::Relaxed), 0);
    }
    Ok(())
}

#[test]
fn positional_cursor_default_port_reads_each_continuation() -> Result<(), IndexError> {
    let objects = CanonicalObjects::default();
    let maps = ModelMaps::default();
    let (wire, children) = block_wire(65)?;
    let root = objects.insert_wire(&wire, &children, &mut entry_budget()?)?;
    let mut work = entry_budget()?;
    let before = work.usage().reads;
    let mut cursor = MapEntries::new(&objects, &maps, EntryMap::Block(root));
    for _ in 0..65 {
        check!(cursor.next(&mut work)?.is_some());
    }
    check!(cursor.next(&mut work)?.is_none());
    check_eq!(work.usage().reads.checked_sub(before), Some(2));
    Ok(())
}

#[test]
fn positional_cursor_retained_refusal_retries_same_continuation() -> Result<(), IndexError> {
    let objects = CanonicalObjects::default();
    let maps = ModelMaps::default();
    let (wire, children) = block_wire(65)?;
    let root = objects.insert_wire(&wire, &children, &mut entry_budget()?)?;
    let retained = RetainedObjects::new(&objects);
    let mut work = entry_budget()?;
    let mut cursor = MapEntries::new(&retained, &maps, EntryMap::Block(root));
    for ordinal in 0_u16..64 {
        let entry = cursor.next(&mut work)?.ok_or(IndexError::ResourceLimited)?;
        check_eq!(entry.key()?, ordinal.to_be_bytes().as_slice());
    }
    retained.refuse_retained.store(true, Ordering::Relaxed);
    check!(matches!(
        cursor.next(&mut work),
        Err(IndexError::ResourceLimited)
    ));
    let last = cursor.next(&mut work)?.ok_or(IndexError::ResourceLimited)?;
    check_eq!(last.key()?, 64_u16.to_be_bytes().as_slice());
    check!(cursor.next(&mut work)?.is_none());
    check_eq!(retained.physical_gets.load(Ordering::Relaxed), 1);
    check_eq!(retained.retained_reads.load(Ordering::Relaxed), 2);
    Ok(())
}

#[test]
fn positional_cursor_rejects_changed_retained_data_before_continuation() -> Result<(), IndexError> {
    let objects = CanonicalObjects::default();
    let maps = ModelMaps::default();
    let (wire, children) = block_wire(65)?;
    let root = objects.insert_wire(&wire, &children, &mut entry_budget()?)?;
    let retained = RetainedObjects::new(&objects);
    let mut work = entry_budget()?;
    let mut cursor = MapEntries::new(&retained, &maps, EntryMap::Block(root));
    for _ in 0..64 {
        check!(cursor.next(&mut work)?.is_some());
    }
    retained.change_retained.store(true, Ordering::Relaxed);
    check!(matches!(
        cursor.next(&mut work),
        Err(IndexError::CorruptObject(_))
    ));
    let last = cursor.next(&mut work)?.ok_or(IndexError::ResourceLimited)?;
    check_eq!(last.key()?, 64_u16.to_be_bytes().as_slice());
    Ok(())
}

#[test]
fn positional_cursor_validates_entire_wire_before_first_page() -> Result<(), IndexError> {
    let objects = CanonicalObjects::default();
    let maps = ModelMaps::default();
    let (mut wire, children) = block_wire(65)?;
    let final_key = wire
        .len()
        .checked_sub(4)
        .ok_or(IndexError::ResourceLimited)?;
    wire.get_mut(final_key..final_key + 2)
        .ok_or(IndexError::ResourceLimited)?
        .copy_from_slice(&63_u16.to_be_bytes());
    let root = objects.insert_wire(&wire, &children, &mut entry_budget()?)?;
    let retained = RetainedObjects::new(&objects);
    let mut cursor = MapEntries::new(&retained, &maps, EntryMap::Block(root));
    check!(matches!(
        cursor.next(&mut entry_budget()?),
        Err(IndexError::CorruptObject(_))
    ));
    let (wire, short_children) = block_wire(65)?;
    let root = objects.insert_wire(
        &wire,
        short_children
            .get(..64)
            .ok_or(IndexError::ResourceLimited)?,
        &mut entry_budget()?,
    )?;
    let mut cursor = MapEntries::new(&objects, &maps, EntryMap::Block(root));
    check!(matches!(
        cursor.next(&mut entry_budget()?),
        Err(IndexError::CorruptObject(_))
    ));
    let (mut wire, children) = block_wire(65)?;
    wire.push(0);
    let root = objects.insert_wire(&wire, &children, &mut entry_budget()?)?;
    let mut cursor = MapEntries::new(&objects, &maps, EntryMap::Block(root));
    check!(matches!(
        cursor.next(&mut entry_budget()?),
        Err(IndexError::CorruptObject(_))
    ));
    Ok(())
}

#[test]
fn positional_cursor_page_grant_refusal_releases_original_memory() -> Result<(), IndexError> {
    let objects = CanonicalObjects::default();
    let maps = ModelMaps::default();
    let (wire, children) = block_wire(65)?;
    let root = objects.insert_wire(&wire, &children, &mut entry_budget()?)?;
    let (mut work, memory) = funded_batch_budget(1)?;
    let mut cursor = MapEntries::new(&objects, &maps, EntryMap::Block(root));
    check!(matches!(
        cursor.next(&mut work),
        Err(IndexError::ResourceLimited)
    ));
    drop(cursor);
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn positional_cursor_mid_page_refusal_retries_from_original_row() -> Result<(), IndexError> {
    let objects = CanonicalObjects::default();
    let maps = ModelMaps::default();
    let (wire, children) = block_wire(65)?;
    let root = objects.insert_wire(&wire, &children, &mut entry_budget()?)?;
    let retained = RetainedObjects::new(&objects);
    let memory = Arc::new(TokenMemory {
        ceiling: 1_000_000,
        ..TokenMemory::default()
    });
    let mut work = entry_budget()?;
    work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
        DenyOnePageKey {
            memory: Arc::clone(&memory),
            calls: AtomicUsize::new(0),
        },
    ))));
    let before = work.usage().reads;
    let mut cursor = MapEntries::new(&retained, &maps, EntryMap::Block(root));
    check!(matches!(
        cursor.next(&mut work),
        Err(IndexError::ResourceLimited)
    ));
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    for ordinal in 0_u16..65 {
        let entry = cursor
            .next(&mut work)?
            .ok_or_else(|| IndexError::BackendViolation("retry lost a positional row".into()))?;
        let (key, child) = entry.object_parts()?;
        check_eq!(key.as_slice(), ordinal.to_be_bytes().as_slice());
        check_eq!(*child, child_at_row(&children, ordinal)?);
    }
    check!(cursor.next(&mut work)?.is_none());
    drop(cursor);
    check_eq!(retained.physical_gets.load(Ordering::Relaxed), 1);
    check_eq!(retained.retained_reads.load(Ordering::Relaxed), 1);
    check_eq!(work.usage().reads.checked_sub(before), Some(2));
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    Ok(())
}

#[test]
fn positional_cursor_retains_funded_object_until_final_page_or_early_drop() -> Result<(), IndexError>
{
    let objects = CanonicalObjects::default();
    let maps = ModelMaps::default();
    let (wire, children) = block_wire(65)?;
    let root = objects.insert_wire(&wire, &children, &mut entry_budget()?)?;
    let retained = RetainedObjects::new(&objects).with_funded_object();
    let object_bytes = StoredObject::allocation_bytes(wire.len(), children.len())?;

    let (mut work, memory) = funded_batch_budget(1_000_000)?;
    {
        let mut cursor = MapEntries::new(&retained, &maps, EntryMap::Block(root));
        let first = cursor.next(&mut work)?.ok_or(IndexError::ResourceLimited)?;
        check_eq!(first.key()?, 0_u16.to_be_bytes().as_slice());
        let live_with_object = memory.live.load(Ordering::Relaxed);
        check!(live_with_object > object_bytes);
        drop(cursor);
        let live_with_page = memory.live.load(Ordering::Relaxed);
        check_eq!(
            live_with_object.checked_sub(live_with_page),
            Some(object_bytes)
        );
        check!(live_with_page > 0);
        drop(first);
    }
    check_eq!(memory.live.load(Ordering::Relaxed), 0);

    let (mut work, memory) = funded_batch_budget(1_000_000)?;
    {
        let mut cursor = MapEntries::new(&retained, &maps, EntryMap::Block(root));
        for ordinal in 0_u16..65 {
            let entry = cursor.next(&mut work)?.ok_or(IndexError::ResourceLimited)?;
            check_eq!(entry.key()?, ordinal.to_be_bytes().as_slice());
        }
        check!(cursor.next(&mut work)?.is_none());
        drop(cursor);
    }
    check_eq!(memory.live.load(Ordering::Relaxed), 0);
    check!(!memory.invalid_release.load(Ordering::Relaxed));
    check_eq!(retained.physical_gets.load(Ordering::Relaxed), 2);
    Ok(())
}
