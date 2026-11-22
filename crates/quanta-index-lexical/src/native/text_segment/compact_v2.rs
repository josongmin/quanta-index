//! Compact a bounded group of immutable layers using their admitted postings.

use super::super::TextRecipe;
use super::build::{Window, copy_work, funded_vec, seal_observed};
use super::read::{open_v2_segment, replay_v2_postings, verify_v2_segment};
use super::wire::{self, Doc, SegmentEdge};
use super::wire_v2::LayerRow;
use quanta_index_core::index::{AttemptContext, IndexError, MemoryLease};
use std::mem::size_of;

#[derive(Clone, Copy)]
struct SplitDoc {
    owner: [u8; 32],
    tokens: u64,
    layer: u16,
    slot: u16,
}

/// Split only live document masks. The V1 segment reference and its physical
/// descriptor remain unchanged, including when the same segment straddles the
/// new owner fence and is a child of both partition manifests.
pub(super) fn split_masks(
    recipe: TextRecipe,
    layers: &[LayerRow],
    lower: Option<[u8; 32]>,
    upper: [u8; 32],
    context: &mut AttemptContext<'_>,
) -> Result<
    (
        [u8; 32],
        Vec<LayerRow>,
        Option<MemoryLease>,
        Vec<LayerRow>,
        Option<MemoryLease>,
    ),
    IndexError,
> {
    if layers.len() < 2 || layers.len() > super::wire_v2::STORAGE_CHILDREN {
        return Err(IndexError::ResourceLimited);
    }
    let total_docs = layers.iter().try_fold(0usize, |sum, layer| {
        sum.checked_add(usize::from(layer.live_docs))
            .ok_or(IndexError::ResourceLimited)
    })?;
    if total_docs < 2
        || total_docs
            > super::wire_v2::STORAGE_CHILDREN
                .checked_mul(wire::MAX_DOCS)
                .ok_or(IndexError::ResourceLimited)?
    {
        return Err(IndexError::ResourceLimited);
    }
    let (mut directory, _directory_memory) = funded_vec::<SplitDoc>(total_docs, context.budget)?;
    for (index, layer) in layers.iter().enumerate() {
        context.budget.checkpoint()?;
        let segment = open_v2_segment(layer.reference, recipe, context.objects, context.budget)?;
        if segment.descriptor()
            != (SegmentEdge {
                reference: layer.reference,
                first: layer.first,
                last: layer.last,
                docs: layer.physical_docs,
                tokens: layer.physical_tokens,
            })
        {
            return Err(wire::corrupt("split physical layer descriptor"));
        }
        let start = directory.len();
        let mut tokens = 0u64;
        for slot in 0..usize::from(layer.physical_docs) {
            context.budget.checkpoint()?;
            if layer.live_mask & (1u128 << slot) == 0 {
                continue;
            }
            let doc = segment.doc(slot)?;
            if lower.is_some_and(|bound| doc.owner <= bound) || doc.owner > upper {
                return Err(wire::corrupt("split live owner outside partition"));
            }
            tokens = tokens
                .checked_add(doc.tokens)
                .ok_or(IndexError::ResourceLimited)?;
            copy_work(context.budget, size_of::<SplitDoc>())?;
            directory.push(SplitDoc {
                owner: doc.owner,
                tokens: doc.tokens,
                layer: u16::try_from(index).map_err(|_error| IndexError::ResourceLimited)?,
                slot: u16::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?,
            });
        }
        if directory.len() - start != usize::from(layer.live_docs) || tokens != layer.live_tokens {
            return Err(wire::corrupt("split live layer summary"));
        }
    }
    let sort_bytes = total_docs
        .checked_mul(total_docs.next_power_of_two().trailing_zeros() as usize)
        .and_then(|n| n.checked_mul(size_of::<SplitDoc>()))
        .ok_or(IndexError::ResourceLimited)?;
    copy_work(context.budget, sort_bytes)?;
    directory.sort_unstable_by_key(|row| row.owner);
    if directory
        .windows(2)
        .any(|pair| pair[0].owner == pair[1].owner)
    {
        return Err(wire::corrupt("split duplicate live owner"));
    }
    let cut = directory[(directory.len() - 1) / 2].owner;
    let (mut left, left_memory) = funded_vec::<LayerRow>(layers.len(), context.budget)?;
    let (mut right, right_memory) = funded_vec::<LayerRow>(layers.len(), context.budget)?;
    for layer in layers {
        let mut empty = *layer;
        empty.live_mask = 0;
        empty.live_docs = 0;
        empty.live_tokens = 0;
        copy_work(context.budget, 2 * size_of::<LayerRow>())?;
        left.push(empty);
        right.push(empty);
    }
    for doc in &directory {
        context.budget.checkpoint()?;
        let target = if doc.owner <= cut {
            &mut left
        } else {
            &mut right
        };
        let layer = target
            .get_mut(usize::from(doc.layer))
            .ok_or(IndexError::ResourceLimited)?;
        layer.live_mask |= 1u128 << doc.slot;
        layer.live_docs = layer
            .live_docs
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        layer.live_tokens = layer
            .live_tokens
            .checked_add(doc.tokens)
            .ok_or(IndexError::ResourceLimited)?;
    }
    left.retain(|layer| layer.live_mask != 0);
    right.retain(|layer| layer.live_mask != 0);
    if left.is_empty() || right.is_empty() {
        return Err(wire::corrupt("split empty side"));
    }
    Ok((cut, left, left_memory, right, right_memory))
}

struct LocatedDoc {
    doc: Doc,
    layer: usize,
    old_slot: usize,
}

