//! Initial-population owner-local occurrence packing.
//!
//! The existing occurrence batch remains the physical installer and the
//! inventory spool remains the only source of logical map edits. This module
//! changes the number of immutable payloads, not their custody protocol.

use quanta_index_core::index::{
    AttemptContext, IndexError, MapValue, MemoryCreditLimits, MemoryLifetime, ObjectRef,
    WorkBudget,
    ports::{MAX_OBJECT_INSTALL_BATCH, ObjectInstallMemoryShape},
};

use super::{
    ConfirmedOccurrences, InventoryEdits, InventoryPartition, OccurrenceEdit, OccurrenceWrites,
    PreparedKey, PreparedPositions, TextSpool, check_term_key,
    cold_tokens::{ColdBlockShape, ColdTokens},
    entry_patch::{
        COLD_ALIAS_MAX_TERMS, COLD_ALIAS_MIN_TERMS, COLD_INLINE_ANCHOR_KEY_BYTES, ColdAliasLayout,
        ColdDescriptorSeal, ColdInventoryAlias, ColdInventoryKeySeal,
    },
};
use crate::native::{
    text_memory::{PreparedPayload, PreparedRows},
    text_occurrence::block::{
        BlockDraft, BlockPush, BlockTerm, MAX_BLOCK_TERMS, PackedBlockView, SealedBlock,
        encode_packed_with_memory,
    },
    text_spool::MAX_INVENTORY_ROOTS,
};

#[derive(Clone, Copy)]
pub(super) enum ColdShapeBranch {
    Alias,
    Direct,
    Sql,
    Empty,
}

/// Diagnostic-only totals gathered during the selected output traversal.
/// Overflow drops the observation without changing the admitted operation.
#[derive(Default)]
pub(super) struct ColdShapeObservation {
    occurrences: u64,
    terms: u64,
    occurrence_key_bytes: u64,
    unique_key_bytes: u64,
    branch: Option<ColdShapeBranch>,
    overflowed: bool,
}

impl ColdShapeObservation {
    fn record_group(&mut self, key_bytes: usize, positions: usize) {
        if self.overflowed {
            return;
        }
        let next = (|| {
            let key = u64::try_from(key_bytes).ok()?;
            let count = u64::try_from(positions).ok()?;
            Some((
                self.occurrences.checked_add(count)?,
                self.terms.checked_add(1)?,
                self.occurrence_key_bytes
                    .checked_add(key.checked_mul(count)?)?,
                self.unique_key_bytes.checked_add(key)?,
            ))
        })();
        if let Some((occurrences, terms, occurrence_key_bytes, unique_key_bytes)) = next {
            self.occurrences = occurrences;
            self.terms = terms;
            self.occurrence_key_bytes = occurrence_key_bytes;
            self.unique_key_bytes = unique_key_bytes;
        } else {
            self.overflowed = true;
        }
    }

    fn finish(&mut self, selected: ColdShapeBranch, terms: u64) {
        if self.terms != terms {
            self.overflowed = true;
        }
        self.branch = Some(if terms == 0 {
            ColdShapeBranch::Empty
        } else {
            selected
        });
    }

    pub(super) fn complete(&self) -> Option<(u64, u64, u64, u64, ColdShapeBranch)> {
        (!self.overflowed).then_some((
            self.occurrences,
            self.terms,
            self.occurrence_key_bytes,
            self.unique_key_bytes,
            self.branch?,
        ))
    }
}

