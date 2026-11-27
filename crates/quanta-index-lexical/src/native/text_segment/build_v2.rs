//! Copy-on-write Native Text root. Existing V1 posting segments are immutable;
//! a patch rewrites only the owner partition touched by its Source delta.
#![expect(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    reason = "bounded partition rows and document masks have checked capacities"
)]

use super::super::token_prefix_cache::TokenPrefixCache;
use super::super::{TextRecipe, check_admission, owner_key};
use super::build::{Window, copy_work, funded_vec, seal_observed};
use super::wire::{self, SegmentEdge};
use super::wire_v2::{self, LayerRow, PartitionKind, RootRow, TextCut};
use crate::native::text_path::{PathIndex, PathPostingValue, PathWrites};
use quanta_index_core::DiagnosticStageV1;
use quanta_index_core::index::{
    AttemptContext, DeltaInput, Digest, IndexError, MemberBinding, MemoryLease, ObjectRef,
    SourceCut, SourceDelta, SourceInputCursor,
};
use std::mem::size_of;

const MAX_OWNER: [u8; 32] = [0xff; 32];

fn full_mask(docs: u16) -> Result<u128, IndexError> {
    if docs == 0 || usize::from(docs) > wire::MAX_DOCS {
        return Err(wire::corrupt("V2 segment document bound"));
    }
    Ok(if docs == 128 {
        u128::MAX
    } else {
        (1u128 << docs) - 1
    })
}

fn layer_from_edge(edge: SegmentEdge) -> Result<LayerRow, IndexError> {
    Ok(LayerRow {
        reference: edge.reference,
        first: edge.first,
        last: edge.last,
        physical_docs: edge.docs,
        physical_tokens: edge.tokens,
        live_mask: full_mask(edge.docs)?,
        live_docs: edge.docs,
        live_tokens: edge.tokens,
    })
}

fn direct_row(edge: SegmentEdge, upper: [u8; 32]) -> RootRow {
    RootRow {
        upper,
        kind: PartitionKind::DirectAll,
        reference: edge.reference,
        live_docs: u32::from(edge.docs),
        live_tokens: edge.tokens,
    }
}

fn seal_verified(
    window: &mut Window,
    recipe: TextRecipe,
    edges: &mut Vec<SegmentEdge>,
    edge_memory: &mut Option<MemoryLease>,
    cache: &mut Option<Box<TokenPrefixCache>>,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    if window.docs.is_empty() {
        return Ok(());
    }
    if edges.len() == edges.capacity() {
        if edges.len() == wire::MAX_SEGMENTS {
            return Err(IndexError::ResourceLimited);
        }
        let next_capacity = edges
            .capacity()
            .max(4)
            .checked_mul(2)
            .ok_or(IndexError::ResourceLimited)?
            .min(wire::MAX_SEGMENTS);
        let (mut grown, memory) = funded_vec(next_capacity, context.budget)?;
        copy_work(context.budget, edges.len() * size_of::<SegmentEdge>())?;
        grown.extend_from_slice(edges);
        *edges = grown;
        *edge_memory = memory;
    }
    let before = edges.len();
    seal_observed(
        window,
        recipe.segment_child_recipe().output_policy(),
        edges,
        cache,
        context,
    )?;
    for edge in edges.iter().skip(before) {
        let segment =
            super::read::open_v2_segment(edge.reference, recipe, context.objects, context.budget)?;
        if segment.descriptor() != *edge {
            return Err(wire::corrupt("V2 newly sealed segment descriptor"));
        }
        super::read::verify_v2_segment(&segment, context.objects, context.budget)?;
    }
    Ok(())
}

