//! Source-bound Literal publication using immutable ordered posting blocks.

use sha2::{Digest as _, Sha256};
use std::cmp::Ordering;

use quanta_index_core::index::{
    AttemptContext, AttemptRef, DeltaInput, Digest, IndexError, InputBinding, MapPageCursor,
    MapRoot, MemberBinding, MemoryLease, MemoryLifetime, ObjectBatchInput, ObjectChildren,
    ObjectRef, OrdinalMapEdit, SourceCut, SourceDelta, SourceInputCursor, WorkBudget, WorkUsage,
    ports::{MapEditCursor, PersistentMapPort},
};

use super::{
    literal::{load_literal_body, observe_member_work, owner_key},
    literal_blocks::{
        ColdOwnerPrefix, LiteralBlockBuilder, PreparedLiteralBlock, PushResult, load_block,
    },
    literal_delta_runs::{DeltaCollector, DeltaStream},
    literal_memberships::{MembershipEdit, OwnerMemberships},
    text_memory::PreparedRows,
};

type Key = [u8; 35];
const MAGIC: &[u8; 8] = b"QLITBLK2";
const BLOCK_BATCH: usize = 32;
const DIRECTORY_PAGE: u32 = 64;

#[derive(Clone, Copy)]
pub(super) struct PackedRoot {
    pub(super) sources: MapRoot,
    pub(super) blocks: MapRoot,
}

fn corrupt(message: &str) -> IndexError {
    IndexError::CorruptObject(format!("literal blocks: {message}"))
}

impl PackedRoot {
    pub(super) fn decode(bytes: &[u8]) -> Result<Option<Self>, IndexError> {
        let Some(mut rest) = bytes.strip_prefix(MAGIC) else {
            return Ok(None);
        };
        let mut read_root = || {
            let (tag, remaining) = rest
                .split_first()
                .ok_or_else(|| corrupt("missing root tag"))?;
            rest = remaining;
            match tag {
                0 => Ok(MapRoot(None)),
                1 => {
                    let (digest, remaining) = rest
                        .split_first_chunk::<32>()
                        .ok_or_else(|| corrupt("truncated root digest"))?;
                    rest = remaining;
                    Ok(MapRoot(Some(ObjectRef(Digest(*digest)))))
                }
                _ => Err(corrupt("unknown root tag")),
            }
        };
        let sources = read_root()?;
        let blocks = read_root()?;
        if !rest.is_empty() || (sources.0.is_none() && blocks.0.is_some()) {
            return Err(corrupt("invalid root shape"));
        }
        Ok(Some(Self { sources, blocks }))
    }

    pub(super) fn install(
        self,
        context: &mut AttemptContext<'_>,
    ) -> Result<(ObjectRef, Digest), IndexError> {
        use sha2::{Digest as _, Sha256};
        let mut bytes = [0_u8; 74];
        let (magic, mut rest) = bytes
            .split_first_chunk_mut::<8>()
            .ok_or_else(|| corrupt("root buffer missing"))?;
        magic.copy_from_slice(MAGIC);
        for root in [self.sources, self.blocks] {
            let (tag, remaining) = rest
                .split_first_mut()
                .ok_or_else(|| corrupt("root tag buffer"))?;
            *tag = u8::from(root.0.is_some());
            rest = remaining;
            if let Some(ObjectRef(Digest(digest))) = root.0 {
                let (target, remaining) = rest
                    .split_first_chunk_mut::<32>()
                    .ok_or_else(|| corrupt("root digest buffer"))?;
                target.copy_from_slice(&digest);
                rest = remaining;
            }
        }
        let len = 74_usize
            .checked_sub(rest.len())
            .ok_or(IndexError::ResourceLimited)?;
        let payload = bytes
            .get(..len)
            .ok_or_else(|| corrupt("root length differs"))?;
        let artifact = context.put(payload, self.children().as_slice())?;
        Ok((artifact, Digest(Sha256::digest(payload).into())))
    }

