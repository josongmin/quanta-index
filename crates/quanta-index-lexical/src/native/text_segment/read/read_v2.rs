//! Authenticated layered text reader. A partition is the bounded unit of
//! materialization; posting pages are opened only for the queried term.

use super::super::build::funded_vec;
use super::super::wire_v2::{self, LayerRow, PartitionKind, RootRow};
use super::*;
use quanta_index_core::index::MemoryLease;
use std::mem::size_of;

struct RootV2 {
    object: StoredObject,
    source: SourceCut,
    summary: Summary,
    count: usize,
    row_offset: usize,
}

impl RootV2 {
    fn row(&self, index: usize) -> Result<(Option<[u8; 32]>, RootRow), IndexError> {
        if index >= self.count {
            return Err(wire::corrupt("V2 partition ordinal"));
        }
        let rows = self
            .object
            .payload
            .get(self.row_offset..)
            .ok_or_else(|| wire::corrupt("V2 root row offset"))?;
        let row = wire_v2::root_row(rows, index)?;
        let lower = if index == 0 {
            None
        } else {
            Some(wire_v2::root_row(rows, index - 1)?.upper)
        };
        Ok((lower, row))
    }
}

fn open_root(
    artifact: ObjectRef,
    recipe: TextRecipe,
    exact_source: Option<SourceCut>,
    expected_logical: Option<Digest>,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<RootV2, IndexError> {
    budget.checkpoint()?;
    let object = objects.get(artifact, budget)?;
    if object.registration.reference != artifact {
        return Err(wire::corrupt("V2 root identity"));
    }
    charge_decode(&object, budget)?;
    let view = wire_v2::decode_root(
        &object.payload,
        recipe.output_policy(),
        &object.registration.children,
    )?;
    if exact_source.is_some_and(|source| source != view.source) {
        return Err(IndexError::Conflict);
    }
    let (docs, indexed_tokens) = view.totals();
    if expected_logical.is_some_and(|expected| {
        expected
            != wire_v2::logical_commitment(
                recipe.output_policy(),
                view.source,
                docs,
                indexed_tokens,
                view.path,
            )
    }) {
        return Err(wire::corrupt("V2 root logical commitment"));
    }
    Ok(RootV2 {
        source: view.source,
        summary: Summary {
            docs,
            indexed_tokens,
            path: view.path,
        },
        count: view.count,
        row_offset: view.row_offset(),
        object,
    })
}

pub(super) fn root(
    member: &MemberBinding,
    recipe: TextRecipe,
    exact_source: Option<SourceCut>,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<Summary, IndexError> {
    if member.input.policy != recipe.output_policy()
        || member.input.root != exact_source.map_or(member.input.root, |source| source.owners)
    {
        return Err(IndexError::Conflict);
    }
    let root = open_root(
        member.artifact,
        recipe,
        exact_source,
        Some(member.logical_commitment),
        objects,
        budget,
    )?;
    if root.source.owners != member.input.root {
        return Err(IndexError::Conflict);
    }
    Ok(root.summary)
}

/// The installed root is checked against the pinned Source and logical
/// commitment. New child bodies are checked before sealing by the writer;
/// unchanged children retain their prior immutable validation.
pub(super) fn verify_full(
    member: &MemberBinding,
    recipe: TextRecipe,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let _ = root(member, recipe, None, objects, budget)?;
    Ok(())
}

#[derive(Clone, Copy)]
struct Entry {
    doc: Doc,
    location: DocumentLocation,
    layer: usize,
    matched: bool,
}

struct Entries {
    rows: Vec<Entry>,
    _memory: Option<MemoryLease>,
    layers: Vec<LayerTermCursor>,
    _layer_memory: Option<MemoryLease>,
    masks: Vec<u128>,
    mask_memory: Option<MemoryLease>,
}

fn term_mask(
    segment: &Segment,
    key: &[u8],
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<u128, IndexError> {
    let mut routes = Reader::at(&segment.object.payload, segment.route_start);
    let mut mask = 0u128;
    let mut last_doc = None;
    for _ in 0..segment.routes {
        budget.checkpoint()?;
        let route = Route::decode(&mut routes)?;
        if key < route.first {
            break;
        }
        if key > route.last {
            continue;
        }
        let (_, fragment, _) =
            inspect_page(&route, Some(key), segment, None, true, objects, budget)?;
        if let Some(fragment) = fragment {
            if last_doc.is_some_and(|prior| fragment.first_doc <= prior) {
                return Err(wire::corrupt("V2 posting continuation order"));
            }
            last_doc = Some(fragment.last_doc);
            mask |= fragment.selected_docs;
        }
    }
    Ok(mask)
}

fn add_segment(
    entries: &mut Vec<Entry>,
    layers: &mut Vec<LayerTermCursor>,
    segment: Segment,
    mask: u128,
    lower: Option<[u8; 32]>,
    upper: [u8; 32],
    key: Option<&[u8]>,
    known_mask: Option<u128>,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<(u32, u64, u128), IndexError> {
    let matched = match key {
        Some(key) => match known_mask {
            Some(mask) => mask,
            None => term_mask(&segment, key, objects, budget)?,
        },
        None => 0,
    };
    let mut docs = 0u32;
    let mut tokens = 0u64;
    let layer_index = layers.len();
    for index in 0..segment.docs {
        budget.checkpoint()?;
        let doc = segment.doc(index)?;
        let bit = document_bit(index)?;
        if mask & bit == 0 {
            continue;
        }
        if lower.is_some_and(|boundary| doc.owner <= boundary) || doc.owner > upper {
            return Err(wire::corrupt("V2 live document outside partition"));
        }
        docs = docs.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        tokens = tokens
            .checked_add(doc.tokens)
            .ok_or(IndexError::ResourceLimited)?;
        entries.push(Entry {
            doc,
            location: DocumentLocation {
                edge: segment.descriptor(),
                row: index,
            },
            layer: layer_index,
            matched: matched & bit != 0,
        });
        budget.charge(WorkUsage {
            bytes: size_of::<Entry>() as u64,
            ..WorkUsage::default()
        })?;
    }
    let live_matches = matched & mask;
    if known_mask.is_some() && live_matches != 0 {
        layers.push(LayerTermCursor::new(segment));
        budget.charge(WorkUsage {
            bytes: size_of::<LayerTermCursor>() as u64,
            ..WorkUsage::default()
        })?;
    }
    Ok((docs, tokens, live_matches))
}

fn check_layer(actual: SegmentEdge, layer: LayerRow) -> Result<(), IndexError> {
    if actual.reference != layer.reference
        || actual.first != layer.first
        || actual.last != layer.last
        || actual.docs != layer.physical_docs
        || actual.tokens != layer.physical_tokens
    {
        return Err(wire::corrupt("V2 physical layer descriptor"));
    }
    Ok(())
}

fn load_entries(
    root: &RootV2,
    recipe: TextRecipe,
    partition: usize,
    key: Option<&[u8]>,
    known_masks: Option<&[u128]>,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<Entries, IndexError> {
    let (lower, row) = root.row(partition)?;
    let capacity = usize::try_from(row.live_docs).map_err(|_| IndexError::ResourceLimited)?;
    let max_layers = if key.is_some() {
        match row.kind {
            PartitionKind::DirectAll => 1,
            PartitionKind::Manifest => capacity.min(wire_v2::STORAGE_CHILDREN),
        }
    } else {
        0
    };
    let (mut rows, memory) = funded_vec(capacity, budget)?;
    let (mut layers, layer_memory) =
        funded_vec(known_masks.map_or(0, |masks| masks.len()), budget)?;
    let (mut masks, mask_memory) =
        funded_vec(if known_masks.is_none() { max_layers } else { 0 }, budget)?;
    match row.kind {
        PartitionKind::DirectAll => {
            if known_masks.is_some_and(|known| known.len() != 1) {
                return Err(wire::corrupt("V2 direct term mask count"));
            }
            let segment = open_v2_segment(row.reference, recipe, objects, budget)?;
            let descriptor = segment.descriptor();
            if u32::from(descriptor.docs) != row.live_docs || descriptor.tokens != row.live_tokens {
                return Err(wire::corrupt("V2 direct summary"));
            }
            let mask = if segment.docs == 128 {
                u128::MAX
            } else {
                (1u128 << segment.docs) - 1
            };
            let (_, _, term_bits) = add_segment(
                &mut rows,
                &mut layers,
                segment,
                mask,
                lower,
                row.upper,
                key,
                known_masks.map(|known| known[0]),
                objects,
                budget,
            )?;
            if key.is_some() && known_masks.is_none() {
                masks.push(term_bits);
            }
        }
        PartitionKind::Manifest => {
            let object = objects.get(row.reference, budget)?;
            if object.registration.reference != row.reference {
                return Err(wire::corrupt("V2 partition identity"));
            }
            charge_decode(&object, budget)?;
            let view = wire_v2::decode_partition(
                &object.payload,
                recipe.output_policy(),
                lower,
                row.upper,
                row.live_docs,
                row.live_tokens,
                &object.registration.children,
            )?;
            if known_masks.is_some_and(|known| known.len() != view.count) {
                return Err(wire::corrupt("V2 manifest term mask count"));
            }
            for index in 0..view.count {
                budget.checkpoint()?;
                let layer = view.layer(index)?;
                let segment = open_v2_segment(layer.reference, recipe, objects, budget)?;
                check_layer(segment.descriptor(), layer)?;
                let (docs, tokens, term_bits) = add_segment(
                    &mut rows,
                    &mut layers,
                    segment,
                    layer.live_mask,
                    lower,
                    row.upper,
                    key,
                    known_masks.map(|known| known[index]),
                    objects,
                    budget,
                )?;
                if key.is_some() && known_masks.is_none() {
                    masks.push(term_bits);
                }
                if docs != u32::from(layer.live_docs) || tokens != layer.live_tokens {
                    return Err(wire::corrupt("V2 live layer summary"));
                }
            }
        }
    }
    if rows.len() != capacity {
        return Err(wire::corrupt("V2 live document count"));
    }
    rows.sort_unstable_by_key(|entry| entry.doc.owner);
    if rows
        .windows(2)
        .any(|pair| pair[0].doc.owner == pair[1].doc.owner)
    {
        return Err(wire::corrupt("V2 duplicate live owner"));
    }
    let tokens = rows.iter().try_fold(0u64, |total, entry| {
        total
            .checked_add(entry.doc.tokens)
            .ok_or(IndexError::ResourceLimited)
    })?;
    if tokens != row.live_tokens {
        return Err(wire::corrupt("V2 partition token total"));
    }
    Ok(Entries {
        rows,
        _memory: memory,
        layers,
        _layer_memory: layer_memory,
        masks,
        mask_memory,
    })
}

fn load_document_root<'a>(
    slot: &'a mut Option<RootV2>,
    artifact: ObjectRef,
    recipe: TextRecipe,
    context: &mut TextReadContext<'_>,
) -> Result<&'a RootV2, IndexError> {
    if slot.is_none() {
        *slot = Some(open_root(
            artifact,
            recipe,
            Some(context.ticket.view.source),
            None,
            context.objects,
            context.budget,
        )?);
    }
    slot.as_ref()
        .filter(|root| root.source == context.ticket.view.source)
        .ok_or(IndexError::Conflict)
}

pub(super) struct DocumentLookup {
    artifact: ObjectRef,
    recipe: TextRecipe,
    root: Option<RootV2>,
    cached: Option<(usize, Entries)>,
}

impl DocumentLookup {
    pub(super) fn new(artifact: ObjectRef, recipe: TextRecipe) -> Self {
        Self {
            artifact,
            recipe,
            root: None,
            cached: None,
        }
    }

    pub(super) fn locate(
        &mut self,
        owner: OwnerKey,
        context: &mut TextReadContext<'_>,
    ) -> Result<Option<(DocumentLocation, Doc)>, IndexError> {
        let root = load_document_root(&mut self.root, self.artifact, self.recipe, context)?;
        let key = super::super::super::owner_key(owner);
        let mut low = 0usize;
        let mut high = root.count;
        while low < high {
            context.budget.checkpoint()?;
            let mid = low.midpoint(high);
            if key <= root.row(mid)?.1.upper {
                high = mid;
            } else {
                low = mid + 1;
            }
        }
        if low == root.count {
            return Ok(None);
        }
        if self.cached.as_ref().is_none_or(|(slot, _)| *slot != low) {
            self.cached = None;
            self.cached = Some((
                low,
                load_entries(
                    root,
                    self.recipe,
                    low,
                    None,
                    None,
                    context.objects,
                    context.budget,
                )?,
            ));
        }
        let entries = &self
            .cached
            .as_ref()
            .ok_or_else(|| wire::corrupt("V2 partition cache"))?
            .1
            .rows;
        Ok(entries
            .binary_search_by_key(&key, |entry| entry.doc.owner)
            .ok()
            .map(|index| {
                let entry = entries[index];
                (entry.location, entry.doc)
            }))
    }
}

pub(super) struct DocumentCursor {
    artifact: ObjectRef,
    recipe: TextRecipe,
    root: Option<RootV2>,
    entries: Option<Entries>,
    partition: usize,
    next_doc: usize,
}

impl DocumentCursor {
    pub(super) fn new(artifact: ObjectRef, recipe: TextRecipe) -> Self {
        Self {
            artifact,
            recipe,
            root: None,
            entries: None,
            partition: 0,
            next_doc: 0,
        }
    }

    pub(super) fn next(
        &mut self,
        context: &mut TextReadContext<'_>,
    ) -> Result<Option<super::super::super::ValidatedUniverseOwner>, IndexError> {
        let root = load_document_root(&mut self.root, self.artifact, self.recipe, context)?;
        loop {
            context.budget.checkpoint()?;
            if let Some(entries) = &self.entries {
                if let Some(entry) = entries.rows.get(self.next_doc) {
                    self.next_doc += 1;
                    return validate_segment_document(entry.doc, self.recipe, context).map(Some);
                }
            }
            if self.partition == root.count {
                self.entries = None;
                return Ok(None);
            }
            self.entries = None;
            self.entries = Some(load_entries(
                root,
                self.recipe,
                self.partition,
                None,
                None,
                context.objects,
                context.budget,
            )?);
            self.partition += 1;
            self.next_doc = 0;
        }
    }
}

struct ActivePage {
    object: StoredObject,
    cursor: usize,
    end: usize,
    remaining: u16,
    last_doc: Option<u32>,
}

struct LayerTermCursor {
    segment: Segment,
    route_slot: usize,
    route_offset: usize,
    page: Option<ActivePage>,
    last_fragment_doc: Option<u32>,
}

impl LayerTermCursor {
    fn new(segment: Segment) -> Self {
        let route_offset = segment.route_start;
        Self {
            segment,
            route_slot: 0,
            route_offset,
            page: None,
            last_fragment_doc: None,
        }
    }

    fn next_for(
        &mut self,
        target_row: usize,
        recipe: TextRecipe,
        key: &[u8],
        context: &mut TextReadContext<'_>,
    ) -> Result<ValidatedPostingHit, IndexError> {
        let target = u32::try_from(target_row).map_err(|_| IndexError::ResourceLimited)?;
        let doc = self.segment.doc(target_row)?;
        loop {
            context.budget.checkpoint()?;
            if let Some(page) = &mut self.page {
                if page.remaining > 0 {
                    let mut cursor = Reader::at(&page.object.payload, page.cursor);
                    let delta = cursor.varint()?;
                    let docid = if let Some(previous) = page.last_doc {
                        if delta == 0 {
                            return Err(wire::corrupt("V2 posting docID delta"));
                        }
                        previous
                            .checked_add(delta)
                            .ok_or(IndexError::ResourceLimited)?
                    } else {
                        delta
                    };
                    let tf = usize::try_from(cursor.varint()?)
                        .map_err(|_| IndexError::ResourceLimited)?;
                    if docid > target {
                        return Err(wire::corrupt("V2 membership without posting"));
                    }
                    if docid < target {
                        for _ in 0..tf {
                            let _ = cursor.varint()?;
                        }
                        page.cursor = cursor.offset;
                        page.last_doc = Some(docid);
                        page.remaining -= 1;
                        continue;
                    }
                    let bytes = tf
                        .checked_mul(size_of::<u32>())
                        .ok_or(IndexError::ResourceLimited)?;
                    let memory = context
                        .budget
                        .reserve_memory(bytes as u64, MemoryLifetime::Temporary)?;
                    let mut positions = Vec::new();
                    positions
                        .try_reserve_exact(tf)
                        .map_err(|_| IndexError::ResourceLimited)?;
                    if positions.capacity() > tf {
                        return Err(IndexError::ResourceLimited);
                    }
                    let mut previous: Option<u32> = None;
                    for index in 0..tf {
                        context.budget.checkpoint()?;
                        let value = cursor.varint()?;
                        let position = if index == 0 {
                            value
                        } else {
                            if value == 0 {
                                return Err(wire::corrupt("V2 posting position delta"));
                            }
                            previous
                                .ok_or_else(|| wire::corrupt("V2 posting position predecessor"))?
                                .checked_add(value)
                                .ok_or(IndexError::ResourceLimited)?
                        };
                        positions.push(position);
                        previous = Some(position);
                    }
                    if cursor.offset > page.end || tf as u64 > doc.tokens {
                        return Err(wire::corrupt("V2 posting bounds or frequency"));
                    }
                    page.cursor = cursor.offset;
                    page.last_doc = Some(docid);
                    page.remaining -= 1;
                    let validated = validate_segment_document(doc, recipe, context)?;
                    if validated.doc_indexed_tokens != doc.tokens {
                        return Err(wire::corrupt("V2 posting document token mismatch"));
                    }
                    context.budget.charge(WorkUsage {
                        bytes: bytes as u64,
                        ..WorkUsage::default()
                    })?;
                    return Ok(ValidatedPostingHit {
                        owner: validated.owner,
                        source_record: validated.source_record,
                        doc_indexed_tokens: validated.doc_indexed_tokens,
                        occurrence:
                            crate::native::text_occurrence::ScopedOccurrence::from_checked_positions(
                                positions, memory,
                            ),
                    });
                }
            }
            if let Some(page) = self.page.take() {
                if page.cursor != page.end {
                    return Err(wire::corrupt("V2 posting fragment EOF"));
                }
            }
            if self.route_slot == self.segment.routes {
                return Err(wire::corrupt("V2 membership without posting"));
            }
            let mut routes = Reader::at(&self.segment.object.payload, self.route_offset);
            let route = Route::decode(&mut routes)?;
            self.route_offset = routes.offset;
            self.route_slot += 1;
            if key < route.first {
                return Err(wire::corrupt("V2 membership without posting"));
            }
            if key > route.last {
                continue;
            }
            let (object, fragment, _) = inspect_page(
                &route,
                Some(key),
                &self.segment,
                None,
                false,
                context.objects,
                context.budget,
            )?;
            let Some(fragment) = fragment else {
                continue;
            };
            if self
                .last_fragment_doc
                .is_some_and(|prior| fragment.first_doc <= prior)
            {
                return Err(wire::corrupt("V2 posting continuation order"));
            }
            self.last_fragment_doc = Some(fragment.last_doc);
            if target > fragment.last_doc {
                continue;
            }
            if target < fragment.first_doc {
                return Err(wire::corrupt("V2 membership without posting"));
            }
            self.page = Some(ActivePage {
                object,
                cursor: fragment.start,
                end: fragment.end,
                remaining: fragment.df,
                last_doc: None,
            });
        }
    }
}

struct PartitionMasks {
    masks: Vec<u128>,
    _memory: Option<MemoryLease>,
}

pub(super) struct TermCursor {
    root: RootV2,
    recipe: TextRecipe,
    key: Vec<u8>,
    _key_memory: Option<MemoryLease>,
    df: u64,
    visited: u64,
    partition: usize,
    next_doc: usize,
    entries: Option<Entries>,
    masks: Vec<PartitionMasks>,
    _masks_memory: Option<MemoryLease>,
    last_owner: Option<[u8; 32]>,
}

impl TermCursor {
    pub(super) fn new(
        artifact: ObjectRef,
        recipe: TextRecipe,
        key: &[u8],
        context: &mut TextReadContext<'_>,
    ) -> Result<Self, IndexError> {
        if key.is_empty() || key.len() > wire::MAX_KEY {
            return Err(wire::corrupt("V2 query term width"));
        }
        let _ = crate::native::text_memory::decode_case_term_key(key)?;
        let root = open_root(
            artifact,
            recipe,
            Some(context.ticket.view.source),
            None,
            context.objects,
            context.budget,
        )?;
        let key_memory = context
            .budget
            .reserve_memory(key.len() as u64, MemoryLifetime::Temporary)?;
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(key.len())
            .map_err(|_| IndexError::ResourceLimited)?;
        if owned.capacity() > key.len() {
            return Err(IndexError::ResourceLimited);
        }
        owned.extend_from_slice(key);
        context.budget.charge(WorkUsage {
            bytes: key.len() as u64,
            ..WorkUsage::default()
        })?;
        let (mut masks, masks_memory) = funded_vec(root.count, context.budget)?;
        let mut df = 0u64;
        for partition in 0..root.count {
            context.budget.checkpoint()?;
            let mut entries = load_entries(
                &root,
                recipe,
                partition,
                Some(key),
                None,
                context.objects,
                context.budget,
            )?;
            let matches = entries.rows.iter().filter(|entry| entry.matched).count();
            df = df
                .checked_add(matches as u64)
                .ok_or(IndexError::ResourceLimited)?;
            masks.push(PartitionMasks {
                masks: std::mem::take(&mut entries.masks),
                _memory: entries.mask_memory.take(),
            });
            context.budget.charge(WorkUsage {
                bytes: size_of::<PartitionMasks>() as u64,
                ..WorkUsage::default()
            })?;
        }
        if df > root.summary.docs {
            return Err(wire::corrupt("V2 term DF exceeds live docs"));
        }
        Ok(Self {
            root,
            recipe,
            key: owned,
            _key_memory: key_memory,
            df,
            visited: 0,
            partition: 0,
            next_doc: 0,
            entries: None,
            masks,
            _masks_memory: masks_memory,
            last_owner: None,
        })
    }

    pub(super) const fn df(&self) -> u64 {
        self.df
    }

    pub(super) fn next_hit(
        &mut self,
        context: &mut TextReadContext<'_>,
    ) -> Result<Option<ValidatedPostingHit>, IndexError> {
        if self.df == 0 {
            return Ok(None);
        }
        loop {
            context.budget.checkpoint()?;
            if let Some(entries) = &mut self.entries {
                while let Some(entry) = entries.rows.get(self.next_doc).copied() {
                    self.next_doc += 1;
                    if !entry.matched {
                        continue;
                    }
                    if self
                        .last_owner
                        .is_some_and(|prior| entry.doc.owner <= prior)
                    {
                        return Err(wire::corrupt("V2 posting owner order"));
                    }
                    let layer = entries
                        .layers
                        .get_mut(entry.layer)
                        .ok_or_else(|| wire::corrupt("V2 active term layer"))?;
                    let hit =
                        layer.next_for(entry.location.row, self.recipe, &self.key, context)?;
                    self.last_owner = Some(entry.doc.owner);
                    self.visited = self
                        .visited
                        .checked_add(1)
                        .ok_or(IndexError::ResourceLimited)?;
                    if self.visited > self.df {
                        return Err(wire::corrupt("V2 posting exceeds DF"));
                    }
                    return Ok(Some(hit));
                }
            }
            if self.partition == self.root.count {
                self.entries = None;
                if self.visited != self.df {
                    return Err(wire::corrupt("V2 posting differs from DF"));
                }
                return Ok(None);
            }
            self.entries = None;
            let current = self.partition;
            self.partition += 1;
            let masks = &self
                .masks
                .get(current)
                .ok_or_else(|| wire::corrupt("V2 partition term masks"))?
                .masks;
            if masks.iter().all(|&mask| mask == 0) {
                continue;
            }
            self.entries = Some(load_entries(
                &self.root,
                self.recipe,
                current,
                Some(&self.key),
                Some(masks),
                context.objects,
                context.budget,
            )?);
            self.next_doc = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::index::{
        MapRoot, ObjectBatchInput, RegisteredObject, RegisteredObjectBatch, WorkLimits,
        ports::ObjectWriteSink,
    };
    use std::{
        collections::BTreeMap,
        time::{Duration, Instant},
    };

    #[derive(Default)]
    struct FixtureObjects(BTreeMap<ObjectRef, StoredObject>);

    impl FixtureObjects {
        fn add(&mut self, reference: ObjectRef, payload: Vec<u8>, children: Vec<ObjectRef>) {
            let bytes = payload.len() as u64;
            let _previous = self.0.insert(
                reference,
                StoredObject::new(
                    RegisteredObject::unscoped(reference, bytes, children),
                    payload,
                    None,
                ),
            );
        }
    }

    impl ImmutableObjectPort for FixtureObjects {
        fn identify(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("read-only fixture".into()))
        }
        fn put(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("read-only fixture".into()))
        }
        fn install(
            &self,
            _: &[u8],
            _: &[ObjectRef],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            Err(IndexError::BackendViolation("read-only fixture".into()))
        }
        fn install_batch(
            &self,
            _: &[ObjectBatchInput],
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<RegisteredObjectBatch, IndexError> {
            Err(IndexError::BackendViolation("read-only fixture".into()))
        }
        fn get(
            &self,
            reference: ObjectRef,
            budget: &mut WorkBudget,
        ) -> Result<StoredObject, IndexError> {
            budget.checkpoint()?;
            self.0
                .get(&reference)
                .cloned()
                .ok_or(IndexError::SnapshotUnavailable)
        }
    }

    fn reference(byte: u8) -> ObjectRef {
        ObjectRef(Digest([byte; 32]))
    }

    fn posting_page() -> Result<Vec<u8>, IndexError> {
        let mut page = Vec::new();
        page.extend_from_slice(wire::PAGE_MAGIC);
        for key in [b"\0x".as_slice(), b"\x01x".as_slice()] {
            wire::push_key(&mut page, key)?;
            page.extend_from_slice(&3u16.to_be_bytes());
            let body = [0u8, 1, 0, 1, 1, 0, 1, 1, 0];
            page.extend_from_slice(&(body.len() as u32).to_be_bytes());
            page.extend_from_slice(&body);
        }
        Ok(page)
    }

    fn segment_payload(recipe: TextRecipe, page_ref: ObjectRef) -> Result<Vec<u8>, IndexError> {
        let mut payload = Vec::new();
        payload.extend_from_slice(wire::SEGMENT_MAGIC);
        payload.extend_from_slice(&recipe.segment_child_recipe().output_policy().0);
        payload.extend_from_slice(&3u16.to_be_bytes());
        for (owner, source) in [(0x10u8, 1u8), (0x20, 2), (0x30, 3)] {
            payload.extend_from_slice(&[owner; 32]);
            payload.extend_from_slice(&reference(source).0.0);
            payload.extend_from_slice(&1u64.to_be_bytes());
        }
        payload.extend_from_slice(&1u16.to_be_bytes());
        wire::write_route(&mut payload, page_ref, b"\0x", b"\x01x")?;
        Ok(payload)
    }

    #[test]
    fn shared_physical_segment_checks_each_live_partition_mask() -> Result<(), IndexError> {
        let recipe = TextRecipe::segmented_layered(Digest([7; 32]));
        let root_ref = reference(40);
        let left_ref = reference(41);
        let right_ref = reference(42);
        let segment_ref = reference(43);
        let page_ref = reference(44);
        let segment = segment_payload(recipe, page_ref)?;
        let layer = LayerRow {
            reference: segment_ref,
            first: [0x10; 32],
            last: [0x30; 32],
            physical_docs: 3,
            physical_tokens: 3,
            live_mask: 0b011,
            live_docs: 2,
            live_tokens: 2,
        };
        let lower = [0x20; 32];
        let mut left = Vec::new();
        let mut right = Vec::new();
        let left_statistics =
            wire_v2::encode_partition(&mut left, recipe.output_policy(), None, lower, &[layer])?;
        let right_statistics = wire_v2::encode_partition(
            &mut right,
            recipe.output_policy(),
            Some(lower),
            [0xff; 32],
            &[LayerRow {
                live_mask: 0b100,
                live_docs: 1,
                live_tokens: 1,
                ..layer
            }],
        )?;
        assert_eq!(left_statistics, (2, 2));
        assert_eq!(right_statistics, (1, 1));
        let source = SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        };
        let mut root_payload = Vec::new();
        let root_statistics = wire_v2::encode_root(
            &mut root_payload,
            recipe.output_policy(),
            source,
            PathIndex {
                postings: MapRoot(None),
                statistics: crate::native::text::TextStatistics {
                    docs: 3,
                    indexed_tokens: 0,
                },
            },
            &[
                RootRow {
                    upper: lower,
                    kind: PartitionKind::Manifest,
                    reference: left_ref,
                    live_docs: 2,
                    live_tokens: 2,
                },
                RootRow {
                    upper: [0xff; 32],
                    kind: PartitionKind::Manifest,
                    reference: right_ref,
                    live_docs: 1,
                    live_tokens: 1,
                },
            ],
        )?;
        assert_eq!(root_statistics, (3, 3));
        let mut objects = FixtureObjects::default();
        objects.add(page_ref, posting_page()?, Vec::new());
        objects.add(segment_ref, segment, vec![page_ref]);
        objects.add(left_ref, left, vec![segment_ref]);
        objects.add(right_ref, right, vec![segment_ref]);
        objects.add(root_ref, root_payload, vec![left_ref, right_ref]);
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 1_000,
                writes: 0,
                bytes: 1_000_000,
                mutations: 0,
            },
            Instant::now()
                .checked_add(Duration::from_secs(10))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let root = open_root(root_ref, recipe, Some(source), None, &objects, &mut budget)?;
        let left_docs = load_entries(&root, recipe, 0, None, None, &objects, &mut budget)?;
        let right_docs = load_entries(&root, recipe, 1, None, None, &objects, &mut budget)?;
        assert_eq!(
            left_docs
                .rows
                .iter()
                .map(|entry| entry.doc.owner)
                .collect::<Vec<_>>(),
            vec![[0x10; 32], [0x20; 32]]
        );
        assert_eq!(
            right_docs
                .rows
                .iter()
                .map(|entry| entry.doc.owner)
                .collect::<Vec<_>>(),
            vec![[0x30; 32]]
        );
        let left_term = load_entries(&root, recipe, 0, Some(b"\0x"), None, &objects, &mut budget)?;
        let right_term = load_entries(&root, recipe, 1, Some(b"\0x"), None, &objects, &mut budget)?;
        assert_eq!(
            left_term.rows.iter().filter(|entry| entry.matched).count()
                + right_term.rows.iter().filter(|entry| entry.matched).count(),
            3
        );

        // The authenticated layer descriptor still matches, and wire summaries
        // still add up, but a selected owner from the left range is illegal.
        let mut forged = Vec::new();
        let forged_statistics = wire_v2::encode_partition(
            &mut forged,
            recipe.output_policy(),
            Some(lower),
            [0xff; 32],
            &[LayerRow {
                live_mask: 0b001,
                live_docs: 1,
                live_tokens: 1,
                ..layer
            }],
        )?;
        assert_eq!(forged_statistics, right_statistics);
        objects.add(right_ref, forged, vec![segment_ref]);
        assert!(matches!(
            load_entries(&root, recipe, 1, None, None, &objects, &mut budget),
            Err(IndexError::CorruptObject(_))
        ));
        Ok(())
    }
}