fn read_root(
    member: &MemberBinding,
    recipe: TextRecipe,
    source: SourceCut,
    context: &mut AttemptContext<'_>,
) -> Result<(Vec<RootRow>, Option<MemoryLease>, PathIndex), IndexError> {
    if member.input.policy != recipe.output_policy() || member.input.root != source.owners {
        return Err(IndexError::Conflict);
    }
    let object = context.objects.get(member.artifact, context.budget)?;
    if object.registration.reference != member.artifact {
        return Err(wire::corrupt("V2 prior root identity"));
    }
    copy_work(context.budget, object.payload.len())?;
    if let Some(refusal) = super::retired::refuse_retired_root(
        &object.payload,
        recipe.output_policy(),
        &object.registration.children,
        Some(member.logical_commitment),
    ) {
        return Err(refusal);
    }
    let view = wire_v2::decode_root(
        &object.payload,
        recipe.output_policy(),
        &object.registration.children,
    )?;
    let cut = TextCut::of(source);
    if view.cut != cut {
        return Err(IndexError::Conflict);
    }
    let (docs, tokens) = view.totals();
    if member.logical_commitment
        != wire_v2::logical_commitment(recipe.output_policy(), cut, docs, tokens, view.path)
    {
        return Err(wire::corrupt("V2 prior logical commitment"));
    }
    let (mut rows, lease) = funded_vec(view.count, context.budget)?;
    for index in 0..view.count {
        context.budget.checkpoint()?;
        copy_work(context.budget, size_of::<RootRow>())?;
        rows.push(view.row(index)?);
    }
    Ok((rows, lease, view.path))
}

fn load_layers(
    row: RootRow,
    lower: Option<[u8; 32]>,
    recipe: TextRecipe,
    context: &mut AttemptContext<'_>,
) -> Result<(Vec<LayerRow>, Option<MemoryLease>), IndexError> {
    if row.kind == PartitionKind::DirectAll {
        let segment =
            super::read::open_v2_segment(row.reference, recipe, context.objects, context.budget)?;
        let edge = segment.descriptor();
        if lower.is_some_and(|bound| edge.first <= bound)
            || edge.last > row.upper
            || u32::from(edge.docs) != row.live_docs
            || edge.tokens != row.live_tokens
        {
            return Err(wire::corrupt("V2 prior direct segment"));
        }
        let (mut layers, lease) = funded_vec(1, context.budget)?;
        copy_work(context.budget, size_of::<LayerRow>())?;
        layers.push(layer_from_edge(edge)?);
        return Ok((layers, lease));
    }
    let object = context.objects.get(row.reference, context.budget)?;
    if object.registration.reference != row.reference {
        return Err(wire::corrupt("V2 prior partition identity"));
    }
    copy_work(context.budget, object.payload.len())?;
    let view = wire_v2::decode_partition(
        &object.payload,
        recipe.output_policy(),
        lower,
        row.upper,
        row.live_docs,
        row.live_tokens,
        &object.registration.children,
    )?;
    let (mut layers, lease) = funded_vec(view.count, context.budget)?;
    for index in 0..view.count {
        context.budget.checkpoint()?;
        copy_work(context.budget, size_of::<LayerRow>())?;
        layers.push(view.layer(index)?);
    }
    Ok((layers, lease))
}