    pub(super) fn children(self) -> ObjectChildren {
        match (self.sources.0, self.blocks.0) {
            (None, None) => ObjectChildren::Empty,
            (Some(source), None) => ObjectChildren::One([source]),
            (Some(source), Some(blocks)) => ObjectChildren::Two([source, blocks]),
            (None, Some(blocks)) => ObjectChildren::One([blocks]),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct SealInput {
    pub(super) source: SourceCut,
    pub(super) input: DeltaInput<SourceDelta>,
    pub(super) prior: MapRoot,
}

#[derive(Clone, Debug)]
pub(super) struct SealWitness {
    attempt: AttemptRef,
    source: SourceCut,
    input: InputBinding,
    prior: MapRoot,
    directory: Option<DirectorySeal>,
    member: Option<MemberBinding>,
}

impl SealWitness {
    pub(super) fn bind_member(&mut self, member: &MemberBinding) {
        self.member = Some(member.clone());
    }

    fn validate_identity(
        &self,
        member: &MemberBinding,
        root: PackedRoot,
        attempt: AttemptRef,
    ) -> Result<(), IndexError> {
        if attempt != self.attempt
            || member.input != self.input
            || self.member.as_ref() != Some(member)
            || root.sources != self.source.owners
            || member.input.root != self.source.owners
        {
            return Err(corrupt("literal seal attempt, binding or source differs"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct DirectorySeal {
    count: u64,
    digest: [u8; 32],
}

struct Changes {
    owners: PreparedRows<[u8; 32]>,
    rows: DeltaStream,
    ready: Option<(Key, bool)>,
}

impl Changes {
    fn collect(input: &SealInput, context: &mut AttemptContext<'_>) -> Result<Self, IndexError> {
        let mut cursor = SourceInputCursor::new(
            input.source,
            input.input.clone(),
            context.maps,
            context.objects,
        )?;
        let mut owners = PreparedRows::new(usize::MAX / 32)?;
        let mut rows = DeltaCollector::new(context.budget)?;
        let mut grams = None;
        while let Some(change) = cursor.next_change(context.budget)? {
            let owner = owner_key(change.owner);
            if owners
                .as_slice()
                .last()
                .is_some_and(|prior| *prior >= owner)
            {
                return Err(corrupt("source changes are not ordered by owner"));
            }
            let prior = change.prior.as_ref().map(|(_, record)| record);
            let next = change.next.as_ref().map(|(_, record)| record);
            if prior.map(|record| record.bytes) == next.map(|record| record.bytes) {
                continue;
            }
            let old = prior
                .map(|record| load_literal_body(record, context.objects, context.budget))
                .transpose()?;
            let new = next
                .map(|record| load_literal_body(record, context.objects, context.budget))
                .transpose()?;
            let ordinal =
                u32::try_from(owners.len()).map_err(|_error| IndexError::ResourceLimited)?;
            owners.push(owner, context.budget)?;
            let analyzer = match &mut grams {
                Some(analyzer) => analyzer,
                slot @ None => slot.insert(OwnerMemberships::new(context.budget)?),
            };
            analyzer.diff_owner(
                ordinal,
                old.as_ref().map(|object| object.payload.as_slice()),
                new.as_ref().map(|object| object.payload.as_slice()),
                context.budget,
                |chunk, budget| rows.extend(chunk, context.scratch, budget),
            )?;
        }
        drop(grams);
        let rows = rows.finish(context.scratch, context.budget)?;
        Ok(Self {
            owners,
            rows,
            ready: None,
        })
    }

    fn key(&self, row: MembershipEdit) -> Result<Key, IndexError> {
        let index = usize::try_from(row.ordinal).map_err(|_error| IndexError::ResourceLimited)?;
        let owner = self
            .owners
            .as_slice()
            .get(index)
            .ok_or_else(|| corrupt("edit owner absent"))?;
        let mut key = [0; 35];
        let (gram, tail) = key
            .split_first_chunk_mut::<3>()
            .ok_or_else(|| corrupt("edit key shape"))?;
        *gram = row.gram;
        tail.copy_from_slice(owner);
        Ok(key)
    }

    fn peek(&mut self, budget: &mut WorkBudget) -> Result<Option<(Key, bool)>, IndexError> {
        if self.ready.is_none() {
            budget.checkpoint()?;
            let Some(row) = self.rows.next(budget)? else {
                return Ok(None);
            };
            let key = self.key(row)?;
            self.ready = Some((key, row.add));
        }
        Ok(self.ready)
    }

    fn affects(&mut self, upper: Key, budget: &mut WorkBudget) -> Result<bool, IndexError> {
        Ok(self.peek(budget)?.is_some_and(|(key, _)| key <= upper))
    }
}

type DirectoryEntry = (Key, ObjectRef);

struct Directory<'a> {
    maps: &'a dyn PersistentMapPort,
    root: MapRoot,
    after: Option<Key>,
    page: MapPageCursor,
    more: bool,
}

impl<'a> Directory<'a> {
    fn new(maps: &'a dyn PersistentMapPort, root: MapRoot) -> Self {
        Self {
            maps,
            root,
            after: None,
            page: MapPageCursor::default(),
            more: root.0.is_some(),
        }
    }

    fn next(&mut self, budget: &mut WorkBudget) -> Result<Option<DirectoryEntry>, IndexError> {
        loop {
            if let Some(row) = self.page.next() {
                let (key, reference) = row.object_parts()?;
                let key: Key = key
                    .as_slice()
                    .try_into()
                    .map_err(|_error| corrupt("directory key width"))?;
                if self.after.is_some_and(|prior| prior >= key) {
                    return Err(corrupt("directory did not advance"));
                }
                self.after = Some(key);
                return Ok(Some((key, *reference)));
            }
            if !self.more {
                return Ok(None);
            }
            let page = self.maps.scan(
                self.root,
                self.after.as_ref().map(<[u8; 35]>::as_slice),
                DIRECTORY_PAGE,
                budget,
            )?;
            if page.entries.is_empty() && page.has_more {
                return Err(corrupt("empty directory continuation"));
            }
            self.more = page.has_more;
            self.page = page.cursor();
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BlockSeal {
    first: Key,
    last: Key,
    count: u16,
    digest: [u8; 32],
}

struct BlockHasher {
    hasher: Sha256,
    first: Option<Key>,
    last: Option<Key>,
    count: u16,
}

impl BlockHasher {
    fn new() -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"QIDX literal logical block keys v2");
        Self {
            hasher,
            first: None,
            last: None,
            count: 0,
        }
    }

    fn push(&mut self, key: Key, budget: &mut WorkBudget) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if self.last.is_some_and(|last| last >= key) {
            return Err(corrupt("logical block keys are not ordered"));
        }
        // This transcript is derived from logical keys, independently of the
        // block encoder and the persisted block parser. The prefix byte makes
        // the fixed-width suffix self-delimiting; the first key uses prefix 0.
        let mut prefix = 0_usize;
        if let Some(previous) = self.last.as_ref() {
            while prefix < key.len() && previous[prefix] == key[prefix] {
                prefix += 1;
            }
        }
        let suffix = key.get(prefix..).ok_or(IndexError::ResourceLimited)?;
        let prefix = u8::try_from(prefix).map_err(|_| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(
                1_usize
                    .checked_add(suffix.len())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        self.hasher.update([prefix]);
        self.hasher.update(suffix);
        if self.first.is_none() {
            self.first = Some(key);
        }
        self.last = Some(key);
        self.count = self
            .count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(())
    }

    fn finish(mut self) -> Result<BlockSeal, IndexError> {
        let first = self
            .first
            .ok_or_else(|| corrupt("empty logical block seal"))?;
        let last = self
            .last
            .ok_or_else(|| corrupt("missing logical block last key"))?;
        self.hasher.update(self.count.to_be_bytes());
        Ok(BlockSeal {
            first,
            last,
            count: self.count,
            digest: self.hasher.finalize().into(),
        })
    }
}

struct DirectoryHasher {
    hasher: Sha256,
    last: Option<Key>,
    count: u64,
}

impl DirectoryHasher {
    fn new() -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"QIDX literal directory descriptors v1");
        Self {
            hasher,
            last: None,
            count: 0,
        }
    }

    fn push(
        &mut self,
        key: Key,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if self.last.is_some_and(|last| last >= key) {
            return Err(corrupt("intended directory keys are not ordered"));
        }
        budget.charge(WorkUsage {
            bytes: u64::try_from(key.len() + reference.0.0.len())
                .map_err(|_| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        self.hasher.update(key);
        self.hasher.update(reference.0.0);
        self.last = Some(key);
        self.count = self
            .count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(())
    }

    fn finish(mut self) -> DirectorySeal {
        self.hasher.update(self.count.to_be_bytes());
        DirectorySeal {
            count: self.count,
            digest: self.hasher.finalize().into(),
        }
    }
}

fn merge_keys<'a>(
    prior: impl Iterator<Item = Result<Key, IndexError>> + 'a,
    changes: &'a mut Changes,
    upper: Option<Key>,
) -> impl FnMut(&mut WorkBudget) -> Result<Option<Key>, IndexError> + 'a {
    let mut prior = prior.peekable();
    move |budget| loop {
        let edit = changes
            .peek(budget)?
            .filter(|(key, _)| upper.is_none_or(|last| *key <= last));
        let order = match (prior.peek(), edit) {
            (Some(Err(error)), _) => return Err(error.clone()),
            (Some(Ok(base)), Some((key, _))) => base.cmp(&key),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => return Ok(None),
        };
        match order {
            Ordering::Less => return prior.next().transpose(),
            Ordering::Equal => {
                let _old = prior.next();
                changes.ready = None;
                if edit.is_some_and(|(_, add)| add) {
                    return Err(corrupt("added posting already exists"));
                }
            }
            Ordering::Greater => {
                changes.ready = None;
                return edit
                    .map(|(key, add)| {
                        if add {
                            Ok(key)
                        } else {
                            Err(corrupt("removed posting is absent"))
                        }
                    })
                    .transpose();
            }
        }
    }
}

struct Pending {
    key: Key,
    expected: Option<ObjectRef>,
    input: Option<ObjectBatchInput>,
    seal: Option<BlockSeal>,
    _memory: Option<MemoryLease>,
}

struct Publisher {
    pending: PreparedRows<Pending>,
    edits: PreparedRows<OrdinalMapEdit>,
    directory: DirectoryHasher,
}

impl Publisher {
    fn new() -> Result<Self, IndexError> {
        Ok(Self {
            pending: PreparedRows::new(BLOCK_BATCH)?,
            edits: PreparedRows::new(usize::MAX / std::mem::size_of::<OrdinalMapEdit>())?,
            directory: DirectoryHasher::new(),
        })
    }

    fn push(
        &mut self,
        key: Key,
        expected: Option<ObjectRef>,
        block: Option<(PreparedLiteralBlock, BlockSeal)>,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        let (input, memory, seal) = match block {
            Some((block, seal)) => {
                let (input, memory) = block.into_batch_input();
                (Some(input), memory, Some(seal))
            }
            None => (None, None, None),
        };
        self.pending.push(
            Pending {
                key,
                expected,
                input,
                seal,
                _memory: memory,
            },
            context.budget,
        )?;
        if self.pending.len() == BLOCK_BATCH {
            self.flush(context)?;
        }
        Ok(())
    }

    fn flush(&mut self, context: &mut AttemptContext<'_>) -> Result<(), IndexError> {
        if self.pending.len() == 0 {
            context.budget.checkpoint()?;
            return Ok(());
        }
        let bytes = BLOCK_BATCH
            .checked_mul(std::mem::size_of::<ObjectBatchInput>())
            .ok_or(IndexError::ResourceLimited)?;
        let _memory = context.budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut inputs = Vec::new();
        inputs
            .try_reserve_exact(BLOCK_BATCH)
            .map_err(|_error| IndexError::ResourceLimited)?;
        // Deletion rows have no object install and retain their order in the directory.
        let mut has_payload = [false; BLOCK_BATCH];
        for (row, present) in self.pending.as_mut_slice().iter_mut().zip(&mut has_payload) {
            if let Some(input) = row.input.take() {
                inputs.push(input);
                *present = true;
            }
        }
        // Lane 46 includes the object port's real batch preparation/install.
        let references = observe_member_work(context, 46, |context| context.put_batch(&inputs))?;
        let mut references = references.iter().copied();
        for (row, present) in self.pending.as_slice().iter().zip(has_payload) {
            let next = if present {
                let reference = references
                    .next()
                    .ok_or_else(|| corrupt("block install omitted a reference"))?;
                let expected = row.seal.ok_or_else(|| corrupt("block seal missing"))?;
                // Lane 47 re-reads and independently validates the persisted child.
                observe_member_work(context, 47, |context| {
                    let block = load_block(context.objects, row.key, reference, context.budget)?;
                    let mut observed = BlockHasher::new();
                    for key in block.keys() {
                        observed.push(key?, context.budget)?;
                    }
                    if observed.finish()? != expected {
                        return Err(corrupt("persisted block differs from source-derived keys"));
                    }
                    Ok(())
                })?;
                self.directory.push(row.key, reference, context.budget)?;
                Some(reference)
            } else {
                if row.seal.is_some() {
                    return Err(corrupt("deleted directory entry retained a block seal"));
                }
                None
            };
            let edit =
                OrdinalMapEdit::copy_object_key(0, &row.key, row.expected, next, context.budget)?;
            self.edits.push(edit, context.budget)?;
        }
        if references.next().is_some() {
            return Err(corrupt("block install returned extra references"));
        }
        drop(inputs);
        self.pending.clear();
        Ok(())
    }

    fn untouched(
        &mut self,
        entry: DirectoryEntry,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        self.flush(context)?;
        self.directory.push(entry.0, entry.1, context.budget)
    }

    fn range(
        &mut self,
        mut rows: impl FnMut(&mut WorkBudget) -> Result<Option<Key>, IndexError>,
        old: Option<DirectoryEntry>,
        shared: Option<ColdOwnerPrefix>,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        let mut builder = LiteralBlockBuilder::new(context.budget, shared)?;
        let mut seal = BlockHasher::new();
        let mut expected = old;
        while let Some(key) = rows(context.budget)? {
            if matches!(builder.try_push(key, context.budget)?, PushResult::Full) {
                let full = std::mem::replace(
                    &mut builder,
                    LiteralBlockBuilder::new(context.budget, shared)?,
                );
                self.block(full.finish()?, seal.finish()?, &mut expected, context)?;
                seal = BlockHasher::new();
                if matches!(builder.try_push(key, context.budget)?, PushResult::Full) {
                    return Err(corrupt("one posting exceeds a block"));
                }
            }
            seal.push(key, context.budget)?;
        }
        if !builder.is_empty() {
            self.block(builder.finish()?, seal.finish()?, &mut expected, context)?;
        }
        if let Some((key, reference)) = expected {
            self.push(key, Some(reference), None, context)?;
        }
        Ok(())
    }

    fn block(
        &mut self,
        block: PreparedLiteralBlock,
        seal: BlockSeal,
        expected: &mut Option<DirectoryEntry>,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        let key = block.last_key;
        if block.count == 0
            || block.first_key > key
            || block.first_key != seal.first
            || key != seal.last
            || block.count != seal.count
        {
            return Err(corrupt("published block boundary differs"));
        }
        let prior = if expected.is_some_and(|(last, _)| last == key) {
            expected.take().map(|(_, reference)| reference)
        } else {
            None
        };
        self.push(key, prior, Some((block, seal)), context)
    }
}

struct EditCursor<'a>(std::slice::Iter<'a, OrdinalMapEdit>);

impl MapEditCursor for EditCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        budget.checkpoint()?;
        Ok(self.0.next().cloned())
    }
}

pub(super) fn publish(
    input: &SealInput,
    binding: InputBinding,
    context: &mut AttemptContext<'_>,
) -> Result<(MapRoot, SealWitness), IndexError> {
    let attempt = context.attempt;
    let witness = |directory| SealWitness {
        attempt,
        source: input.source,
        input: binding,
        prior: input.prior,
        directory,
        member: None,
    };
    let mut changes = observe_member_work(context, 44, |context| Changes::collect(input, context))?;
    if changes.peek(context.budget)?.is_none() {
        return Ok((input.prior, witness(None)));
    }
    let mut publisher = Publisher::new()?;
    let mut directory = Directory::new(context.maps, input.prior);
    let mut lower = None;
    while let Some(old @ (last, reference)) = directory.next(context.budget)? {
        if changes.affects(last, context.budget)? {
            let block = load_block(context.objects, last, reference, context.budget)?;
            let first = block.first_key()?;
            if lower.is_some_and(|key| key >= first) {
                return Err(corrupt("prior block overlaps its predecessor"));
            }
            // Lane 45 covers range merge and block encoding. Nested lanes 46/47
            // separate any batch flush from that inclusive range cost.
            observe_member_work(context, 45, |context| {
                publisher.range(
                    merge_keys(block.keys(), &mut changes, Some(last)),
                    Some(old),
                    None,
                    context,
                )
            })?;
        } else {
            publisher.untouched(old, context)?;
        }
        lower = Some(last);
    }
    if changes.peek(context.budget)?.is_some() {
        // Empty prior blocks mean every emitted key comes from the sorted owner table.
        let shared = if input.prior.0.is_none() {
            ColdOwnerPrefix::from_sorted(changes.owners.as_slice())
        } else {
            None
        };
        observe_member_work(context, 45, |context| {
            publisher.range(
                merge_keys(std::iter::empty(), &mut changes, None),
                None,
                shared,
                context,
            )
        })?;
    }
    drop(changes);
    publisher.flush(context)?;
    let directory = publisher.directory.finish();
    let roots = observe_member_work(context, 48, |context| {
        context.apply_stream(
            &[input.prior],
            &mut EditCursor(publisher.edits.as_slice().iter()),
        )
    })?;
    let root = roots
        .into_iter()
        .next()
        .ok_or_else(|| corrupt("missing directory output"))?;
    context.budget.checkpoint()?;
    Ok((root, witness(Some(directory))))
}

/// Verify the persisted directory against the source-derived descriptors.
/// New child blocks were independently decoded after installation by publish.
pub(super) fn seal(
    witness: &SealWitness,
    binding: &MemberBinding,
    root: PackedRoot,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    context.budget.checkpoint()?;
    witness.validate_identity(binding, root, context.attempt)?;
    let Some(expected) = witness.directory else {
        if root.blocks != witness.prior {
            return Err(corrupt("unchanged literal directory differs"));
        }
        return context.budget.checkpoint();
    };
    let mut actual = DirectoryHasher::new();
    let mut directory = Directory::new(context.maps, root.blocks);
    while let Some((last, reference)) = directory.next(context.budget)? {
        actual.push(last, reference, context.budget)?;
    }
    let observed = actual.finish();
    if observed.count != expected.count {
        return Err(corrupt("persisted directory count differs"));
    }
    // The digest covers every ordered key and reference, including untouched rows.
    if observed.digest != expected.digest {
        return Err(corrupt("persisted directory descriptors differ"));
    }
    context.budget.checkpoint()
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::index::{
        AttemptId, CollectionId, IndexInstanceId, OperationKey, ProducerId, SchemaId, SlotId,
        StorageFormatId, StreamId, TenantId, WorkLimits, WorkspaceKey, WorktreeId,
    };
    use std::time::{Duration, Instant};

    fn budget(bytes: u64) -> Result<WorkBudget, IndexError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?;
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes,
                mutations: 100,
            },
            deadline,
        ))
    }

    fn attempt() -> AttemptRef {
        AttemptRef {
            operation: OperationKey {
                workspace: WorkspaceKey {
                    tenant: TenantId([1; 16]),
                    collection: CollectionId([2; 16]),
                    worktree: WorktreeId([3; 16]),
                    slot: SlotId([4; 16]),
                },
                epoch: 1,
                producer: ProducerId([5; 16]),
                stream: StreamId([6; 16]),
                stream_epoch: 1,
                sequence: 1,
            },
            attempt: AttemptId([7; 16]),
            fence: 1,
        }
    }

    #[test]
    fn literal_seal_witness_binds_full_member_and_exact_attempt() -> Result<(), IndexError> {
        let source = SourceCut {
            owners: MapRoot(Some(ObjectRef(Digest([8; 32])))),
            paths: MapRoot(Some(ObjectRef(Digest([9; 32])))),
            frontiers: MapRoot(None),
        };
        let input = InputBinding {
            root: source.owners,
            schema: SchemaId([10; 16]),
            policy: Digest([11; 32]),
            context: Digest([12; 32]),
        };
        let member = MemberBinding {
            instance: IndexInstanceId([13; 16]),
            format: StorageFormatId([14; 16]),
            artifact: ObjectRef(Digest([15; 32])),
            input,
            logical_commitment: Digest([16; 32]),
        };
        let root = PackedRoot {
            sources: source.owners,
            blocks: MapRoot(None),
        };
        let mut witness = SealWitness {
            attempt: attempt(),
            source,
            input,
            prior: MapRoot(None),
            directory: None,
            member: None,
        };
        witness.bind_member(&member);
        witness.validate_identity(&member, root, attempt())?;
        let mut changed = member.clone();
        changed.input.context = Digest([17; 32]);
        assert!(
            witness
                .validate_identity(&changed, root, attempt())
                .is_err()
        );
        changed = member.clone();
        changed.artifact = ObjectRef(Digest([18; 32]));
        assert!(
            witness
                .validate_identity(&changed, root, attempt())
                .is_err()
        );
        let mut stale = attempt();
        stale.fence = 2;
        assert!(witness.validate_identity(&member, root, stale).is_err());
        let wrong_root = PackedRoot {
            sources: MapRoot(None),
            blocks: MapRoot(None),
        };
        assert!(
            witness
                .validate_identity(&member, wrong_root, attempt())
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn literal_seal_hashes_distinguish_keys_and_directory_refs_with_limits()
    -> Result<(), IndexError> {
        let mut work_budget = budget(1000)?;
        let mut keys = BlockHasher::new();
        keys.push([1; 35], &mut work_budget)?;
        keys.push([2; 35], &mut work_budget)?;
        let gold = keys.finish()?;
        assert_eq!(gold.count, 2);
        let mut changed = BlockHasher::new();
        changed.push([1; 35], &mut work_budget)?;
        changed.push([3; 35], &mut work_budget)?;
        assert_ne!(gold, changed.finish()?);
        let mut unsorted = BlockHasher::new();
        unsorted.push([1; 35], &mut work_budget)?;
        assert!(unsorted.push([1; 35], &mut work_budget).is_err());

        let mut expected = DirectoryHasher::new();
        expected.push([1; 35], ObjectRef(Digest([1; 32])), &mut work_budget)?;
        let expected = expected.finish();
        let mut forged = DirectoryHasher::new();
        forged.push([1; 35], ObjectRef(Digest([2; 32])), &mut work_budget)?;
        assert_ne!(expected.digest, forged.finish().digest);
        let mut limited = budget(0)?;
        assert!(BlockHasher::new().push([1; 35], &mut limited).is_err());
        let expired_at = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .ok_or(IndexError::ResourceLimited)?;
        let mut expired = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 100,
                mutations: 100,
            },
            expired_at,
        );
        assert!(
            DirectoryHasher::new()
                .push([1; 35], ObjectRef(Digest([1; 32])), &mut expired)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn literal_seal_witness_hashes_shared_key_prefixes_once() -> Result<(), IndexError> {
        // Independent golden: SHA256(domain || (prefix:u8 || suffix)*32 || count:u16 BE).
        // The first 35-byte key has prefix 0; each following key shares 34 bytes.
        let mut work_budget = budget(10_000)?;
        let before = work_budget.usage().bytes;
        let mut keys = BlockHasher::new();
        for last in 0_u8..32 {
            let mut key = [0_u8; 35];
            key[34] = last;
            keys.push(key, &mut work_budget)?;
        }
        let seal = keys.finish()?;
        let mut last = [0_u8; 35];
        last[34] = 31;
        assert_eq!(seal.count, 32);
        assert_eq!(seal.first, [0_u8; 35]);
        assert_eq!(seal.last, last);
        assert_eq!(work_budget.usage().bytes - before, 98);
        assert!(work_budget.usage().bytes - before <= 128);
        assert_eq!(
            seal.digest,
            [
                0x4d, 0x83, 0x23, 0x5f, 0x49, 0x1d, 0xec, 0x62, 0xf5, 0x7f, 0xc5, 0xca, 0x4f, 0x58,
                0x99, 0xd8, 0x79, 0x30, 0x60, 0x60, 0xfb, 0x99, 0x67, 0x04, 0xdb, 0x8a, 0xa4, 0x1f,
                0x38, 0x20, 0x16, 0xfb,
            ]
        );
        Ok(())
    }

    #[test]
    fn packed_root_reads_fixed_wire_and_refuses_invalid_shapes() -> Result<(), IndexError> {
        let mut wire = b"QLITBLK2".to_vec();
        wire.push(1);
        wire.extend_from_slice(&[7; 32]);
        wire.push(0);
        let root = PackedRoot::decode(&wire)?.ok_or_else(|| corrupt("gold root missing"))?;
        assert_eq!(root.sources, MapRoot(Some(ObjectRef(Digest([7; 32])))));
        assert_eq!(root.blocks, MapRoot(None));
        assert!(PackedRoot::decode(br#"{"owners":null,"grams":null}"#)?.is_none());
        wire.push(0);
        assert!(matches!(
            PackedRoot::decode(&wire),
            Err(IndexError::CorruptObject(_))
        ));
        let mut orphan = b"QLITBLK2".to_vec();
        orphan.extend_from_slice(&[0, 1]);
        orphan.extend_from_slice(&[9; 32]);
        assert!(matches!(
            PackedRoot::decode(&orphan),
            Err(IndexError::CorruptObject(_))
        ));
        Ok(())
    }
}