/// Maximum new caller-owned grants that may coexist during one cold owner's
/// canonical block/stage pass. Existing pending grants remain in their parent
/// authority; Object and Scratch provider grants are composed separately.
pub(super) fn owned_cold_memory_bound(
    shape: ColdBlockShape,
) -> Result<MemoryCreditLimits, IndexError> {
    let terms = shape.term_count.min(MAX_BLOCK_TERMS);
    let block_capacity = terms
        .checked_next_power_of_two()
        .ok_or(IndexError::ResourceLimited)?
        .clamp(8, MAX_BLOCK_TERMS);
    let block_rows = block_capacity
        .checked_add(block_capacity / 2)
        .and_then(|rows| {
            rows.checked_mul(
                std::mem::size_of::<BlockTerm>() + std::mem::size_of::<PreparedPositions>(),
            )
        })
        .ok_or(IndexError::ResourceLimited)?;
    let group_and_draft = shape
        .maximum_key_position_peak
        .checked_add(block_rows)
        .and_then(|bytes| bytes.checked_add(MAX_BLOCK_TERMS * std::mem::size_of::<u16>()))
        .and_then(|bytes| bytes.checked_add(super::super::text_occurrence::block::MAX_BLOCK_BYTES))
        .ok_or(IndexError::ResourceLimited)?;
    let packet_payloads = shape
        .total_payload_bytes
        .min(
            usize::try_from(super::MAX_OCCURRENCE_STAGED_BYTES)
                .map_err(|_error| IndexError::ResourceLimited)?,
        )
        .checked_add(shape.maximum_payload_bytes)
        .ok_or(IndexError::ResourceLimited)?;
    let write_rows = OccurrenceWrites::metadata_bytes(MAX_OBJECT_INSTALL_BATCH * 2)?;
    let inventory_rows = (quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS * 2)
        .checked_mul(std::mem::size_of::<(
            InventoryPartition,
            quanta_index_core::index::MapEdit,
        )>())
        .ok_or(IndexError::ResourceLimited)?;
    let inventory_keys = quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS
        .checked_mul(
            usize::try_from(super::MAX_TERM_KEY_BYTES)
                .map_err(|_error| IndexError::ResourceLimited)?,
        )
        .ok_or(IndexError::ResourceLimited)?;
    let confirmed = quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS
        .checked_mul(std::mem::size_of::<super::ConfirmedOccurrence>())
        .ok_or(IndexError::ResourceLimited)?;
    let temporary = group_and_draft
        .checked_add(packet_payloads)
        .and_then(|bytes| bytes.checked_add(usize::try_from(write_rows).ok()?))
        .and_then(|bytes| bytes.checked_add(inventory_rows))
        .and_then(|bytes| bytes.checked_add(inventory_keys))
        .and_then(|bytes| bytes.checked_add(confirmed))
        .ok_or(IndexError::ResourceLimited)?;
    // Each owned key and distinct position vector has one grant. Two grants
    // may overlap during one Vec growth; packet/row backing likewise overlaps.
    let object_slots = shape.object_count.min(MAX_OBJECT_INSTALL_BATCH);
    let temporary_slots = terms
        .checked_mul(2)
        .and_then(|slots| slots.checked_add(3)) // incoming key and grow-copy positions
        .and_then(|slots| slots.checked_add(5)) // two row growths and vector index
        .and_then(|slots| slots.checked_add(object_slots + 3)) // payload and packet growth
        .and_then(|slots| slots.checked_add(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS + 3))
        .ok_or(IndexError::ResourceLimited)?;
    Ok(MemoryCreditLimits {
        temporary_data_bytes: u64::try_from(temporary)
            .map_err(|_error| IndexError::ResourceLimited)?,
        retained_data_bytes: 0,
        max_live_temporary_leases: u32::try_from(temporary_slots)
            .map_err(|_error| IndexError::ResourceLimited)?,
        max_live_retained_leases: 0,
    })
}

pub(super) fn cold_object_install_shape(
    shape: ColdBlockShape,
    writes: &OccurrenceWrites<'_>,
) -> Result<Option<ObjectInstallMemoryShape>, IndexError> {
    let pending_inputs = writes.pending.len();
    let maximum_inputs = pending_inputs
        .checked_add(shape.object_count)
        .ok_or(IndexError::ResourceLimited)?
        .min(MAX_OBJECT_INSTALL_BATCH);
    if maximum_inputs == 0 {
        return Ok(None);
    }
    // `flush_before_input` splits before either physical packet limit. The
    // current pending packet can overlap the next cold block, while later
    // packets contain only cold blocks. Include the adopted run's retained
    // bytes, whose original `inputs` have already been released.
    let input_bytes = writes.inputs.iter().try_fold(0usize, |sum, input| {
        sum.checked_add(input.payload.len())
            .ok_or(IndexError::ResourceLimited)
    })?;
    let pending_bytes = input_bytes
        .max(usize::try_from(writes.retained_bytes).map_err(|_error| IndexError::ResourceLimited)?);
    let maximum_total_payload_bytes = pending_bytes
        .checked_add(shape.total_payload_bytes)
        .ok_or(IndexError::ResourceLimited)?
        .min(
            usize::try_from(super::MAX_OCCURRENCE_STAGED_BYTES)
                .map_err(|_error| IndexError::ResourceLimited)?,
        );
    let maximum_payload_bytes = writes
        .inputs
        .iter()
        .map(|input| input.payload.len())
        .max()
        .unwrap_or(0)
        .max(if writes.prepared_run.is_some() {
            // `try_start_run` admits only childless block payloads at this
            // fixed maximum before it releases the original input packet.
            pending_bytes.min(super::super::text_occurrence::block::MAX_BLOCK_BYTES)
        } else {
            0
        })
        .max(shape.maximum_payload_bytes);
    let packet = ObjectInstallMemoryShape {
        maximum_inputs,
        maximum_total_payload_bytes,
        maximum_payload_bytes,
        maximum_total_children: 0,
    };
    packet.validate()?;
    Ok(Some(packet))
}

