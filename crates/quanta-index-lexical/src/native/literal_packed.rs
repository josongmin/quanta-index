//! Source-bound Literal publication using immutable ordered posting blocks.

use std::cmp::Ordering;

use quanta_index_core::index::{
    AttemptContext, DeltaInput, Digest, IndexError, MapPageCursor, MapRoot, MemoryLease,
    MemoryLifetime, ObjectBatchInput, ObjectChildren, ObjectRef, OrdinalMapEdit, SourceCut,
    SourceDelta, SourceInputCursor, WorkBudget,
    ports::{MapEditCursor, PersistentMapPort},
};

use super::{
    literal::{load_literal_body, owner_key},
    literal_blocks::{LiteralBlockBuilder, PreparedLiteralBlock, PushResult, load_block},
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
    _memory: Option<MemoryLease>,
}

struct Publisher {
    pending: PreparedRows<Pending>,
    edits: PreparedRows<OrdinalMapEdit>,
}

impl Publisher {
    fn new() -> Result<Self, IndexError> {
        Ok(Self {
            pending: PreparedRows::new(BLOCK_BATCH)?,
            edits: PreparedRows::new(usize::MAX / std::mem::size_of::<OrdinalMapEdit>())?,
        })
    }

    fn push(
        &mut self,
        key: Key,
        expected: Option<ObjectRef>,
        block: Option<PreparedLiteralBlock>,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        let (input, memory) = match block {
            Some(block) => {
                let (input, memory) = block.into_batch_input();
                (Some(input), memory)
            }
            None => (None, None),
        };
        self.pending.push(
            Pending {
                key,
                expected,
                input,
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
        let references = context.put_batch(&inputs)?;
        let mut references = references.iter().copied();
        for (row, present) in self.pending.as_slice().iter().zip(has_payload) {
            let next = if present {
                Some(
                    references
                        .next()
                        .ok_or_else(|| corrupt("block install omitted a reference"))?,
                )
            } else {
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

    fn range(
        &mut self,
        mut rows: impl FnMut(&mut WorkBudget) -> Result<Option<Key>, IndexError>,
        old: Option<DirectoryEntry>,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        let mut builder = LiteralBlockBuilder::new(context.budget)?;
        let mut expected = old;
        while let Some(key) = rows(context.budget)? {
            if self.push_key(&mut builder, key, &mut expected, context)? {
                let result = builder.try_push(key, context.budget)?;
                if matches!(result, PushResult::Full) {
                    return Err(corrupt("one posting exceeds a block"));
                }
            }
        }
        if !builder.is_empty() {
            self.block(builder.finish()?, &mut expected, context)?;
        }
        if let Some((key, reference)) = expected {
            self.push(key, Some(reference), None, context)?;
        }
        Ok(())
    }

    fn push_key(
        &mut self,
        builder: &mut LiteralBlockBuilder,
        key: Key,
        expected: &mut Option<DirectoryEntry>,
        context: &mut AttemptContext<'_>,
    ) -> Result<bool, IndexError> {
        if !matches!(builder.try_push(key, context.budget)?, PushResult::Full) {
            return Ok(false);
        }
        let full = std::mem::replace(builder, LiteralBlockBuilder::new(context.budget)?);
        let block = full.finish()?;
        self.block(block, expected, context)?;
        Ok(true)
    }

    fn block(
        &mut self,
        block: PreparedLiteralBlock,
        expected: &mut Option<DirectoryEntry>,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        let key = block.last_key;
        if block.count == 0 || block.first_key > key {
            return Err(corrupt("published block boundary differs"));
        }
        let prior = if expected.is_some_and(|(last, _)| last == key) {
            expected.take().map(|(_, reference)| reference)
        } else {
            None
        };
        self.push(key, prior, Some(block), context)
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
    context: &mut AttemptContext<'_>,
) -> Result<MapRoot, IndexError> {
    let mut changes = Changes::collect(input, context)?;
    if changes.peek(context.budget)?.is_none() {
        return Ok(input.prior);
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
            publisher.range(
                merge_keys(block.keys(), &mut changes, Some(last)),
                Some(old),
                context,
            )?;
        }
        lower = Some(last);
    }
    if changes.peek(context.budget)?.is_some() {
        publisher.range(
            merge_keys(std::iter::empty(), &mut changes, None),
            None,
            context,
        )?;
    }
    drop(changes);
    publisher.flush(context)?;
    let roots = context.apply_stream(
        &[input.prior],
        &mut EditCursor(publisher.edits.as_slice().iter()),
    )?;
    roots
        .into_iter()
        .next()
        .ok_or_else(|| corrupt("missing directory output"))
}

fn compare_range(
    target: &mut Directory<'_>,
    ready: &mut Option<DirectoryEntry>,
    mut expected: impl FnMut(&mut WorkBudget) -> Result<Option<Key>, IndexError>,
    lower: Option<Key>,
    upper: Option<Key>,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    let mut last_seen = lower;
    while let Some((last, reference)) = *ready {
        if upper.is_some_and(|key| last > key) {
            break;
        }
        let block = load_block(context.objects, last, reference, context.budget)?;
        let first = block.first_key()?;
        if last_seen.is_some_and(|key| key >= first) {
            return Err(corrupt("target block overlaps its predecessor"));
        }
        for key in block.keys() {
            if expected(context.budget)? != Some(key?) {
                return Err(corrupt("target posting differs from source delta"));
            }
        }
        last_seen = Some(last);
        *ready = target.next(context.budget)?;
    }
    if expected(context.budget)?.is_some() {
        return Err(corrupt("target block omits a source posting"));
    }
    Ok(())
}

/// Reconstruct the logical edit stream from authenticated source bytes, then
/// compare persisted target rows and prove untouched block references equal.
pub(super) fn seal(
    input: &SealInput,
    root: PackedRoot,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    if root.sources != input.source.owners {
        return Err(corrupt("root source differs from seal input"));
    }
    let mut changes = Changes::collect(input, context)?;
    if changes.peek(context.budget)?.is_none() && root.blocks == input.prior {
        return Ok(());
    }
    let mut prior = Directory::new(context.maps, input.prior);
    let mut target = Directory::new(context.maps, root.blocks);
    let mut ready = target.next(context.budget)?;
    let mut lower = None;
    while let Some(old @ (last, reference)) = prior.next(context.budget)? {
        if changes.affects(last, context.budget)? {
            let block = load_block(context.objects, last, reference, context.budget)?;
            let first = block.first_key()?;
            if lower.is_some_and(|key| key >= first) {
                return Err(corrupt("prior block overlaps its predecessor"));
            }
            compare_range(
                &mut target,
                &mut ready,
                merge_keys(block.keys(), &mut changes, Some(last)),
                lower,
                Some(last),
                context,
            )?;
        } else {
            if ready != Some(old) {
                return Err(corrupt("untouched block reference differs"));
            }
            ready = target.next(context.budget)?;
        }
        lower = Some(last);
    }
    compare_range(
        &mut target,
        &mut ready,
        merge_keys(std::iter::empty(), &mut changes, None),
        lower,
        None,
        context,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
