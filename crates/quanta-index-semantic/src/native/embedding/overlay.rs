//! Request-local `SourceUtf8` semantic rows ranked in the selected exact heap.
//! Source and projection objects remain owned by the selected durable view.

use super::*;

pub const MAX_SEMANTIC_OVERLAY_DOCUMENTS: usize = 256;
const MAX_OVERLAY_DOCUMENT_BYTES: usize = 1024 * 1024;
const MAX_OVERLAY_TOTAL_BYTES: usize = 16 * 1024 * 1024;
const RETAINED_VECTOR_ID_CAPACITY: usize = 64;

#[derive(Clone, Debug)]
pub struct SemanticOverlayDocument {
    pub owner: OwnerKey,
    pub revision: u64,
    /// Admitted editor path retained by the original atomic overlay snapshot.
    pub path: Option<Arc<str>>,
    /// None masks a selected durable owner without introducing a candidate.
    pub text: Option<Arc<str>>,
}

/// Retains the original atomic overlay snapshot and its grants.
///
/// The runtime owns the snapshot through admission, execution and serialization.
/// Lexical eligibility uses the same selected session and request budget.
#[derive(Debug)]
pub struct SemanticReadRequest {
    pub query: SemanticQueryRequest,
    pub overlay: Vec<SemanticOverlayDocument>,
    pub overlay_memory: Option<MemoryLease>,
    pub lexical_eligibility: Option<super::SemanticLexicalEligibility>,
}

impl From<SemanticQueryRequest> for SemanticReadRequest {
    fn from(query: SemanticQueryRequest) -> Self {
        Self {
            query,
            overlay: Vec::new(),
            overlay_memory: None,
            lexical_eligibility: None,
        }
    }
}