pub(super) fn cold_output_credit_bound(
    shape: ColdBlockShape,
    context: &AttemptContext<'_>,
    spool: &TextSpool,
    writes: &OccurrenceWrites<'_>,
) -> Result<Option<MemoryCreditLimits>, IndexError> {
    let Some(packet) = cold_object_install_shape(shape, writes)? else {
        return Ok(None);
    };
    let Some(objects) = context.put_batch_memory_bound(packet)? else {
        return Ok(None);
    };
    let scratch = quanta_index_scratch_sqlite::map_edit_sql_memory_bound(
        "INSERT INTO inventory(root,k,p,n) VALUES ",
        quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS,
    )?;
    let local = owned_cold_memory_bound(shape)?
        .checked_add(spool.inventory_confirmation_memory_bound(packet.maximum_inputs)?)?;
    Ok(Some(local.checked_add(scratch)?.checked_add(objects)?))
}

struct PackTarget<'a, 'objects> {
    ordinal: u32,
    spool: &'a TextSpool,
    confirmed: &'a mut ConfirmedOccurrences,
    inventory_edits: &'a mut InventoryEdits,
    writes: &'a mut OccurrenceWrites<'objects>,
}

enum PackedSelectors {
    Terms {
        terms: PreparedRows<BlockTerm>,
        alias_only: bool,
    },
    AliasFirst {
        first: PreparedKey,
        term_count: usize,
    },
    Directory {
        terms: PreparedRows<BlockTerm>,
    },
}

impl PackedSelectors {
    fn len(&self) -> usize {
        match self {
            Self::Terms { terms, .. } | Self::Directory { terms } => terms.len(),
            Self::AliasFirst { term_count, .. } => *term_count,
        }
    }
}

