//! Reuse authenticated chunk coordinates only under an explicit text-only
//! provider capability and a persisted prior reuse identity.

use super::*;

/// Borrowed, already authenticated prior owner artifact. The adapter is the
/// sole issuer; an external caller cannot construct a reusable prior owner.
#[derive(Clone, Copy)]
pub struct EmbeddingPrior<'a> {
    payload: ObjectRef,
    reuse_identity: Option<Digest>,
    chunks: &'a [ChunkMember],
    vectors: &'a [EmbeddingVector],
}

impl<'a> EmbeddingPrior<'a> {
    pub(super) fn new(
        payload: ObjectRef,
        reuse_identity: Option<Digest>,
        chunks: &'a [ChunkMember],
        vectors: &'a [EmbeddingVector],
    ) -> Self {
        Self {
            payload,
            reuse_identity,
            chunks,
            vectors,
        }
    }

    #[must_use]
    pub fn payload(self) -> ObjectRef {
        self.payload
    }

    #[must_use]
    pub fn reuse_identity(self) -> Option<Digest> {
        self.reuse_identity
    }

    #[must_use]
    pub fn chunks(self) -> &'a [ChunkMember] {
        self.chunks
    }

    #[must_use]
    pub fn vectors(self) -> &'a [EmbeddingVector] {
        self.vectors
    }
}

/// Temporary index and mapping backing, always dropped before its lease.
struct ReuseScratch<'a> {
    old_index: Vec<(Digest, usize)>,
    texts: Vec<&'a str>,
    old_vector_positions: Vec<Option<usize>>,
    missing_texts: Vec<&'a str>,
    _memory: Option<MemoryLease>,
}

/// Positional provider output is retained until its lease has been moved to
/// the final owner result; failure drops coordinate backing before its lease.
struct ChangedCoordinates {
    vectors: std::vec::IntoIter<Vec<f32>>,
    memory: Option<MemoryLease>,
}