type Remap = [Option<u16>; wire::MAX_DOCS];

pub(super) fn compact(
    recipe: TextRecipe,
    layers: &[LayerRow],
    context: &mut AttemptContext<'_>,
) -> Result<SegmentEdge, IndexError> {
    let docs = layers.iter().try_fold(0usize, |total, layer| {
        total
            .checked_add(usize::from(layer.live_docs))
            .ok_or(IndexError::ResourceLimited)
    })?;
    if docs == 0 || docs > wire::MAX_DOCS {
        return Err(IndexError::ResourceLimited);
    }
    if let [layer] = layers
        && layer.live_docs == layer.physical_docs
    {
        return Err(IndexError::InvalidInput(
            "compaction requires multiple layers or dead documents".into(),
        ));
    }
    let (mut located, _doc_memory) = funded_vec(docs, context.budget)?;
    let (mut segments, _segment_memory) = funded_vec(layers.len(), context.budget)?;
    for (layer_index, layer) in layers.iter().enumerate() {
        context.budget.checkpoint()?;
        let segment = open_v2_segment(layer.reference, recipe, context.objects, context.budget)?;
        let descriptor = segment.descriptor();
        if descriptor
            != (SegmentEdge {
                reference: layer.reference,
                first: layer.first,
                last: layer.last,
                docs: layer.physical_docs,
                tokens: layer.physical_tokens,
            })
            || layer.live_mask.count_ones() != u32::from(layer.live_docs)
        {
            return Err(wire::corrupt("compaction layer identity or live mask"));
        }
        let start = located.len();
        let mut tokens = 0u64;
        for slot in 0..usize::from(descriptor.docs) {
            context.budget.checkpoint()?;
            let shift = u32::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?;
            let bit = 1u128
                .checked_shl(shift)
                .ok_or(IndexError::ResourceLimited)?;
            if layer.live_mask & bit == 0 {
                continue;
            }
            let doc = segment.doc(slot)?;
            tokens = tokens
                .checked_add(doc.tokens)
                .ok_or(IndexError::ResourceLimited)?;
            copy_work(context.budget, size_of::<LocatedDoc>())?;
            located.push(LocatedDoc {
                doc,
                layer: layer_index,
                old_slot: slot,
            });
        }
        if located.len().checked_sub(start) != Some(usize::from(layer.live_docs))
            || tokens != layer.live_tokens
        {
            return Err(wire::corrupt("compaction live summary"));
        }
        segments.push(segment);
    }

    // Sort the bounded document directory. The charged scratch cursor provides
    // canonical (term, remapped docID, position) order across overlapping layers.
    let levels = usize::try_from(docs.next_power_of_two().trailing_zeros())
        .map_err(|_error| IndexError::ResourceLimited)?;
    let sort_bytes = docs
        .checked_mul(levels)
        .and_then(|items| items.checked_mul(size_of::<LocatedDoc>()))
        .ok_or(IndexError::ResourceLimited)?;
    copy_work(context.budget, sort_bytes)?;
    located.sort_unstable_by_key(|row| row.doc.owner);
    if located
        .windows(2)
        .any(|pair| matches!(pair, [a, b] if a.doc.owner == b.doc.owner))
    {
        return Err(wire::corrupt("duplicate live owner in compaction"));
    }
    let (mut slot_by_layer, _remap_memory) = funded_vec::<Remap>(layers.len(), context.budget)?;
    let remap_bytes = layers
        .len()
        .checked_mul(size_of::<Remap>())
        .ok_or(IndexError::ResourceLimited)?;
    copy_work(context.budget, remap_bytes)?;
    slot_by_layer.resize(layers.len(), [None; wire::MAX_DOCS]);
    let mut window = Window::new(context.budget)?;
    let has_postings = located.iter().any(|row| row.doc.tokens != 0);
    if has_postings {
        window.enable_spill(context.scratch, context.budget)?;
    }
    for (output_slot, row) in located.iter().enumerate() {
        let local = u16::try_from(output_slot).map_err(|_error| IndexError::ResourceLimited)?;
        let slot = slot_by_layer
            .get_mut(row.layer)
            .and_then(|slots| slots.get_mut(row.old_slot))
            .ok_or_else(|| wire::corrupt("compaction document remap"))?;
        *slot = Some(local);
        copy_work(context.budget, size_of::<Doc>())?;
        window.docs.push(row.doc);
    }
    let scratch = context.scratch;
    for ((segment, layer), remap) in segments.iter().zip(layers).zip(&slot_by_layer) {
        replay_v2_postings(
            segment,
            layer.live_mask,
            context.objects,
            context.budget,
            &mut |key, old_slot, position, budget| {
                let new_slot = remap
                    .get(old_slot)
                    .copied()
                    .flatten()
                    .ok_or_else(|| wire::corrupt("compaction posting remap"))?;
                window.append_group(key, new_slot, std::iter::once(position), scratch, budget)
            },
        )?;
    }
    drop(segments);
    let (mut edges, _edge_memory) = funded_vec(1, context.budget)?;
    seal_observed(
        &mut window,
        recipe.segment_child_recipe().output_policy(),
        &mut edges,
        &mut None,
        context,
    )?;
    let [edge] = edges.as_slice() else {
        return Err(wire::corrupt("compaction produced no segment"));
    };
    let installed = open_v2_segment(edge.reference, recipe, context.objects, context.budget)?;
    if installed.descriptor() != *edge {
        return Err(wire::corrupt("compaction installed descriptor"));
    }
    verify_v2_segment(&installed, context.objects, context.budget)?;
    Ok(*edge)
}