/// Prepare one new owner's sorted token inventory. The caller must establish
/// an empty prior source and empty prior inventory before choosing this path.
pub(super) fn prepare_owner_blocks(
    ordinal: u32,
    context: &mut AttemptContext<'_>,
    spool: &TextSpool,
    cold_tokens: &mut Option<ColdTokens>,
    confirmed: &mut ConfirmedOccurrences,
    inventory_edits: &mut InventoryEdits,
    writes: &mut OccurrenceWrites<'_>,
    mut shape: Option<&mut ColdShapeObservation>,
) -> Result<(u64, Option<ColdInventoryAlias>), IndexError> {
    context.budget.checkpoint()?;
    if inventory_edits.ordinal != ordinal {
        return Err(IndexError::BackendViolation(
            "native packed inventory ordinal changed during preparation".into(),
        ));
    }
    let mut target = PackTarget {
        ordinal,
        spool,
        confirmed,
        inventory_edits,
        writes,
    };
    let mut use_sql_after_optional_denial = false;
    let mut tentative_alias_shape = shape.is_some().then(ColdShapeObservation::default);
    let prepared_alias = if let Some(tokens) = cold_tokens.as_ref() {
        if let Some(borrowed) = tokens.try_packed_alias(context.budget)? {
            let first_bytes = borrowed.first_key()?;
            let first_memory = context.budget.try_reserve_memory_for_optimization(
                u64::try_from(first_bytes.len()).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?;
            if let Some(first_memory) = first_memory {
                let payload_memory = context.budget.try_reserve_memory_for_optimization(
                    u64::try_from(borrowed.encoded_len())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    MemoryLifetime::Temporary,
                )?;
                if let Some(payload_memory) = payload_memory {
                    let mut seal = ColdInventoryKeySeal::new();
                    for ordinal in 0..borrowed.term_count() {
                        let (key, vector) = borrowed.term(ordinal)?;
                        seal.push(key, context.budget)?;
                        if let Some(shape) = tentative_alias_shape.as_mut() {
                            if let Ok(positions) = borrowed.vector_len(usize::from(vector)) {
                                shape.record_group(key.len(), positions);
                            } else {
                                shape.overflowed = true;
                            }
                        }
                    }
                    let first = PreparedKey::copy_with_memory(
                        first_bytes,
                        Some(first_memory),
                        context.budget,
                    )?;
                    if let Some(first) = first {
                        let payload = encode_packed_with_memory(
                            &borrowed,
                            borrowed.encoded_len(),
                            Some(payload_memory),
                            context.budget,
                        )?;
                        if let Some(payload) = payload {
                            Some((
                                first,
                                PreparedPayload::from_staged(payload)?,
                                borrowed.term_count(),
                                seal.finish(),
                            ))
                        } else {
                            use_sql_after_optional_denial = true;
                            None
                        }
                    } else {
                        use_sql_after_optional_denial = true;
                        None
                    }
                } else {
                    use_sql_after_optional_denial = true;
                    None
                }
            } else {
                use_sql_after_optional_denial = true;
                None
            }
        } else {
            None
        }
    } else {
        None
    };
    if let Some((first, payload, term_count, key_digest)) = prepared_alias {
        // The canonical payload and selector now own their exact grants. The
        // optional source arena cannot compete with installation metadata.
        *cold_tokens = None;
        let terms = u64::try_from(term_count).map_err(|_error| IndexError::ResourceLimited)?;
        let reference = target.stage_packed_selected(
            payload,
            PackedSelectors::AliasFirst { first, term_count },
            context,
        )?;
        if let (Some(shape), Some(tentative)) = (shape.as_deref_mut(), tentative_alias_shape.take())
        {
            *shape = tentative;
            shape.finish(ColdShapeBranch::Alias, terms);
        }
        return Ok((
            terms,
            Some(ColdInventoryAlias {
                reference,
                terms,
                key_digest,
                anchors: 1,
                descriptor_digest: ColdDescriptorSeal::new().finish(),
                layout: ColdAliasLayout::Inline,
                attempt: context.attempt,
            }),
        ));
    }
    if !use_sql_after_optional_denial {
        if let Some(tokens) = cold_tokens.as_ref() {
            let block_shape = tokens.block_shape(context.budget)?;
            if let Some(limits) =
                cold_output_credit_bound(block_shape, context, spool, &*target.writes)?
            {
                let attempt = context.attempt;
                let objects = context.objects;
                let maps = context.maps;
                let custody = context.custody;
                let scratch = context.scratch;
                if let Some(mut credit) = context.budget.try_scoped_memory_credit(limits)? {
                    let mut scoped_context = AttemptContext {
                        attempt,
                        objects,
                        maps,
                        custody,
                        scratch,
                        budget: &mut credit,
                    };
                    let multiblock = block_shape.object_count > 1
                        && block_shape.term_count
                            > usize::try_from(COLD_ALIAS_MAX_TERMS)
                                .map_err(|_error| IndexError::ResourceLimited)?
                        && block_shape.maximum_key_bytes
                            <= usize::try_from(super::MAX_TERM_KEY_BYTES)
                                .map_err(|_error| IndexError::ResourceLimited)?;
                    let result = run_owner_blocks(
                        &mut target,
                        cold_tokens,
                        &mut scoped_context,
                        multiblock,
                        shape.as_deref_mut(),
                    )?;
                    if let Some(shape) = shape.as_deref_mut() {
                        shape.finish(ColdShapeBranch::Direct, result.0);
                    }
                    return Ok(result);
                }
            }
            use_sql_after_optional_denial = true;
        }
    }
    if use_sql_after_optional_denial {
        let tokens = cold_tokens.as_ref().ok_or_else(|| {
            IndexError::BackendViolation("cold SQL replay lost its validated arena".into())
        })?;
        // The fixed SQL statement and borrowed rows need no second arena or
        // Original grant. After the disposable reset, any failure is terminal.
        spool.reset_tokens(context.budget)?;
        tokens.replay_borrowed_pages(context.budget, |rows, budget| {
            spool.token_batch_borrowed(rows, budget)
        })?;
        *cold_tokens = None;
    }
    let result = run_owner_blocks(
        &mut target,
        cold_tokens,
        context,
        false,
        shape.as_deref_mut(),
    )?;
    if let Some(shape) = shape.as_deref_mut() {
        shape.finish(ColdShapeBranch::Sql, result.0);
    }
    Ok(result)
}

fn run_owner_blocks(
    target: &mut PackTarget<'_, '_>,
    cold_tokens: &Option<ColdTokens>,
    context: &mut AttemptContext<'_>,
    multiblock: bool,
    mut shape: Option<&mut ColdShapeObservation>,
) -> Result<(u64, Option<ColdInventoryAlias>), IndexError> {
    let spool = target.spool;
    let mut draft = BlockDraft::new()?;
    let mut term_count = 0_u64;
    // A cold alias is valid only for the entire ordered key stream of one
    // packed block. Once a term cannot fit, no prefix witness is usable.
    let mut key_seal = Some(ColdInventoryKeySeal::new());
    let mut descriptors = ColdDescriptorSeal::new();
    let mut anchors = 0_u64;
    let mut first_reference = None;
    let attempt = context.attempt;
    let objects = context.objects;
    let maps = context.maps;
    let custody = context.custody;
    let scratch = context.scratch;
    let mut consume = |term: PreparedKey, positions: PreparedPositions, budget: &mut WorkBudget| {
        if let Some(shape) = shape.as_deref_mut() {
            shape.record_group(term.as_slice().len(), positions.as_slice().len());
        }
        let mut group_context = AttemptContext {
            attempt,
            objects,
            maps,
            custody,
            scratch,
            budget,
        };
        term_count = term_count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        check_term_key(term.as_slice())?;
        if positions.as_slice().is_empty() {
            return Err(IndexError::BackendViolation(
                "native text token term has no positions".into(),
            ));
        }
        if let Some(seal) = key_seal.as_mut() {
            if multiblock || term_count <= COLD_ALIAS_MAX_TERMS {
                seal.push(term.as_slice(), group_context.budget)?;
            } else {
                key_seal = None;
            }
        }
        match draft.try_push(term, positions, group_context.budget)? {
            BlockPush::Added => Ok(()),
            BlockPush::Full { term, positions } => {
                if !multiblock {
                    key_seal = None;
                }
                if draft.len() != 0 {
                    let sealed = std::mem::replace(&mut draft, BlockDraft::new()?)
                        .finish(group_context.budget)?;
                    if multiblock {
                        let reference = target.stage_sealed_alias(
                            sealed,
                            &mut descriptors,
                            &mut group_context,
                        )?;
                        let _first = first_reference.get_or_insert(reference);
                        anchors = anchors.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                    } else {
                        target.stage_sealed(sealed, &mut group_context)?;
                    }
                }
                // Full returns the original owned input. A term too large
                // for an empty block follows the canonical leaf path.
                match draft.try_push(term, positions, group_context.budget)? {
                    BlockPush::Added => Ok(()),
                    BlockPush::Full { term, positions } => {
                        if multiblock {
                            let reference = target.stage_leaf_alias(
                                term,
                                positions,
                                &mut descriptors,
                                &mut group_context,
                            )?;
                            let _first = first_reference.get_or_insert(reference);
                            anchors = anchors.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                            Ok(())
                        } else {
                            target
                                .stage_leaf(term, positions, &mut group_context)
                                .map(|_| ())
                        }
                    }
                }
            }
        }
    };
    let limit = usize::try_from(super::MAX_POSITIONS_PER_TERM)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if let Some(tokens) = cold_tokens.as_ref() {
        tokens.visit_groups(limit, context.budget, &mut consume)?;
    } else {
        spool.visit_token_groups(limit, context.budget, &mut consume)?;
    }
    if draft.len() == 0 {
        if multiblock {
            let seal = key_seal.ok_or_else(|| {
                IndexError::BackendViolation("cold multi-block source seal absent".into())
            })?;
            return Ok((
                term_count,
                Some(ColdInventoryAlias {
                    reference: first_reference.ok_or(IndexError::ResourceLimited)?,
                    terms: term_count,
                    key_digest: seal.finish(),
                    anchors,
                    descriptor_digest: descriptors.finish(),
                    layout: ColdAliasLayout::Directory,
                    attempt: context.attempt,
                }),
            ));
        }
        return Ok((term_count, None));
    }
    let sealed = draft.finish(context.budget)?;
    if multiblock {
        let reference = target.stage_sealed_alias(sealed, &mut descriptors, context)?;
        let _first = first_reference.get_or_insert(reference);
        anchors = anchors.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        let seal = key_seal.ok_or_else(|| {
            IndexError::BackendViolation("cold multi-block source seal absent".into())
        })?;
        return Ok((
            term_count,
            Some(ColdInventoryAlias {
                reference: first_reference.ok_or(IndexError::ResourceLimited)?,
                terms: term_count,
                key_digest: seal.finish(),
                anchors,
                descriptor_digest: descriptors.finish(),
                layout: ColdAliasLayout::Directory,
                attempt: context.attempt,
            }),
        ));
    }
    match (key_seal, sealed) {
        (Some(seal), SealedBlock::Packed { payload, terms }) => {
            let count = u64::try_from(terms.len()).map_err(|_error| IndexError::ResourceLimited)?;
            if count != term_count {
                return Err(IndexError::BackendViolation(
                    "cold inventory key seal term count differs from packed block".into(),
                ));
            }
            if (COLD_ALIAS_MIN_TERMS..=COLD_ALIAS_MAX_TERMS).contains(&count) {
                let first = terms
                    .as_slice()
                    .first()
                    .ok_or(IndexError::ResourceLimited)?;
                let layout = if first.key.as_slice().len() > COLD_INLINE_ANCHOR_KEY_BYTES {
                    ColdAliasLayout::Directory
                } else {
                    ColdAliasLayout::Inline
                };
                let (reference, descriptor_digest) = if layout == ColdAliasLayout::Directory {
                    let mut descriptors = ColdDescriptorSeal::new();
                    let reference = target.stage_sealed_alias(
                        SealedBlock::Packed { payload, terms },
                        &mut descriptors,
                        context,
                    )?;
                    (reference, descriptors.finish())
                } else {
                    let reference = target.stage_packed(
                        PreparedPayload::from_staged(payload)?,
                        terms,
                        context,
                        true,
                    )?;
                    (reference, ColdDescriptorSeal::new().finish())
                };
                return Ok((
                    term_count,
                    Some(ColdInventoryAlias {
                        reference,
                        terms: term_count,
                        key_digest: seal.finish(),
                        anchors: 1,
                        descriptor_digest,
                        layout,
                        attempt: context.attempt,
                    }),
                ));
            }
            target.stage_sealed(SealedBlock::Packed { payload, terms }, context)?;
        }
        (_, sealed) => target.stage_sealed(sealed, context)?,
    }
    Ok((term_count, None))
}

impl PackTarget<'_, '_> {
    fn stage_sealed_alias(
        &mut self,
        sealed: SealedBlock,
        descriptors: &mut ColdDescriptorSeal,
        context: &mut AttemptContext<'_>,
    ) -> Result<ObjectRef, IndexError> {
        match sealed {
            SealedBlock::Packed { payload, terms } => {
                let fence = &terms
                    .as_slice()
                    .last()
                    .ok_or(IndexError::ResourceLimited)?
                    .key;
                descriptors.begin(fence.as_slice(), 1, terms.len(), context.budget)?;
                let reference = self.stage_packed_selected(
                    PreparedPayload::from_staged(payload)?,
                    PackedSelectors::Directory { terms },
                    context,
                )?;
                descriptors.reference(reference);
                Ok(reference)
            }
            SealedBlock::Leaf { term, positions } => {
                self.stage_leaf_alias(term, positions, descriptors, context)
            }
        }
    }

    fn stage_leaf_alias(
        &mut self,
        term: PreparedKey,
        positions: PreparedPositions,
        descriptors: &mut ColdDescriptorSeal,
        context: &mut AttemptContext<'_>,
    ) -> Result<ObjectRef, IndexError> {
        descriptors.begin(term.as_slice(), 2, 1, context.budget)?;
        let reference = self.stage_leaf_selected(term, positions, context, true)?;
        descriptors.reference(reference);
        Ok(reference)
    }

    fn stage_sealed(
        &mut self,
        sealed: SealedBlock,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        match sealed {
            SealedBlock::Leaf { term, positions } => self
                .stage_leaf(term, positions, context)
                .map(|_reference| ()),
            SealedBlock::Packed { payload, terms } => self
                .stage_packed(
                    PreparedPayload::from_staged(payload)?,
                    terms,
                    context,
                    false,
                )
                .map(|_reference| ()),
        }
    }

    fn stage_leaf(
        &mut self,
        term: PreparedKey,
        positions: PreparedPositions,
        context: &mut AttemptContext<'_>,
    ) -> Result<ObjectRef, IndexError> {
        self.stage_leaf_selected(term, positions, context, false)
    }

    fn stage_leaf_selected(
        &mut self,
        term: PreparedKey,
        positions: PreparedPositions,
        context: &mut AttemptContext<'_>,
        directory: bool,
    ) -> Result<ObjectRef, IndexError> {
        let payload = crate::native::text_memory::encode(&positions.occurrence, context.budget)?;
        drop(positions);
        let reference = context
            .objects
            .identify(payload.as_slice(), &[], context.budget)?
            .reference;
        let value = if directory {
            super::entry_map::directory::descriptor(2, 1, reference)?
        } else {
            reference.into()
        };
        self.stage_identified(term, reference, payload, value, context)?;
        Ok(reference)
    }

    fn stage_identified(
        &mut self,
        term: PreparedKey,
        reference: ObjectRef,
        payload: PreparedPayload,
        value: MapValue,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        let pending = self
            .writes
            .pending
            .iter()
            .any(|row| row.identified == reference);
        if pending {
            self.confirmed.check_attempt(context.attempt)?;
        }
        if !pending
            && self.confirmed.contains_or_spill(
                reference,
                context.attempt,
                self.spool,
                context.budget,
            )?
        {
            self.push_ready_value(term, value, context.budget)?;
            return Ok(());
        }
        self.writes.flush_before_input(
            reference,
            payload.as_slice().len(),
            context,
            self.spool,
            self.confirmed,
            self.inventory_edits,
        )?;
        let _slot = self.writes.push_with_value(
            OccurrenceEdit {
                ordinal: self.ordinal,
                key: term,
                prior: None,
                identified: reference,
            },
            payload,
            value,
            self.spool,
            self.inventory_edits,
            context.budget,
        )?;
        self.flush_full(context)
    }

    fn stage_packed(
        &mut self,
        payload: PreparedPayload,
        terms: PreparedRows<BlockTerm>,
        context: &mut AttemptContext<'_>,
        alias_only: bool,
    ) -> Result<ObjectRef, IndexError> {
        self.stage_packed_selected(
            payload,
            PackedSelectors::Terms { terms, alias_only },
            context,
        )
    }

    fn stage_packed_selected(
        &mut self,
        payload: PreparedPayload,
        selectors: PackedSelectors,
        context: &mut AttemptContext<'_>,
    ) -> Result<ObjectRef, IndexError> {
        context.budget.checkpoint()?;
        if !(2..=MAX_BLOCK_TERMS).contains(&selectors.len()) {
            return Err(IndexError::BackendViolation(
                "native packed occurrence has invalid term count".into(),
            ));
        }
        // Only the content identity survives. Release its transient metadata
        // before flushing the prior packet's independently admitted outputs.
        let reference = context
            .objects
            .identify(payload.as_slice(), &[], context.budget)?
            .reference;
        let pending = self
            .writes
            .pending
            .iter()
            .any(|row| row.identified == reference);
        if pending {
            self.confirmed.check_attempt(context.attempt)?;
        }
        let ready = !pending
            && self.confirmed.contains_or_spill(
                reference,
                context.attempt,
                self.spool,
                context.budget,
            )?;
        if !ready {
            self.writes.flush_before_input(
                reference,
                payload.as_slice().len(),
                context,
                self.spool,
                self.confirmed,
                self.inventory_edits,
            )?;
        }
        // Only the fallback emits the remaining selectors in original order.
        let (first, mut remaining, alias_only, value) = match selectors {
            PackedSelectors::Terms {
                mut terms,
                alias_only,
            } => {
                if alias_only {
                    let last = terms
                        .len()
                        .checked_sub(1)
                        .ok_or(IndexError::ResourceLimited)?;
                    terms.as_mut_slice().swap(0, last);
                } else {
                    terms.as_mut_slice().reverse();
                }
                let first = terms.pop().ok_or(IndexError::ResourceLimited)?;
                (first.key, Some(terms), alias_only, reference.into())
            }
            PackedSelectors::AliasFirst { first, .. } => (first, None, true, reference.into()),
            PackedSelectors::Directory { mut terms } => {
                let value = super::entry_map::directory::descriptor(1, terms.len(), reference)?;
                let fence = terms.pop().ok_or(IndexError::ResourceLimited)?.key;
                (fence, None, true, value)
            }
        };
        if ready {
            self.push_ready_value(first, value, context.budget)?;
            if !alias_only && let Some(terms) = remaining.as_mut() {
                while let Some(term) = terms.pop() {
                    self.push_ready(term.key, reference, context.budget)?;
                }
            }
            return Ok(reference);
        }
        let slot = self.writes.push_with_value(
            OccurrenceEdit {
                ordinal: self.ordinal,
                key: first,
                prior: None,
                identified: reference,
            },
            payload,
            value,
            self.spool,
            self.inventory_edits,
            context.budget,
        )?;
        if !alias_only && let Some(terms) = remaining.as_mut() {
            while let Some(term) = terms.pop() {
                self.writes.push_block_alias(
                    self.ordinal,
                    term.key,
                    slot,
                    self.spool,
                    self.inventory_edits,
                    context.budget,
                )?;
            }
        }
        self.flush_full(context)?;
        Ok(reference)
    }

    fn push_ready(
        &mut self,
        term: PreparedKey,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.push_ready_value(term, reference.into(), budget)
    }

    fn push_ready_value(
        &mut self,
        term: PreparedKey,
        value: MapValue,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.inventory_edits.push_partition(
            InventoryPartition::Ready(self.ordinal),
            term,
            None,
            Some(value),
            budget,
        )?;
        if self.inventory_edits.rows.len() == quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
            self.inventory_edits.flush(self.spool, budget)?;
        }
        Ok(())
    }

    fn flush_full(&mut self, context: &mut AttemptContext<'_>) -> Result<(), IndexError> {
        if self.writes.pending.len() == MAX_OBJECT_INSTALL_BATCH {
            self.writes
                .flush(context, self.spool, self.confirmed, self.inventory_edits)?;
        }
        Ok(())
    }
}

