//! File-local chunk provider, authenticated membership and exact lookup.

use super::*;

/// Fund the additional authenticated join, replayed text hashing and copied
/// coordinates before the request-only provider adapter performs that work.
pub(super) fn charge_chunk_reconciliation(
    payload_bytes: u64,
    old_count: usize,
    new_count: usize,
    dimension: u32,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let count = old_count
        .checked_add(new_count)
        .ok_or(IndexError::ResourceLimited)?;
    let levels = usize::BITS
        .checked_sub(count.saturating_sub(1).leading_zeros())
        .ok_or(IndexError::ResourceLimited)?;
    let compare_bytes = u64::try_from(count)
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_mul(
            u64::from(levels)
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?,
        )
        // New-plan sort, old-index sort, two ID joins and final result sort.
        .and_then(|bytes| bytes.checked_mul(5 * 32))
        .ok_or(IndexError::ResourceLimited)?;
    let coordinate_bytes = u64::try_from(new_count)
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_mul(u64::from(dimension))
        .and_then(|cells| cells.checked_mul(4))
        .ok_or(IndexError::ResourceLimited)?;
    let bytes = payload_bytes
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(compare_bytes))
        .and_then(|bytes| bytes.checked_add(coordinate_bytes))
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes,
        mutations: u64::try_from(count).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })
}