impl SemanticReadRequest {
    pub(super) fn validate_overlay(
        &self,
        recipe: &EmbeddingRecipe,
        budget: Option<&mut WorkBudget>,
    ) -> Result<(), IndexError> {
        if self.overlay.len() > MAX_SEMANTIC_OVERLAY_DOCUMENTS {
            return Err(IndexError::ResourceLimited);
        }
        if !self.overlay.is_empty()
            && !matches!(recipe.input_policy, EmbeddingInputPolicy::SourceUtf8)
        {
            // Editor text cannot certify a producer projection or rendered member.
            return Err(IndexError::SnapshotUnavailable);
        }
        let mut previous = None;
        let mut total = 0usize;
        for document in &self.overlay {
            if document.revision == 0 {
                return Err(IndexError::InvalidInput(
                    "semantic overlay revision must be positive".into(),
                ));
            }
            if previous.is_some_and(|owner| owner >= document.owner) {
                return Err(IndexError::InvalidInput(
                    "semantic overlay owners must be sorted and unique".into(),
                ));
            }
            previous = Some(document.owner);
            if self.query.constraints.repo_relative_path_exact.is_some()
                && document.text.is_some()
                && document.path.is_none()
            {
                return Err(IndexError::SnapshotUnavailable);
            }
            if let Some(text) = &document.text {
                if text.len() > MAX_OVERLAY_DOCUMENT_BYTES
                    || u64::try_from(text.len()).map_err(|_error| IndexError::ResourceLimited)?
                        > recipe.max_source_bytes
                {
                    return Err(IndexError::ResourceLimited);
                }
                total = total
                    .checked_add(text.len())
                    .filter(|bytes| *bytes <= MAX_OVERLAY_TOTAL_BYTES)
                    .ok_or(IndexError::ResourceLimited)?;
            }
        }
        if let Some(memory) = &self.overlay_memory {
            let row_bytes = self
                .overlay
                .capacity()
                .checked_mul(std::mem::size_of::<SemanticOverlayDocument>())
                .ok_or(IndexError::ResourceLimited)?;
            if memory.bytes()
                < u64::try_from(row_bytes).map_err(|_error| IndexError::ResourceLimited)?
            {
                return Err(IndexError::ResourceLimited);
            }
        }
        if let Some(budget) = budget {
            budget.charge(WorkUsage {
                bytes: u64::try_from(total).map_err(|_error| IndexError::ResourceLimited)?,
                mutations: u64::try_from(self.overlay.len())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SemanticOverlayChunk {
    pub id: Digest,
    pub input_digest: Digest,
    /// Byte coordinates in the editor text from the admitted snapshot.
    pub start: u64,
    pub end: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SemanticOverlayHit {
    pub revision: u64,
    pub chunk: Option<SemanticOverlayChunk>,
}

pub(super) trait OverlayEmbeddingPort: Sync {
    fn embed_overlay(
        &self,
        text: &str,
        recipe: &EmbeddingRecipe,
        budget: &mut WorkBudget,
    ) -> Result<OverlayVectors, IndexError>;
}

#[derive(Clone, Copy)]
pub(super) struct OverlaySearch<'a> {
    pub documents: &'a [SemanticOverlayDocument],
    pub bridge: &'a dyn OverlayEmbeddingPort,
}

pub(super) struct OverlayVectors {
    batch: quanta_index_core::TextEmbeddingBatch,
    chunks: Option<super::super::chunks::ChunkPlan>,
}

impl<P: TextEmbeddingProvider> OverlayEmbeddingPort for TextEmbeddingBridge<P> {
    fn embed_overlay(
        &self,
        text: &str,
        recipe: &EmbeddingRecipe,
        budget: &mut WorkBudget,
    ) -> Result<OverlayVectors, IndexError> {
        budget.checkpoint()?;
        self.verify_descriptor()?;
        verify_provider_recipe(self, recipe)?;
        let chunks = if let Some(policy) = recipe.chunk_policy {
            Some(plan_chunks_within_work(
                text.as_bytes(),
                policy,
                recipe.max_vectors,
                budget,
            )?)
        } else {
            budget.charge(WorkUsage {
                bytes: u64::try_from(text.len()).map_err(|_error| IndexError::ResourceLimited)?,
                mutations: 1,
                ..WorkUsage::default()
            })?;
            None
        };
        let count = chunks.as_ref().map_or(1, |plan| plan.members.len());
        let text_bytes = count
            .checked_mul(std::mem::size_of::<&str>())
            .ok_or(IndexError::ResourceLimited)?;
        let _text_memory = budget.reserve_memory(
            u64::try_from(text_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut texts = Vec::new();
        texts
            .try_reserve_exact(count)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if let Some(plan) = &chunks {
            for member in &plan.members {
                budget.checkpoint()?;
                let (start, end) = member.payload_span()?;
                let start = usize::try_from(start).map_err(|_error| IndexError::ResourceLimited)?;
                let end = usize::try_from(end).map_err(|_error| IndexError::ResourceLimited)?;
                texts.push(text.get(start..end).ok_or_else(|| {
                    IndexError::BackendViolation("overlay chunk escaped UTF-8 input".into())
                })?);
            }
        } else {
            texts.push(text);
        }
        // The span measures the provider invocation envelope only. Chunk
        // planning and text admission precede it; the original checkpoint,
        // descriptor and vector validation still follow it.
        let span = crate::build::begin_build_stage(
            budget.request().diagnostic_context(),
            quanta_index_core::DiagnosticStageV1::QueryOverlayEmbedding,
        );
        let child = budget
            .request()
            .diagnostic_context()
            .zip(span.as_ref())
            .map(|(diagnostic, span)| diagnostic.child_context(span));
        let provider_result = {
            let scoped = budget.scoped_diagnostic_context(child);
            self.provider.embed_batch_within(&texts, scoped.request())
        };
        super::finish_query_provider_stage(span, &provider_result, budget.request());
        let batch = provider_result.map_err(|error| provider_error(&error, budget.request()))?;
        budget.checkpoint()?;
        self.verify_descriptor()?;
        if batch.len() != count {
            return Err(IndexError::BackendViolation(
                "overlay embedding cardinality differs from selected chunks".into(),
            ));
        }
        for values in &batch {
            budget.charge(WorkUsage {
                mutations: 1,
                ..WorkUsage::default()
            })?;
            validate_bridge_vector(values, self.dimension, self.normalization)?;
        }
        Ok(OverlayVectors { batch, chunks })
    }
}

pub(super) fn masks(documents: &[SemanticOverlayDocument], owner: OwnerKey) -> bool {
    documents
        .binary_search_by_key(&owner, |document| document.owner)
        .is_ok()
}

pub(super) fn rank_overlay(
    overlay: OverlaySearch<'_>,
    query: &SearchQuery<'_>,
    query_norm: f64,
    top_k: usize,
    heap: &mut BinaryHeap<RankedHit>,
    context: &mut ReadContext<'_>,
) -> Result<(), IndexError> {
    for document in overlay.documents {
        context.budget.checkpoint()?;
        if query
            .lexical_eligibility
            .is_some_and(|eligible| !eligible.allows_overlay(document.owner, document.revision))
        {
            continue;
        }
        let Some(text) = document.text.as_deref() else {
            continue;
        };
        if let Some(path) = query.exact_path {
            let actual = document
                .path
                .as_deref()
                .ok_or(IndexError::SnapshotUnavailable)?;
            if actual != path {
                continue;
            }
        }
        let vectors = overlay
            .bridge
            .embed_overlay(text, query.recipe, context.budget)?;
        for (index, values) in vectors.batch.iter().enumerate() {
            context.budget.charge(WorkUsage {
                mutations: 1,
                ..WorkUsage::default()
            })?;
            let (id, chunk) = if let Some(plan) = &vectors.chunks {
                let member = plan.members.get(index).ok_or_else(|| {
                    IndexError::BackendViolation("overlay vector omitted chunk member".into())
                })?;
                let (start, end) = member.payload_span()?;
                (
                    member.id.0.as_slice(),
                    Some(SemanticOverlayChunk {
                        id: member.id,
                        input_digest: member.input_digest,
                        start,
                        end,
                    }),
                )
            } else {
                (b"whole-file".as_slice(), None)
            };
            let score = cosine(query.vector, query_norm, values)?;
            let better = heap.len() < top_k
                || heap.peek().is_some_and(|worst| {
                    worst
                        .0
                        .cosine
                        .total_cmp(&score)
                        .then_with(|| document.owner.cmp(&worst.0.owner))
                        .then_with(|| id.cmp(worst.0.vector_id.as_slice()))
                        == Ordering::Less
                });
            if !better {
                continue;
            }
            if id.len() > RETAINED_VECTOR_ID_CAPACITY {
                return Err(IndexError::BackendViolation(
                    "overlay vector identity exceeds retained capacity".into(),
                ));
            }
            let memory = context.budget.reserve_memory(
                u64::try_from(RETAINED_VECTOR_ID_CAPACITY)
                    .map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Retained,
            )?;
            let mut vector_id = Vec::new();
            vector_id
                .try_reserve_exact(id.len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            if vector_id.capacity() > RETAINED_VECTOR_ID_CAPACITY {
                return Err(IndexError::ResourceLimited);
            }
            vector_id.extend_from_slice(id);
            let mut hit = SearchHit::from_parts(document.owner, vector_id, score, memory);
            hit.overlay = Some(SemanticOverlayHit {
                revision: document.revision,
                chunk,
            });
            if heap.len() == top_k {
                drop(heap.pop());
            }
            heap.push(RankedHit(hit));
        }
    }
    Ok(())
}