impl OccurrenceWrites<'_> {
    /// Add another logical selector for a pending block without another
    /// physical object input. The first term was checked by `push` above.
    pub(super) fn push_block_alias(
        &mut self,
        ordinal: u32,
        term: PreparedKey,
        slot: super::PendingOccurrenceSlot,
        spool: &TextSpool,
        inventory_edits: &mut InventoryEdits,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if usize::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?
            >= MAX_INVENTORY_ROOTS
        {
            return Err(IndexError::BackendViolation(
                "occurrence ordinal exceeds its inventory window".into(),
            ));
        }
        check_term_key(term.as_slice())?;
        let fanout_rows = self
            .fanout_rows
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let pending = self
            .pending
            .get(slot.index)
            .filter(|row| row.identified == slot.identified)
            .ok_or_else(|| {
                IndexError::BackendViolation(
                    "packed occurrence alias has no pending physical input".into(),
                )
            })?;
        let reference = pending.identified;
        inventory_edits.push_partition(
            InventoryPartition::Planned(ordinal),
            term,
            None,
            Some(reference.into()),
            budget,
        )?;
        if inventory_edits.rows.len() == quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
            inventory_edits.flush(spool, budget)?;
        }
        let pending = self
            .pending
            .get_mut(slot.index)
            .ok_or(IndexError::ResourceLimited)?;
        pending.ordinal = pending.ordinal.max(ordinal);
        self.fanout_rows = fanout_rows;
        Ok(())
    }
}