impl<P: TextEmbeddingProvider> TextEmbeddingBridge<P> {
    pub(super) fn embed_chunks(
        &self,
        input: &EmbeddingInput<'_>,
        policy: Utf8ChunkPolicy,
    ) -> Result<EmbeddingOutput, IndexError> {
        let plan = plan_chunks(
            input.source_bytes,
            policy,
            input.recipe.max_vectors,
            input.request_budget,
        )?;
        let count = plan.members.len();
        let text_bytes = count
            .checked_mul(std::mem::size_of::<&str>())
            .ok_or(IndexError::ResourceLimited)?;
        let text_bytes = u64::try_from(text_bytes).map_err(|_error| IndexError::ResourceLimited)?;
        let _text_memory = input
            .request_budget
            .reserve_provider_memory_v1(text_bytes, MemoryLifetime::Temporary)
            .map_err(|error| provider_error(&error, input.request_budget))?;
        let mut texts = Vec::new();
        texts
            .try_reserve_exact(count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        for member in &plan.members {
            let (start, end) = member.payload_span()?;
            let start = usize::try_from(start).map_err(|_error| IndexError::ResourceLimited)?;
            let end = usize::try_from(end).map_err(|_error| IndexError::ResourceLimited)?;
            let bytes = input.source_bytes.get(start..end).ok_or_else(|| {
                IndexError::BackendViolation("chunk planner returned invalid span".into())
            })?;
            let text = std::str::from_utf8(bytes).map_err(|_error| {
                IndexError::BackendViolation("chunk planner split a UTF-8 scalar".into())
            })?;
            texts.push(text);
        }
        self.embed_member_batch(input, &plan.members, &texts)
    }

    pub(super) fn embed_certified_chunks(
        &self,
        input: &EmbeddingInput<'_>,
    ) -> Result<EmbeddingOutput, IndexError> {
        let plan = certified_chunks::plan_within_request(
            input.source_bytes,
            input.owner,
            input.binding,
            input.recipe,
            input.request_budget,
        )?;
        let manifest = certified_chunks::select_manifest(
            input.source_bytes,
            input.owner,
            input.binding,
            input.recipe,
        )?;
        let count = manifest.len();
        let text_bytes = count
            .checked_mul(std::mem::size_of::<&str>())
            .ok_or(IndexError::ResourceLimited)?;
        let _text_memory = input
            .request_budget
            .reserve_provider_memory_v1(
                u64::try_from(text_bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )
            .map_err(|error| provider_error(&error, input.request_budget))?;
        let mut texts = Vec::new();
        texts
            .try_reserve_exact(count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        for member in manifest.iter() {
            budget_checkpoint(input.request_budget)?;
            let member = member.map_err(|error| {
                IndexError::InvalidInput(format!("semantic manifest member: {error}"))
            })?;
            texts.push(member.text);
        }
        self.embed_member_batch(input, &plan.members, &texts)
    }

    fn embed_member_batch(
        &self,
        input: &EmbeddingInput<'_>,
        members: &[ChunkMember],
        texts: &[&str],
    ) -> Result<EmbeddingOutput, IndexError> {
        let count = members.len();
        if count != texts.len() {
            return Err(IndexError::BackendViolation(
                "semantic texts differ from selected membership".into(),
            ));
        }
        let vector_bytes = std::mem::size_of::<EmbeddingVector>()
            .checked_add(32)
            .ok_or(IndexError::ResourceLimited)?;
        let capacity_bytes = count
            .checked_mul(vector_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        let capacity_bytes =
            u64::try_from(capacity_bytes).map_err(|_error| IndexError::ResourceLimited)?;
        let capacity_memory = input
            .request_budget
            .reserve_provider_memory_v1(capacity_bytes, MemoryLifetime::Retained)
            .map_err(|error| provider_error(&error, input.request_budget))?;
        // The established text-provider contract is positional. It cannot
        // certify a maliciously reordered coordinate batch without keyed output.
        let output = self
            .provider
            .embed_batch_within(texts, input.request_budget)
            .map_err(|error| provider_error(&error, input.request_budget))?;
        budget_checkpoint(input.request_budget)?;
        self.verify_descriptor()?;
        if output.len() != count {
            return Err(IndexError::BackendViolation(
                "text provider chunk cardinality differs from selected input".into(),
            ));
        }
        let coordinate_parts = output.into_parts();
        let coordinate_memory = coordinate_parts.1;
        let coordinates = coordinate_parts.0;
        let mut vectors = Vec::new();
        vectors
            .try_reserve_exact(count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        for (member, values) in members.iter().zip(coordinates) {
            budget_checkpoint(input.request_budget)?;
            validate_bridge_vector(&values, self.dimension, self.normalization)?;
            let mut id = Vec::new();
            id.try_reserve_exact(32)
                .map_err(|_error| IndexError::ResourceLimited)?;
            id.extend_from_slice(&member.id.0);
            vectors.push(EmbeddingVector { id, values });
        }
        // Persisted vectors are ID ordered; membership retains input order.
        vectors.sort_unstable_by(|left, right| left.id.cmp(&right.id));
        Ok(EmbeddingOutput::from_parts(
            vectors,
            [coordinate_memory, capacity_memory],
        ))
    }
}

pub(super) struct ChunkLookup {
    indices: Vec<(Digest, usize)>,
    _memory: Option<MemoryLease>,
}

impl ChunkLookup {
    pub(super) fn new(
        members: &[ChunkMember],
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        if members.is_empty() {
            return Ok(Self {
                indices: Vec::new(),
                _memory: None,
            });
        }
        let bytes = members
            .len()
            .checked_mul(std::mem::size_of::<(Digest, usize)>())
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        let mut lookup = Self {
            indices: Vec::new(),
            _memory: budget.reserve_memory(bytes, MemoryLifetime::Temporary)?,
        };
        lookup
            .indices
            .try_reserve_exact(members.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        lookup.indices.extend(
            members
                .iter()
                .enumerate()
                .map(|(index, member)| (member.id, index)),
        );
        lookup.indices.sort_unstable_by_key(|(id, _index)| *id);
        budget.checkpoint()?;
        Ok(lookup)
    }

    pub(super) fn find(
        &self,
        members: &[ChunkMember],
        id: &[u8],
    ) -> Result<Option<ChunkMember>, IndexError> {
        if members.is_empty() {
            return Ok(None);
        }
        let position = self
            .indices
            .binary_search_by(|(member_id, _index)| member_id.0.as_slice().cmp(id))
            .map_err(|_error| {
                IndexError::CorruptObject("selected vector lacks chunk membership".into())
            })?;
        let member = self
            .indices
            .get(position)
            .and_then(|(_id, index)| members.get(*index))
            .copied()
            .ok_or_else(|| {
                IndexError::BackendViolation("chunk lookup index exceeds admitted backing".into())
            })?;
        Ok(Some(member))
    }
}

pub(super) fn prepare_chunk_members(
    payload: &[u8],
    vectors: &[EmbeddingVector],
    recipe: &EmbeddingRecipe,
    budget: &mut WorkBudget,
) -> Result<(Vec<ChunkMember>, Option<MemoryLease>), IndexError> {
    let Some(policy) = recipe.chunk_policy else {
        return Ok((Vec::new(), None));
    };
    let plan = plan_chunks_within_work(payload, policy, recipe.max_vectors, budget)?;
    validate_chunk_bijection(&plan.members, vectors)?;
    Ok((plan.members, plan.memory))
}

pub(super) fn validate_chunk_bijection(
    members: &[ChunkMember],
    vectors: &[EmbeddingVector],
) -> Result<(), IndexError> {
    if members.len() != vectors.len()
        || vectors.len()
            > usize::try_from(MAX_VECTORS).map_err(|_error| IndexError::ResourceLimited)?
    {
        return Err(IndexError::BackendViolation(
            "semantic chunk membership differs from provider vectors".into(),
        ));
    }
    // Fixed 512-byte stack bitset: no per-member allocation. Equal counts
    // and distinct matched vector positions prove both sides of the bijection.
    let mut selected = [0u64; 64];
    for member in members {
        let position = vectors
            .binary_search_by(|vector| vector.id.as_slice().cmp(member.id.0.as_slice()))
            .map_err(|_error| {
                IndexError::BackendViolation("semantic chunk has no provider vector".into())
            })?;
        let shift = u32::try_from(position & 63).map_err(|_error| IndexError::ResourceLimited)?;
        let mask = 1u64.checked_shl(shift).ok_or(IndexError::ResourceLimited)?;
        let word = selected
            .get_mut(position >> 6)
            .ok_or(IndexError::ResourceLimited)?;
        if *word & mask != 0 {
            return Err(IndexError::BackendViolation(
                "semantic chunk membership repeats a logical vector".into(),
            ));
        }
        *word |= mask;
    }
    Ok(())
}

pub(super) fn validate_chunk_artifact(
    objects: &dyn ImmutableObjectPort,
    artifact: &OwnerArtifact,
    recipe: &EmbeddingRecipe,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if !recipe.uses_chunks() {
        return if artifact.chunks.is_empty() {
            Ok(())
        } else {
            Err(IndexError::CorruptObject(
                "non-chunk recipe has chunk membership".into(),
            ))
        };
    }
    let payload = objects.get(artifact.payload_ref, budget)?;
    if payload.registration.reference != artifact.payload_ref
        || !payload.registration.children.is_empty()
        || u64::try_from(payload.payload.len()).map_err(|_error| IndexError::ResourceLimited)?
            > recipe.max_source_bytes
    {
        return Err(IndexError::CorruptObject(
            "chunk input payload binding or bounds mismatch".into(),
        ));
    }
    // Source and certified projection retain distinct authorities. A member
    // cannot name a different valid UTF-8 leaf under an unchanged owner stamp.
    match recipe.input_policy {
        EmbeddingInputPolicy::SourceUtf8 => {
            if artifact.input.projection.is_some()
                || artifact.input.producer_context != Digest([0; 32])
            {
                return Err(IndexError::CorruptObject(
                    "chunk source policy mismatch".into(),
                ));
            }
            let source_ref = artifact
                .input
                .source
                .ok_or_else(|| IndexError::CorruptObject("chunk source record missing".into()))?;
            let source = load_source_record_from_objects(source_ref, objects, budget)?;
            if source.bytes != artifact.payload_ref
                || source.byte_len
                    != u64::try_from(payload.payload.len())
                        .map_err(|_error| IndexError::ResourceLimited)?
            {
                return Err(IndexError::CorruptObject(
                    "chunk source payload mismatch".into(),
                ));
            }
        }
        EmbeddingInputPolicy::ProjectionUtf8 {
            projection,
            producer,
            ..
        }
        | EmbeddingInputPolicy::ProjectionChunks {
            projection,
            producer,
            ..
        } => {
            let reference = artifact.input.projection.ok_or_else(|| {
                IndexError::CorruptObject("chunk projection contribution missing".into())
            })?;
            let contribution = load_contribution_from_objects(reference, objects, budget)?;
            if contribution.projection != projection
                || contribution.producer != producer
                || contribution.owner != artifact.owner
                || contribution.source != artifact.input.source
                || contribution.source_owner.is_some() != contribution.source.is_some()
                || contribution_context(contribution) != artifact.input.producer_context
                || contribution.output != Some(artifact.payload_ref)
            {
                return Err(IndexError::CorruptObject(
                    "chunk projection stamp mismatch".into(),
                ));
            }
        }
    }
    let validated = (|| {
        if recipe.is_certified_chunks() {
            let plan = certified_chunks::plan_within_work(
                &payload.payload,
                artifact.owner,
                artifact.input,
                recipe,
                budget,
            )?;
            validate_chunk_bijection(&plan.members, &artifact.vectors)?;
            Ok(plan)
        } else {
            let (members, memory) =
                prepare_chunk_members(&payload.payload, &artifact.vectors, recipe, budget)?;
            Ok(crate::native::chunks::ChunkPlan { members, memory })
        }
    })()
    .map_err(|error| match error {
        IndexError::BackendViolation(message) | IndexError::InvalidInput(message) => {
            IndexError::CorruptObject(message)
        }
        other @ (IndexError::Conflict
        | IndexError::DigestConflict
        | IndexError::FenceLost
        | IndexError::WorkFenceLost
        | IndexError::ResourceLimited
        | IndexError::WorkLimitExceeded
        | IndexError::Cancelled
        | IndexError::SnapshotUnavailable
        | IndexError::CorruptObject(_)
        | IndexError::Storage(_)
        | IndexError::RebuildRequired
        | IndexError::FocusSubjectNotFound
        | IndexError::MaintenanceRequired) => other,
    })?;
    if validated.members != artifact.chunks {
        return Err(IndexError::CorruptObject(
            "chunk span, input commitment or ordered membership mismatch".into(),
        ));
    }
    Ok(())
}