fn clear_prior(
    layers: &mut [LayerRow],
    owner: [u8; 32],
    expected: Option<ObjectRef>,
    recipe: TextRecipe,
    cached: &mut Option<(usize, super::read::Segment)>,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    let mut found = false;
    for (index, layer) in layers.iter_mut().enumerate() {
        if owner < layer.first || owner > layer.last {
            continue;
        }
        if cached.as_ref().is_none_or(|(slot, _)| *slot != index) {
            *cached = Some((
                index,
                super::read::open_v2_segment(
                    layer.reference,
                    recipe,
                    context.objects,
                    context.budget,
                )?,
            ));
        }
        let segment = &cached.as_ref().ok_or(IndexError::ResourceLimited)?.1;
        let edge = segment.descriptor();
        if edge.first != layer.first
            || edge.last != layer.last
            || edge.docs != layer.physical_docs
            || edge.tokens != layer.physical_tokens
        {
            return Err(wire::corrupt("V2 prior layer descriptor"));
        }
        // A layer has at most 128 sorted document rows. Binary search avoids
        // reading every body again for one changed owner.
        let mut low = 0usize;
        let mut high = usize::from(layer.physical_docs);
        while low < high {
            context.budget.checkpoint()?;
            let mid = low + (high - low) / 2;
            let doc = segment.doc(mid)?;
            if doc.owner < owner {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        if low == usize::from(layer.physical_docs) {
            continue;
        }
        let doc = segment.doc(low)?;
        if doc.owner != owner || layer.live_mask & (1u128 << low) == 0 {
            continue;
        }
        if found || expected != Some(doc.source) {
            return Err(wire::corrupt("V2 changed owner locator"));
        }
        found = true;
        layer.live_mask &= !(1u128 << low);
        layer.live_docs = layer
            .live_docs
            .checked_sub(1)
            .ok_or(IndexError::ResourceLimited)?;
        layer.live_tokens = layer
            .live_tokens
            .checked_sub(doc.tokens)
            .ok_or_else(|| wire::corrupt("V2 live token underflow"))?;
    }
    if found != expected.is_some() {
        return Err(wire::corrupt("V2 prior owner membership"));
    }
    Ok(())
}

struct Active {
    row: RootRow,
    lower: Option<[u8; 32]>,
    layers: Vec<LayerRow>,
    _layers_memory: Option<MemoryLease>,
    new_edges: Vec<SegmentEdge>,
    edges_memory: Option<MemoryLease>,
    window: Option<Window>,
    cached_prior_segment: Option<(usize, super::read::Segment)>,
}

impl Active {
    fn open(
        row: RootRow,
        lower: Option<[u8; 32]>,
        recipe: TextRecipe,
        context: &mut AttemptContext<'_>,
    ) -> Result<Self, IndexError> {
        let (layers, layers_memory) = load_layers(row, lower, recipe, context)?;
        let (new_edges, edges_memory) = funded_vec(4, context.budget)?;
        Ok(Self {
            row,
            lower,
            layers,
            _layers_memory: layers_memory,
            new_edges,
            edges_memory,
            window: None,
            cached_prior_segment: None,
        })
    }

    fn seal(
        &mut self,
        recipe: TextRecipe,
        cache: &mut Option<Box<TokenPrefixCache>>,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        if let Some(window) = self.window.as_mut() {
            seal_verified(
                window,
                recipe,
                &mut self.new_edges,
                &mut self.edges_memory,
                cache,
                context,
            )?;
        }
        Ok(())
    }

    fn finish(
        mut self,
        recipe: TextRecipe,
        allow_split: bool,
        cache: &mut Option<Box<TokenPrefixCache>>,
        context: &mut AttemptContext<'_>,
    ) -> Result<[Option<RootRow>; 2], IndexError> {
        drop(self.cached_prior_segment.take());
        self.seal(recipe, cache, context)?;
        drop(self.window.take());
        self.layers.retain(|layer| layer.live_mask != 0);
        let needed = self
            .layers
            .len()
            .checked_add(self.new_edges.len())
            .ok_or(IndexError::ResourceLimited)?;
        // The preexisting layer vector was sized to its old count. Fund a
        // replacement with exact fan-in before appending new immutable layers.
        let (mut layers, _memory) = funded_vec(needed, context.budget)?;
        for layer in &self.layers {
            copy_work(context.budget, size_of::<LayerRow>())?;
            layers.push(*layer);
        }
        for edge in &self.new_edges {
            copy_work(context.budget, size_of::<LayerRow>())?;
            layers.push(layer_from_edge(*edge)?);
        }
        drop(std::mem::take(&mut self.layers));
        self._layers_memory = None;
        drop(std::mem::take(&mut self.new_edges));
        self.edges_memory = None;
        compact_pairs(recipe, &mut layers, context)?;
        if layers.len() > wire_v2::STORAGE_CHILDREN {
            return Err(IndexError::ResourceLimited);
        }
        if layers.is_empty() {
            return Ok([None, None]);
        }
        // Split at the first dense 16-layer state. Older larger manifests
        // retain their accepted bounded Patch path when no pair can merge.
        if layers.len() == 16 && allow_split {
            let (cut, mut left, _left_memory, mut right, _right_memory) =
                super::compact_v2::split_masks(
                    recipe,
                    &layers,
                    self.lower,
                    self.row.upper,
                    context,
                )?;
            drop(layers);
            compact_pairs(recipe, &mut left, context)?;
            compact_pairs(recipe, &mut right, context)?;
            let first = persist_partition(recipe, self.lower, cut, &left, context)?;
            let second = persist_partition(recipe, Some(cut), self.row.upper, &right, context)?;
            return Ok([Some(first), Some(second)]);
        }
        Ok([
            Some(persist_partition(
                recipe,
                self.lower,
                self.row.upper,
                &layers,
                context,
            )?),
            None,
        ])
    }
}

fn compact_pairs(
    recipe: TextRecipe,
    layers: &mut Vec<LayerRow>,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    // The 16-layer trigger is a physical scheduling choice. Exact 4096-child
    // root and partition bounds are checked separately by the wire encoder.
    while layers.len() >= 16 {
        context.budget.checkpoint()?;
        let pair = (0..layers.len() - 1).rev().find(|&index| {
            u32::from(layers[index].live_docs) + u32::from(layers[index + 1].live_docs)
                <= wire::MAX_DOCS as u32
        });
        let Some(index) = pair else { break };
        let edge = super::compact_v2::compact(recipe, &layers[index..index + 2], context)?;
        copy_work(
            context.budget,
            (layers.len() - index - 1) * size_of::<LayerRow>(),
        )?;
        layers[index] = layer_from_edge(edge)?;
        let _removed = layers.remove(index + 1);
    }
    Ok(())
}

fn persist_partition(
    recipe: TextRecipe,
    lower: Option<[u8; 32]>,
    upper: [u8; 32],
    layers: &[LayerRow],
    context: &mut AttemptContext<'_>,
) -> Result<RootRow, IndexError> {
    if layers.is_empty() || layers.len() > wire_v2::STORAGE_CHILDREN {
        return Err(IndexError::ResourceLimited);
    }
    if let [layer] = layers
        && layer.live_docs == layer.physical_docs
        && lower.is_none_or(|bound| layer.first > bound)
        && layer.last <= upper
    {
        return Ok(direct_row(
            SegmentEdge {
                reference: layer.reference,
                first: layer.first,
                last: layer.last,
                docs: layer.physical_docs,
                tokens: layer.physical_tokens,
            },
            upper,
        ));
    }
    let bytes = 8usize
        .checked_add(32 + 2)
        .and_then(|n| n.checked_add(layers.len() * wire_v2::LAYER_ROW_BYTES))
        .ok_or(IndexError::ResourceLimited)?;
    let (mut payload, _payload_memory) = funded_vec(bytes, context.budget)?;
    let (mut children, _children_memory) = funded_vec(layers.len(), context.budget)?;
    let (docs, tokens) =
        wire_v2::encode_partition(&mut payload, recipe.output_policy(), lower, upper, layers)?;
    if payload.len() != bytes {
        return Err(IndexError::ResourceLimited);
    }
    children.extend(layers.iter().map(|layer| layer.reference));
    copy_work(context.budget, payload.len())?;
    let reference = context.put(&payload, &children)?;
    Ok(RootRow {
        upper,
        kind: PartitionKind::Manifest,
        reference,
        live_docs: docs,
        live_tokens: tokens,
    })
}

fn push_root_row(
    rows: &mut Vec<RootRow>,
    row: RootRow,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    if rows.len() == rows.capacity() {
        return Err(IndexError::ResourceLimited);
    }
    copy_work(context.budget, size_of::<RootRow>())?;
    rows.push(row);
    Ok(())
}

fn can_split_root(already_emitted: usize, old_remaining: usize, source: SourceCut) -> bool {
    // Reserve the optional path-root registration until PathWrites finishes.
    already_emitted
        .checked_add(old_remaining)
        .and_then(|n| n.checked_add(1))
        .is_some_and(|total| total <= wire_v2::root_row_capacity(TextCut::of(source), true))
}

fn write_root(
    recipe: TextRecipe,
    source: SourceCut,
    path: PathIndex,
    rows: &[RootRow],
    context: &mut AttemptContext<'_>,
) -> Result<(ObjectRef, Digest), IndexError> {
    let diagnostic = context.budget.request().diagnostic_context().cloned();
    super::super::observe_attempt(
        context,
        diagnostic.as_ref(),
        DiagnosticStageV1::NativeTextRoot,
        |_child, context| {
            let cut = TextCut::of(source);
            let source_count = cut.edges().count();
            let children_count = source_count
                .checked_add(usize::from(path.postings.0.is_some()))
                .and_then(|n| n.checked_add(rows.len()))
                .ok_or(IndexError::ResourceLimited)?;
            let bytes = 8usize
                .checked_add(
                    32 + 2
                        + source_count * 32
                        + 1
                        + usize::from(path.postings.0.is_some()) * 32
                        + 8
                        + 8
                        + 2,
                )
                .and_then(|n| n.checked_add(rows.len() * wire_v2::ROOT_ROW_BYTES))
                .ok_or(IndexError::ResourceLimited)?;
            let (mut payload, _payload_memory) = funded_vec(bytes, context.budget)?;
            let (docs, tokens) =
                wire_v2::encode_root(&mut payload, recipe.output_policy(), cut, path, rows)?;
            if payload.len() != bytes {
                return Err(IndexError::ResourceLimited);
            }
            let (mut children, _children_memory) = funded_vec(children_count, context.budget)?;
            children.extend(cut.edges());
            children.extend(path.postings.0);
            children.extend(rows.iter().map(|row| row.reference));
            copy_work(context.budget, payload.len())?;
            let artifact = context.put(&payload, &children)?;
            Ok((
                artifact,
                wire_v2::logical_commitment(recipe.output_policy(), cut, docs, tokens, path),
            ))
        },
    )
}

/// Build a FULL V2 root or one selective copy-on-write patch.
pub(in crate::native::text) fn build(
    recipe: TextRecipe,
    source: SourceCut,
    input: DeltaInput<SourceDelta>,
    base: Option<&MemberBinding>,
    context: &mut AttemptContext<'_>,
) -> Result<(ObjectRef, Digest), IndexError> {
    let (prior, change_count) = match &input {
        DeltaInput::Bootstrap => {
            if base.is_some() {
                return Err(IndexError::Conflict);
            }
            (None, 0)
        }
        DeltaInput::Patch(delta) => {
            if delta.next != source || (delta.prior.owners.0.is_some() && base.is_none()) {
                return Err(IndexError::Conflict);
            }
            (Some(delta.prior), delta.changed.count)
        }
    };
    let (base_rows, base_memory, base_path) = match (prior, base) {
        (Some(cut), Some(member)) => read_root(member, recipe, cut, context)?,
        _ => (Vec::new(), None, PathIndex::default()),
    };
    // Each mutated owner reaches one prior partition. An affected partition
    // emits at most two rows, so reserve only the prior rows and the possible
    // splits. Cold builds know their exact count after sealing the segments.
    let possible_splits = usize::try_from(change_count)
        .unwrap_or(usize::MAX)
        .min(base_rows.len());
    let row_capacity = base_rows
        .len()
        .saturating_add(possible_splits)
        .min(wire_v2::STORAGE_CHILDREN);
    let mut rows = funded_vec::<RootRow>(row_capacity, context.budget)?;
    let (mut cold_edges, mut cold_memory) =
        funded_vec::<SegmentEdge>(if base_rows.is_empty() { 4 } else { 0 }, context.budget)?;
    let mut changes = SourceInputCursor::new(source, input, context.maps, context.objects)?;
    let mut paths = None;
    let mut cache = None;
    let mut cache_initialized = false;
    let mut previous_owner = None;
    let mut cold_window: Option<Window> = None;
    let mut active: Option<Active> = None;
    let mut base_index = 0usize;
    while let Some(change) = changes.next_change(context.budget)? {
        context.budget.checkpoint()?;
        let owner = owner_key(change.owner);
        if previous_owner.is_some_and(|previous| previous >= owner) {
            return Err(wire::corrupt("V2 change owner order"));
        }
        previous_owner = Some(owner);
        let prior_ref = change
            .prior
            .as_ref()
            .map(|(reference, record)| {
                check_admission(record, recipe).map(|yes| yes.then_some(*reference))
            })
            .transpose()?
            .flatten();
        let next = change
            .next
            .as_ref()
            .map(|(reference, record)| {
                check_admission(record, recipe).map(|yes| yes.then_some((*reference, record)))
            })
            .transpose()?
            .flatten();
        if prior.is_none_or(|cut| cut.owners.0.is_none()) && change.prior.is_some() {
            return Err(IndexError::Conflict);
        }
        // Path membership covers every Source owner, independently of body
        // admission. An identical immutable SourceRecord is a true path no-op.
        if change.prior.as_ref().map(|(reference, _)| *reference)
            != change.next.as_ref().map(|(reference, _)| *reference)
        {
            let path_writer = if let Some(writer) = paths.as_mut() {
                writer
            } else {
                paths.insert(PathWrites::with_value(
                    context.scratch,
                    PathPostingValue::SourceCutDigest,
                    context.budget,
                )?)
            };
            path_writer.push_change(&change, context.budget)?;
        }
        if prior_ref == next.map(|(reference, _)| reference) {
            continue;
        }
        if base_rows.is_empty() {
            if prior_ref.is_some() {
                return Err(wire::corrupt("V2 absent prior partition"));
            }
            if let Some((reference, record)) = next {
                if !cache_initialized {
                    cache = TokenPrefixCache::try_new(recipe, 64, context.budget)?;
                    cache_initialized = true;
                }
                let seal_before = match cold_window.as_ref() {
                    Some(window) if !window.docs.is_empty() => {
                        window.should_seal_before(record.byte_len)?
                    }
                    _ => false,
                };
                // Reserve a potential path root before sealing body children.
                let row_capacity =
                    wire_v2::root_row_capacity(TextCut::of(source), true).min(wire::MAX_SEGMENTS);
                if cold_edges.len() + 1 + usize::from(seal_before) > row_capacity {
                    return Err(IndexError::ResourceLimited);
                }
                if seal_before {
                    seal_verified(
                        cold_window.as_mut().ok_or(IndexError::ResourceLimited)?,
                        recipe,
                        &mut cold_edges,
                        &mut cold_memory,
                        &mut cache,
                        context,
                    )?;
                }
                let window = if let Some(window) = cold_window.as_mut() {
                    window
                } else {
                    cold_window.insert(Window::new(context.budget)?)
                };
                window.append_document(owner, reference, record, &mut cache, context)?;
            }
            continue;
        }
        while base_index < base_rows.len() && owner > base_rows[base_index].upper {
            if let Some(partition) = active.take() {
                let allow_split =
                    can_split_root(rows.0.len(), base_rows.len() - base_index, source);
                for row in partition
                    .finish(recipe, allow_split, &mut cache, context)?
                    .into_iter()
                    .flatten()
                {
                    push_root_row(&mut rows.0, row, context)?;
                }
            } else {
                push_root_row(&mut rows.0, base_rows[base_index], context)?;
            }
            base_index += 1;
        }
        let row = *base_rows
            .get(base_index)
            .ok_or_else(|| wire::corrupt("V2 missing final partition"))?;
        if active.is_none() {
            let lower = base_index
                .checked_sub(1)
                .map(|index| base_rows[index].upper);
            active = Some(Active::open(row, lower, recipe, context)?);
        }
        let partition = active.as_mut().ok_or(IndexError::ResourceLimited)?;
        clear_prior(
            &mut partition.layers,
            owner,
            prior_ref,
            recipe,
            &mut partition.cached_prior_segment,
            context,
        )?;
        if let Some((reference, record)) = next {
            if !cache_initialized {
                cache = TokenPrefixCache::try_new(recipe, 64, context.budget)?;
                cache_initialized = true;
            }
            if partition
                .window
                .as_ref()
                .is_some_and(|window| !window.docs.is_empty())
                && partition
                    .window
                    .as_ref()
                    .ok_or(IndexError::ResourceLimited)?
                    .should_seal_before(record.byte_len)?
            {
                partition.seal(recipe, &mut cache, context)?;
            }
            let window = if let Some(window) = partition.window.as_mut() {
                window
            } else {
                partition.window.insert(Window::new(context.budget)?)
            };
            window.append_document(owner, reference, record, &mut cache, context)?;
        }
    }
    if base_rows.is_empty() {
        if let Some(window) = cold_window.as_mut() {
            seal_verified(
                window,
                recipe,
                &mut cold_edges,
                &mut cold_memory,
                &mut cache,
                context,
            )?;
        }
        rows = funded_vec(cold_edges.len(), context.budget)?;
        for (index, edge) in cold_edges.iter().enumerate() {
            let upper = if index + 1 == cold_edges.len() {
                MAX_OWNER
            } else {
                edge.last
            };
            push_root_row(&mut rows.0, direct_row(*edge, upper), context)?;
        }
    } else {
        if let Some(partition) = active.take() {
            let allow_split = can_split_root(rows.0.len(), base_rows.len() - base_index, source);
            for row in partition
                .finish(recipe, allow_split, &mut cache, context)?
                .into_iter()
                .flatten()
            {
                push_root_row(&mut rows.0, row, context)?;
            }
            base_index += 1;
        }
        for row in base_rows.iter().skip(base_index) {
            push_root_row(&mut rows.0, *row, context)?;
        }
        if let Some(last) = rows.0.last_mut() {
            last.upper = MAX_OWNER;
        }
    }
    drop(cache);
    drop(cold_window);
    drop((base_rows, base_memory, cold_edges, cold_memory));
    let path = match paths {
        Some(writer) => {
            let diagnostic = context.budget.request().diagnostic_context().cloned();
            super::super::observe_text_step(
                diagnostic.as_ref(),
                DiagnosticStageV1::NativeTextPostings,
                49,
                context,
                |context| writer.finish(base_path, context),
            )?
        }
        None => base_path,
    };
    write_root(recipe, source, path, &rows.0, context)
}

/// Repack one selected partition without changing SourceCut or logical text.
/// The caller pins `prior` to the exact selected view before this attempt.
pub(in crate::native::text) fn repack(
    recipe: TextRecipe,
    source: SourceCut,
    prior: &MemberBinding,
    owner: [u8; 32],
    context: &mut AttemptContext<'_>,
) -> Result<(ObjectRef, Digest), IndexError> {
    let (mut rows, _rows_memory, path) = read_root(prior, recipe, source, context)?;
    let index = rows
        .iter()
        .position(|row| owner <= row.upper)
        .ok_or_else(|| {
            IndexError::InvalidInput("V2 selected root has no partition to repack".into())
        })?;
    let selected = rows[index];
    let lower = index.checked_sub(1).map(|slot| rows[slot].upper);
    let (mut layers, _layers_memory) = load_layers(selected, lower, recipe, context)?;
    let pair = (0..layers.len().saturating_sub(1)).rev().find(|&slot| {
        u32::from(layers[slot].live_docs) + u32::from(layers[slot + 1].live_docs)
            <= wire::MAX_DOCS as u32
    });
    if pair.is_none() && layers.len() >= 16 {
        let next_rows = rows
            .len()
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if next_rows > wire_v2::root_row_capacity(TextCut::of(source), path.postings.0.is_some()) {
            return Err(IndexError::ResourceLimited);
        }
        let (cut, mut left, _left_memory, mut right, _right_memory) =
            super::compact_v2::split_masks(recipe, &layers, lower, selected.upper, context)?;
        compact_pairs(recipe, &mut left, context)?;
        compact_pairs(recipe, &mut right, context)?;
        let first = persist_partition(recipe, lower, cut, &left, context)?;
        let second = persist_partition(recipe, Some(cut), selected.upper, &right, context)?;
        if first
            .live_docs
            .checked_add(second.live_docs)
            .ok_or(IndexError::ResourceLimited)?
            != selected.live_docs
            || first
                .live_tokens
                .checked_add(second.live_tokens)
                .ok_or(IndexError::ResourceLimited)?
                != selected.live_tokens
        {
            return Err(wire::corrupt("V2 repack split changed logical summary"));
        }
        let (mut expanded, _expanded_memory) = funded_vec(next_rows, context.budget)?;
        for row in rows.iter().take(index) {
            push_root_row(&mut expanded, *row, context)?;
        }
        push_root_row(&mut expanded, first, context)?;
        push_root_row(&mut expanded, second, context)?;
        for row in rows.iter().skip(index + 1) {
            push_root_row(&mut expanded, *row, context)?;
        }
        let (artifact, logical) = write_root(recipe, source, path, &expanded, context)?;
        if artifact == prior.artifact || logical != prior.logical_commitment {
            return Err(wire::corrupt(
                "V2 repack split changed logical member or had no effect",
            ));
        }
        return Ok((artifact, logical));
    }
    let (slot, count) = pair
        .map(|slot| (slot, 2))
        .or_else(|| {
            layers
                .iter()
                .position(|layer| layer.live_docs < layer.physical_docs)
                .map(|slot| (slot, 1))
        })
        .ok_or_else(|| {
            IndexError::InvalidInput("V2 selected partition has no bounded compaction work".into())
        })?;
    let edge = super::compact_v2::compact(recipe, &layers[slot..slot + count], context)?;
    layers[slot] = layer_from_edge(edge)?;
    if count == 2 {
        copy_work(
            context.budget,
            (layers.len() - slot - 1) * size_of::<LayerRow>(),
        )?;
        let _removed = layers.remove(slot + 1);
    }
    let replacement = persist_partition(recipe, lower, selected.upper, &layers, context)?;
    if replacement.live_docs != selected.live_docs
        || replacement.live_tokens != selected.live_tokens
        || replacement.reference == selected.reference
    {
        return Err(wire::corrupt(
            "V2 repack changed logical summary or had no effect",
        ));
    }
    copy_work(context.budget, size_of::<RootRow>())?;
    rows[index] = replacement;
    let (artifact, logical) = write_root(recipe, source, path, &rows, context)?;
    if artifact == prior.artifact || logical != prior.logical_commitment {
        return Err(wire::corrupt(
            "V2 repack changed logical member or had no effect",
        ));
    }
    Ok((artifact, logical))
}