impl ReuseScratch<'_> {
    fn new(
        old_count: usize,
        new_count: usize,
        request: &RequestBudgetV1,
    ) -> Result<Self, IndexError> {
        let old_bytes = old_count
            .checked_mul(std::mem::size_of::<(Digest, usize)>())
            .ok_or(IndexError::ResourceLimited)?;
        let per_new = std::mem::size_of::<&str>()
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<Option<usize>>()))
            .ok_or(IndexError::ResourceLimited)?;
        let new_bytes = new_count
            .checked_mul(per_new)
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = old_bytes
            .checked_add(new_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        let memory = request
            .reserve_provider_memory_v1(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )
            .map_err(|error| provider_error(&error, request))?;
        let mut scratch = Self {
            old_index: Vec::new(),
            texts: Vec::new(),
            old_vector_positions: Vec::new(),
            missing_texts: Vec::new(),
            _memory: memory,
        };
        scratch
            .old_index
            .try_reserve_exact(old_count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        scratch
            .texts
            .try_reserve_exact(new_count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        scratch
            .old_vector_positions
            .try_reserve_exact(new_count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        scratch
            .missing_texts
            .try_reserve_exact(new_count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        Ok(scratch)
    }
}

impl<P: TextEmbeddingProvider> TextEmbeddingBridge<P> {
    /// Embed one selected owner, reusing only exact logical IDs and UTF-8
    /// digests that a text-only provider previously certified. The original
    /// request owns every temporary and retained allocation made here.
    pub(super) fn embed_reusing_chunks(
        &self,
        input: &EmbeddingInput<'_>,
        prior: EmbeddingPrior<'_>,
    ) -> Result<EmbeddingOutput, IndexError> {
        self.validate_text_input(input)?;
        if !input.recipe.uses_chunks() {
            return Err(IndexError::InvalidInput(
                "semantic chunk reuse requires member recipe".into(),
            ));
        }
        budget_checkpoint(input.request_budget)?;

        // Current membership must be authenticated before invoking a prior
        // provider hook. Neither a stale owner nor malformed manifest may
        // become a speculative reuse probe.
        let plan = if input.recipe.is_certified_chunks() {
            certified_chunks::plan_within_request(
                input.source_bytes,
                input.owner,
                input.binding,
                input.recipe,
                input.request_budget,
            )?
        } else {
            let policy = input
                .recipe
                .chunk_policy
                .ok_or_else(|| IndexError::InvalidInput("semantic chunk policy missing".into()))?;
            plan_chunks(
                input.source_bytes,
                policy,
                input.recipe.max_vectors,
                input.request_budget,
            )?
        };

        let reuse_identity_matches =
            if prior.chunks.is_empty() || !self.provider.supports_deterministic_payload_reuse() {
                false
            } else if let Some(previous_identity) = prior.reuse_identity {
                self.payload_reuse_identity(input.recipe, prior.payload)? == Some(previous_identity)
            } else {
                false
            };
        if !reuse_identity_matches {
            drop(plan);
            return self.embed_without_reuse(input);
        }

        // The owner adapter has authenticated the old artifact and selected
        // payload. Recheck the local bijection and ID order before indexing.
        validate_chunk_bijection(prior.chunks, prior.vectors).map_err(|_error| {
            IndexError::CorruptObject("prior semantic member/vector bijection mismatch".into())
        })?;
        if prior
            .vectors
            .windows(2)
            .any(|pair| matches!(pair, [left, right] if left.id >= right.id))
        {
            return Err(IndexError::CorruptObject(
                "prior semantic vectors are not sorted by unique ID".into(),
            ));
        }
        let count = plan.members.len();
        let mut scratch = ReuseScratch::new(prior.chunks.len(), count, input.request_budget)?;
        scratch.old_index.extend(
            prior
                .chunks
                .iter()
                .enumerate()
                .map(|(position, member)| (member.id, position)),
        );
        budget_checkpoint(input.request_budget)?;
        scratch
            .old_index
            .sort_unstable_by_key(|(id, _position)| *id);
        budget_checkpoint(input.request_budget)?;
        self.collect_selected_texts(input, &plan.members, &mut scratch.texts)?;
        if scratch.texts.len() != count {
            return Err(IndexError::BackendViolation(
                "semantic texts differ from selected membership".into(),
            ));
        }
        let mut reused_count = 0usize;
        for (member, text) in plan.members.iter().zip(&scratch.texts) {
            budget_checkpoint(input.request_budget)?;
            let old_position = match scratch
                .old_index
                .binary_search_by_key(&member.id, |(id, _position)| *id)
            {
                Ok(position) => Some(
                    scratch
                        .old_index
                        .get(position)
                        .ok_or_else(|| {
                            IndexError::BackendViolation(
                                "prior semantic search result escaped admitted index".into(),
                            )
                        })?
                        .1,
                ),
                // An absent logical member is expected when selected chunks change.
                Err(_insertion) => None,
            };
            let selected = if let Some(old_position) = old_position {
                let old_member = prior.chunks.get(old_position).ok_or_else(|| {
                    IndexError::CorruptObject("prior semantic index escaped membership".into())
                })?;
                if old_member.input_digest == member.input_digest {
                    let vector_position = prior
                        .vectors
                        .binary_search_by(|vector| vector.id.as_slice().cmp(member.id.0.as_slice()))
                        .map_err(|_error| {
                            IndexError::CorruptObject(
                                "prior semantic member has no matching vector".into(),
                            )
                        })?;
                    let vector = prior.vectors.get(vector_position).ok_or_else(|| {
                        IndexError::CorruptObject("prior semantic vector index escaped".into())
                    })?;
                    validate_bridge_vector(&vector.values, self.dimension, self.normalization)
                        .map_err(|_error| {
                            IndexError::CorruptObject(
                                "prior semantic vector dimension or norm mismatch".into(),
                            )
                        })?;
                    reused_count = reused_count
                        .checked_add(1)
                        .ok_or(IndexError::ResourceLimited)?;
                    Some(vector_position)
                } else {
                    None
                }
            } else {
                None
            };
            scratch.old_vector_positions.push(selected);
            if selected.is_none() {
                scratch.missing_texts.push(text);
            }
        }

        // Provider coordinates and copied coordinates have distinct retained
        // leases. Reserve result array plus every 32-byte ID before either
        // provider execution or output allocation.
        let vector_bytes = std::mem::size_of::<EmbeddingVector>()
            .checked_add(32)
            .ok_or(IndexError::ResourceLimited)?;
        let result_bytes = count
            .checked_mul(vector_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        let capacity_memory = input
            .request_budget
            .reserve_provider_memory_v1(
                u64::try_from(result_bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Retained,
            )
            .map_err(|error| provider_error(&error, input.request_budget))?;
        let reuse_memory = quanta_index_core::TextEmbeddingBatch::reserve_output(
            reused_count,
            usize::try_from(self.dimension).map_err(|_error| IndexError::ResourceLimited)?,
            input.request_budget,
        )
        .map_err(|error| provider_error(&error, input.request_budget))?;

        let coordinates = if scratch.missing_texts.is_empty() {
            None
        } else {
            let output = self
                .provider
                .embed_batch_within(&scratch.missing_texts, input.request_budget)
                .map_err(|error| provider_error(&error, input.request_budget))?;
            budget_checkpoint(input.request_budget)?;
            self.verify_descriptor()?;
            if output.len() != scratch.missing_texts.len() {
                return Err(IndexError::BackendViolation(
                    "text provider changed-member cardinality differs from selected input".into(),
                ));
            }
            Some(output)
        };
        budget_checkpoint(input.request_budget)?;
        self.verify_descriptor()?;

        let mut changed = coordinates.map_or_else(
            || ChangedCoordinates {
                vectors: Vec::new().into_iter(),
                memory: None,
            },
            |output| {
                let (vectors, memory) = output.into_parts();
                ChangedCoordinates {
                    vectors: vectors.into_iter(),
                    memory,
                }
            },
        );
        let mut vectors = Vec::new();
        vectors
            .try_reserve_exact(count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        for (member, prior_position) in plan.members.iter().zip(&scratch.old_vector_positions) {
            budget_checkpoint(input.request_budget)?;
            let values = if let Some(position) = prior_position {
                let prior_vector = prior.vectors.get(*position).ok_or_else(|| {
                    IndexError::CorruptObject("prior semantic vector escaped admitted map".into())
                })?;
                let mut copy = Vec::new();
                copy.try_reserve_exact(prior_vector.values.len())
                    .map_err(|_error| IndexError::ResourceLimited)?;
                copy.extend_from_slice(&prior_vector.values);
                copy
            } else {
                let values = changed.vectors.next().ok_or_else(|| {
                    IndexError::BackendViolation("text provider omitted changed vector".into())
                })?;
                validate_bridge_vector(&values, self.dimension, self.normalization)?;
                values
            };
            let mut id = Vec::new();
            id.try_reserve_exact(32)
                .map_err(|_error| IndexError::ResourceLimited)?;
            id.extend_from_slice(&member.id.0);
            vectors.push(EmbeddingVector { id, values });
        }
        if changed.vectors.next().is_some() || vectors.len() != count {
            return Err(IndexError::BackendViolation(
                "text provider changed-member mapping is incomplete".into(),
            ));
        }
        vectors.sort_unstable_by(|left, right| left.id.cmp(&right.id));
        let provider_memory = changed.memory.take();
        Ok(EmbeddingOutput::from_scoped_parts(
            vectors,
            [provider_memory, reuse_memory, capacity_memory],
        ))
    }

    fn embed_without_reuse(
        &self,
        input: &EmbeddingInput<'_>,
    ) -> Result<EmbeddingOutput, IndexError> {
        if input.recipe.is_certified_chunks() {
            self.embed_certified_chunks(input)
        } else {
            let policy = input
                .recipe
                .chunk_policy
                .ok_or_else(|| IndexError::InvalidInput("semantic chunk policy missing".into()))?;
            self.embed_chunks(input, policy)
        }
    }

    fn collect_selected_texts<'a>(
        &self,
        input: &EmbeddingInput<'a>,
        members: &[ChunkMember],
        texts: &mut Vec<&'a str>,
    ) -> Result<(), IndexError> {
        if input.recipe.is_certified_chunks() {
            let manifest = certified_chunks::select_manifest(
                input.source_bytes,
                input.owner,
                input.binding,
                input.recipe,
            )?;
            for member in manifest.iter() {
                budget_checkpoint(input.request_budget)?;
                let member = member.map_err(|error| {
                    IndexError::InvalidInput(format!("semantic manifest member: {error}"))
                })?;
                texts.push(member.text);
            }
        } else {
            for member in members {
                budget_checkpoint(input.request_budget)?;
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
        }
        Ok(())
    }
}
