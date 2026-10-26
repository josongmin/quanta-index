//! Native, owner-local exact vector materialization and bounded cosine oracle.
//! No ANN generation or producer dependency inference is hidden in this module.

use std::{cmp::Ordering, collections::BinaryHeap, sync::Arc, time::Duration};

use quanta_index_contract::{
    EmbeddingNormalization, SearchPlaneErrorCodeV2, SemanticQueryRequest,
    native_protocol::{
        MAX_NATIVE_SEMANTIC_OWNER_SCAN as MAX_OWNER_SCAN,
        MAX_NATIVE_SEMANTIC_QUERY_BYTES as MAX_QUERY_BYTES, MAX_NATIVE_SEMANTIC_TOP_K as MAX_TOP_K,
        MAX_NATIVE_SEMANTIC_VECTOR_ID_BYTES as MAX_VECTOR_ID, NativeSemanticChunk,
        NativeSemanticChunkSpan, NativeSemanticTraversal,
    },
};
use quanta_index_core::index::{
    AttemptContext, CoverageBinding, CoverageStatus, Digest, GroupActivation, IndexError,
    IndexInstanceId, IndexProfile, IndexQueryPort, InputDomain, InstanceSpec,
    MAX_SOURCE_PATH_BYTES, MapRoot, MemoryLease, MemoryLifetime, ObjectChildren, ObjectRef,
    OwnerInputBinding, OwnerKey, OwnerMemberBinding, ProducerId, ProjectionBinding, ProjectionId,
    ProjectionOwnerContribution, QueryOutcome, QueryReadContext, ReadSessionLease, SchemaId,
    SourcePathKey, SourceRecord, SourceRecordOwner, StorageFormatId, ViewSelection, WorkBudget,
    WorkUsage,
    effort::ResourceNeed,
    member_map_key, owner_from_path_map_value, owner_map_key,
    ports::{
        CatalogReadPort, ImmutableObjectPort, IndexFuture, IndexOwnerBuildPort, PersistentMapPort,
    },
};
use quanta_index_core::{
    CoreError, DiagnosticSpanGuardV1, DiagnosticTerminalV1, RequestBudgetV1, TextEmbeddingProvider,
};
use sha2::{Digest as _, Sha256};

use super::certified_chunks;
use super::chunks::{
    ChunkLocation, ChunkMember, Utf8ChunkPolicy, plan_chunks, plan_chunks_within_work,
};

mod ann_directory;
mod ann_materialization;
mod ann_query;
mod chunk_materialization;
mod chunk_reuse;
mod lexical_scope;
mod path_scope;
use path_scope::{search as search_selected_exact_path, validate_path_roots};
mod overlay;
pub use lexical_scope::{SemanticLexicalEligibility, SemanticLexicalEligibleSource};
mod selected_owner;
#[cfg(test)]
use chunk_materialization::prepare_chunk_members;
use chunk_materialization::{ChunkLookup, validate_chunk_artifact, validate_chunk_bijection};
pub use chunk_reuse::EmbeddingPrior;
pub use overlay::{
    MAX_SEMANTIC_OVERLAY_DOCUMENTS, SemanticOverlayChunk, SemanticOverlayDocument,
    SemanticOverlayHit, SemanticReadRequest,
};
mod memory_envelope;
pub use memory_envelope::{chunk_owner_memory_envelope, max_chunk_owner_artifact_json_bytes};

// Bind the mandatory persisted reuse-identity field to profile admission.
// Older recipe policies require an explicit rebuild, never a legacy reader.
const RECIPE_DOMAIN: &[u8] = b"quanta-index-native-semantic-recipe-v4\0";
const MODEL_DOMAIN: &[u8] = b"quanta-index-native-semantic-text-model-v1\0";
// One vector from the complete selected UTF-8 payload. The domain stays
// stable for SourceUtf8; input selection is separately pinned by the recipe.
const WHOLE_FILE_POLICY: &[u8] = b"quanta-index-native-semantic-utf8-whole-file-one-vector-v1\0";
const COMMIT_DOMAIN: &[u8] = b"quanta-index-native-semantic-owner-v1\0";
const REUSE_DOMAIN: &[u8] = b"quanta-index-native-semantic-text-payload-reuse-v1\0";
const MAX_DIMENSION: u32 = 4096;
const MAX_VECTORS: u32 = 4096;
const MAX_CELLS: u64 = 200_000;
const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PAGE: u32 = 4096;

fn whole_file_output_policy() -> Digest {
    Digest(Sha256::digest(WHOLE_FILE_POLICY).into())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbeddingInputPolicy {
    SourceUtf8,
    ProjectionUtf8 {
        projection: ProjectionId,
        producer: ProducerId,
        schema: SchemaId,
    },
    ProjectionChunks {
        projection: ProjectionId,
        producer: ProducerId,
        schema: SchemaId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VectorNormalization {
    L2Unit,
    CosineRaw,
}
quanta_index_contract::native_serde_enum!(VectorNormalization {
    units { L2Unit = 0, CosineRaw = 1 }
    newtypes {}
    records {}
});

/// `max_vectors` and `max_source_bytes` are admission limits; they do not
/// alter output identity. The remaining fields are pinned by `fingerprint`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddingRecipe {
    pub input_policy: EmbeddingInputPolicy,
    pub model_identity: Digest,
    pub dimension: u32,
    pub normalization: VectorNormalization,
    pub output_policy: Digest,
    pub chunk_policy: Option<Utf8ChunkPolicy>,
    pub max_vectors: u32,
    pub max_source_bytes: u64,
}

impl EmbeddingRecipe {
    pub fn validate(&self) -> Result<(), IndexError> {
        if self.is_certified_chunks()
            && (self.chunk_policy.is_some()
                || self.output_policy != certified_chunks::output_policy())
        {
            return Err(IndexError::InvalidInput(
                "certified member recipe policy mismatch".into(),
            ));
        }
        if let Some(policy) = self.chunk_policy {
            policy.validate()?;
            if self.output_policy != policy.output_policy() {
                return Err(IndexError::InvalidInput(
                    "chunk recipe output policy mismatch".into(),
                ));
            }
        }
        let cells = u64::from(self.dimension)
            .checked_mul(u64::from(self.max_vectors))
            .ok_or(IndexError::ResourceLimited)?;
        if self.dimension == 0
            || self.dimension > MAX_DIMENSION
            || self.max_vectors == 0
            || self.max_vectors > MAX_VECTORS
            || cells > MAX_CELLS
            || self.max_source_bytes == 0
            || self.max_source_bytes > MAX_SOURCE_BYTES
        {
            return Err(IndexError::InvalidInput(
                "semantic recipe exceeds exact owner limits".into(),
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn is_certified_chunks(&self) -> bool {
        matches!(
            self.input_policy,
            EmbeddingInputPolicy::ProjectionChunks { .. }
        )
    }

    #[must_use]
    pub fn uses_chunks(&self) -> bool {
        self.chunk_policy.is_some() || self.is_certified_chunks()
    }

    #[must_use]
    pub fn fingerprint(&self) -> Digest {
        let mut hash = Sha256::new();
        hash.update(RECIPE_DOMAIN);
        hash.update(self.model_identity.0);
        hash.update(self.dimension.to_be_bytes());
        hash.update([match self.normalization {
            VectorNormalization::L2Unit => 1,
            VectorNormalization::CosineRaw => 2,
        }]);
        hash.update(self.output_policy.0);
        match self.chunk_policy {
            None => hash.update([0]),
            Some(policy) => {
                hash.update([1]);
                hash.update(policy.max_utf8_bytes.to_be_bytes());
            }
        }
        match self.input_policy {
            EmbeddingInputPolicy::SourceUtf8 => hash.update([1]),
            EmbeddingInputPolicy::ProjectionUtf8 {
                projection,
                producer,
                schema,
            } => {
                hash.update([2]);
                hash.update(projection.0);
                hash.update(producer.0);
                hash.update(schema.0);
            }
            EmbeddingInputPolicy::ProjectionChunks {
                projection,
                producer,
                schema,
            } => {
                hash.update([3]);
                hash.update(projection.0);
                hash.update(producer.0);
                hash.update(schema.0);
            }
        }
        Digest(hash.finalize().into())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddingVector {
    pub id: Vec<u8>,
    pub values: Vec<f32>,
}
quanta_index_contract::native_serde_struct!(EmbeddingVector {
    id: Vec<u8>,
    values: Vec<f32>,
});

/// Native provider output and its original allocation custody.
#[derive(Debug)]
pub struct EmbeddingOutput {
    vectors: Vec<EmbeddingVector>,
    memory: [Option<MemoryLease>; 3],
}
impl EmbeddingOutput {
    /// Whole-file bridge array and ID backing, apart from provider coordinates.
    pub fn single_vector_capacity_bytes() -> Result<u64, IndexError> {
        let bytes = std::mem::size_of::<EmbeddingVector>()
            .checked_add(b"whole-file".len())
            .ok_or(IndexError::ResourceLimited)?;
        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)
    }
    #[must_use]
    pub fn unscoped(vectors: Vec<EmbeddingVector>) -> Self {
        Self::from_parts(vectors, [None, None])
    }
    #[must_use]
    pub fn from_parts(vectors: Vec<EmbeddingVector>, memory: [Option<MemoryLease>; 2]) -> Self {
        let [first, second] = memory;
        Self {
            vectors,
            memory: [first, second, None],
        }
    }
    fn from_scoped_parts(vectors: Vec<EmbeddingVector>, memory: [Option<MemoryLease>; 3]) -> Self {
        Self { vectors, memory }
    }
    #[must_use]
    pub fn into_parts(self) -> (Vec<EmbeddingVector>, [Option<MemoryLease>; 3]) {
        (self.vectors, self.memory)
    }
}
impl std::ops::Deref for EmbeddingOutput {
    type Target = [EmbeddingVector];
    fn deref(&self) -> &Self::Target {
        &self.vectors
    }
}
impl PartialEq<Vec<EmbeddingVector>> for EmbeddingOutput {
    fn eq(&self, other: &Vec<EmbeddingVector>) -> bool {
        self.vectors == *other
    }
}

pub struct EmbeddingInput<'a> {
    pub owner: OwnerKey,
    /// Authenticated selected contribution/source and recipe stamp.
    pub binding: OwnerInputBinding,
    pub source: Option<&'a SourceRecord>,
    /// Selected pinned UTF-8 bytes, from the source or certified projection.
    pub source_bytes: &'a [u8],
    pub recipe: &'a EmbeddingRecipe,
    /// Providers must stop blocking/awaiting when this shared request is
    /// cancelled or reaches its deadline; the adapter rechecks after return.
    pub request_budget: &'a RequestBudgetV1,
}

/// The provider is the sole authority for semantic vectors. Its result is
/// checked before a durable owner artifact can be registered.
pub trait EmbeddingProvider: Send + Sync {
    fn model_identity(&self) -> Digest;
    fn dimension(&self) -> u32;
    fn normalization(&self) -> VectorNormalization;
    fn output_policy(&self) -> Digest;
    /// Return a durable identity only when output depends solely on the
    /// certified payload and pinned recipe. Metadata-sensitive providers keep
    /// the default `None`, including when the payload bytes happen to match.
    fn payload_reuse_identity(
        &self,
        _recipe: &EmbeddingRecipe,
        _payload: ObjectRef,
    ) -> Result<Option<Digest>, IndexError> {
        Ok(None)
    }
    /// Local, side-effect-free declaration of authenticated member reuse support.
    /// Providers depending on owner/source/batch metadata retain the default.
    fn supports_member_reuse(&self) -> bool {
        false
    }
    fn embed<'a>(&'a self, input: EmbeddingInput<'a>) -> IndexFuture<'a, EmbeddingOutput>;
    /// Consume authenticated borrowed prior membership without owning its lease.
    /// Metadata-sensitive providers derive normally; the adapter retains prior
    /// readback custody until the returned future completes.
    fn embed_with_prior<'a>(
        &'a self,
        input: EmbeddingInput<'a>,
        _prior: EmbeddingPrior<'a>,
    ) -> IndexFuture<'a, EmbeddingOutput> {
        self.embed(input)
    }
}

impl<P: EmbeddingProvider + ?Sized> EmbeddingProvider for Arc<P> {
    fn model_identity(&self) -> Digest {
        (**self).model_identity()
    }

    fn dimension(&self) -> u32 {
        (**self).dimension()
    }

    fn normalization(&self) -> VectorNormalization {
        (**self).normalization()
    }

    fn output_policy(&self) -> Digest {
        (**self).output_policy()
    }

    fn payload_reuse_identity(
        &self,
        recipe: &EmbeddingRecipe,
        payload: ObjectRef,
    ) -> Result<Option<Digest>, IndexError> {
        (**self).payload_reuse_identity(recipe, payload)
    }

    fn supports_member_reuse(&self) -> bool {
        (**self).supports_member_reuse()
    }
    fn embed<'a>(&'a self, input: EmbeddingInput<'a>) -> IndexFuture<'a, EmbeddingOutput> {
        (**self).embed(input)
    }
    fn embed_with_prior<'a>(
        &'a self,
        input: EmbeddingInput<'a>,
        prior: EmbeddingPrior<'a>,
    ) -> IndexFuture<'a, EmbeddingOutput> {
        (**self).embed_with_prior(input, prior)
    }
}

/// Adapts the existing text provider to one vector per complete UTF-8 input.
///
/// The provider's immutable model revision and normalization are checked at
/// construction and again before every call. Source and producer projection
/// inputs use the same vector algorithm but distinct recipe fingerprints.
/// Chunking and language-specific extraction require a distinct output policy.
pub struct TextEmbeddingBridge<P> {
    provider: P,
    model_id: String,
    model_revision: String,
    model_identity: Digest,
    dimension: u32,
    normalization: VectorNormalization,
    output_policy: Digest,
    chunk_policy: Option<Utf8ChunkPolicy>,
}

impl<P: TextEmbeddingProvider> TextEmbeddingBridge<P> {
    pub fn new(provider: P) -> Result<Self, IndexError> {
        let model_id = provider.model_id();
        let model_revision = provider.model_revision();
        if model_id.is_empty()
            || model_revision.is_empty()
            || model_id.len() > 1024
            || model_revision.len() > 1024
        {
            return Err(IndexError::InvalidInput(
                "text provider lacks a bounded model identity and revision".into(),
            ));
        }
        let model_id = model_id.to_owned();
        let model_revision = model_revision.to_owned();
        let dimension = u32::try_from(provider.dimension())
            .map_err(|_conversion_error| IndexError::ResourceLimited)?;
        if dimension == 0 || dimension > MAX_DIMENSION {
            return Err(IndexError::InvalidInput(
                "text provider dimension exceeds native semantic limit".into(),
            ));
        }
        let normalization = match provider.normalization() {
            EmbeddingNormalization::None => VectorNormalization::CosineRaw,
            EmbeddingNormalization::L2Unit => VectorNormalization::L2Unit,
        };
        let mut hash = Sha256::new();
        hash.update(MODEL_DOMAIN);
        hash.update(
            u64::try_from(model_id.len())
                .map_err(|_conversion_error| IndexError::ResourceLimited)?
                .to_be_bytes(),
        );
        hash.update(model_id.as_bytes());
        hash.update(
            u64::try_from(model_revision.len())
                .map_err(|_conversion_error| IndexError::ResourceLimited)?
                .to_be_bytes(),
        );
        hash.update(model_revision.as_bytes());
        let model_identity = Digest(hash.finalize().into());
        let output_policy = whole_file_output_policy();
        Ok(Self {
            provider,
            model_id,
            model_revision,
            model_identity,
            dimension,
            normalization,
            output_policy,
            chunk_policy: None,
        })
    }

    /// Partition selected UTF-8 input under a pinned file-local policy.
    /// Producer dependency extraction and stable producer chunk IDs are separate inputs.
    pub fn with_utf8_chunks(provider: P, policy: Utf8ChunkPolicy) -> Result<Self, IndexError> {
        policy.validate()?;
        let mut bridge = Self::new(provider)?;
        bridge.output_policy = policy.output_policy();
        bridge.chunk_policy = Some(policy);
        Ok(bridge)
    }

    /// Embed producer-certified stable members through the same provider and owner artifact.
    pub fn with_certified_chunks(provider: P) -> Result<Self, IndexError> {
        let mut bridge = Self::new(provider)?;
        bridge.output_policy = certified_chunks::output_policy();
        Ok(bridge)
    }

    pub fn certified_chunk_recipe(
        &self,
        projection: ProjectionId,
        producer: ProducerId,
        schema: SchemaId,
        max_input_bytes: u64,
        max_vectors: u32,
    ) -> Result<EmbeddingRecipe, IndexError> {
        if self.output_policy != certified_chunks::output_policy() || self.chunk_policy.is_some() {
            return Err(IndexError::InvalidInput(
                "bridge has no certified chunk policy".into(),
            ));
        }
        let recipe = EmbeddingRecipe {
            input_policy: EmbeddingInputPolicy::ProjectionChunks {
                projection,
                producer,
                schema,
            },
            model_identity: self.model_identity,
            dimension: self.dimension,
            normalization: self.normalization,
            output_policy: self.output_policy,
            chunk_policy: None,
            max_vectors,
            max_source_bytes: max_input_bytes,
        };
        recipe.validate()?;
        Ok(recipe)
    }

    pub fn chunk_recipe(
        &self,
        max_source_bytes: u64,
        max_vectors: u32,
    ) -> Result<EmbeddingRecipe, IndexError> {
        if self.chunk_policy.is_none() {
            return Err(IndexError::InvalidInput(
                "bridge has no chunk policy".into(),
            ));
        }
        let mut recipe = self.recipe(max_source_bytes)?;
        recipe.max_vectors = max_vectors;
        recipe.validate()?;
        Ok(recipe)
    }

    pub fn recipe(&self, max_source_bytes: u64) -> Result<EmbeddingRecipe, IndexError> {
        if self.output_policy == certified_chunks::output_policy() {
            return Err(IndexError::InvalidInput(
                "certified bridge requires projection identity".into(),
            ));
        }
        let recipe = EmbeddingRecipe {
            input_policy: EmbeddingInputPolicy::SourceUtf8,
            model_identity: self.model_identity,
            dimension: self.dimension,
            normalization: self.normalization,
            output_policy: self.output_policy,
            chunk_policy: self.chunk_policy,
            max_vectors: 1,
            max_source_bytes,
        };
        recipe.validate()?;
        Ok(recipe)
    }

    pub fn projection_recipe(
        &self,
        projection: ProjectionId,
        producer: ProducerId,
        schema: SchemaId,
        max_input_bytes: u64,
    ) -> Result<EmbeddingRecipe, IndexError> {
        let mut recipe = self.recipe(max_input_bytes)?;
        recipe.input_policy = EmbeddingInputPolicy::ProjectionUtf8 {
            projection,
            producer,
            schema,
        };
        Ok(recipe)
    }

    fn validate_text_input(&self, input: &EmbeddingInput<'_>) -> Result<(), IndexError> {
        self.verify_descriptor()?;
        input.recipe.validate()?;
        if u64::try_from(input.source_bytes.len()).map_err(|_error| IndexError::ResourceLimited)?
            > input.recipe.max_source_bytes
        {
            return Err(IndexError::ResourceLimited);
        }
        if input.recipe.model_identity != self.model_identity
            || input.recipe.dimension != self.dimension
            || input.recipe.normalization != self.normalization
            || input.recipe.output_policy != self.output_policy
            || input.recipe.chunk_policy != self.chunk_policy
            || input.recipe.is_certified_chunks()
                != (self.output_policy == certified_chunks::output_policy())
            || (matches!(input.recipe.input_policy, EmbeddingInputPolicy::SourceUtf8)
                && input.source.is_none())
        {
            return Err(IndexError::InvalidInput(
                "text provider recipe mismatch".into(),
            ));
        }
        Ok(())
    }

    fn verify_descriptor(&self) -> Result<(), IndexError> {
        if self.provider.model_id() != self.model_id
            || self.provider.model_revision() != self.model_revision
            || u32::try_from(self.provider.dimension()) != Ok(self.dimension)
            || self.provider.normalization()
                != match self.normalization {
                    VectorNormalization::CosineRaw => EmbeddingNormalization::None,
                    VectorNormalization::L2Unit => EmbeddingNormalization::L2Unit,
                }
        {
            return Err(IndexError::BackendViolation(
                "text provider descriptor changed after composition".into(),
            ));
        }
        Ok(())
    }

    /// Query embedding uses the same provider, revision and caller budget as
    /// corpus materialization. The caller must keep the recipe from this bridge.
    pub fn embed_query(
        &self,
        text: &str,
        budget: &RequestBudgetV1,
    ) -> Result<quanta_index_core::TextEmbeddingBatch, IndexError> {
        self.verify_descriptor()?;
        budget_checkpoint(budget)?;
        // Observe only the provider invocation envelope. Descriptor checks,
        // request checkpoints, cardinality and vector validation retain their
        // original order outside this span.
        let span = crate::build::begin_build_stage(
            budget.diagnostic_context(),
            quanta_index_core::DiagnosticStageV1::QueryEmbedding,
        );
        let child = budget
            .diagnostic_context()
            .zip(span.as_ref())
            .map(|(diagnostic, span)| diagnostic.child_context(span));
        let provider_budget = child.map_or_else(
            || std::borrow::Cow::Borrowed(budget),
            |child| std::borrow::Cow::Owned(budget.clone().with_diagnostic_context(child)),
        );
        let provider_result = self
            .provider
            .embed_batch_within(&[text], provider_budget.as_ref());
        finish_query_provider_stage(span, &provider_result, budget);
        let vectors = provider_result.map_err(|error| provider_error(&error, budget))?;
        budget_checkpoint(budget)?;
        self.verify_descriptor()?;
        if vectors.len() != 1 {
            return Err(IndexError::BackendViolation(
                "text provider returned a non-singleton query batch".into(),
            ));
        }
        let vector = vectors.first().ok_or_else(|| {
            IndexError::BackendViolation("text provider omitted query vector".into())
        })?;
        validate_bridge_vector(vector, self.dimension, self.normalization)?;
        Ok(vectors)
    }
}

/// Classify the provider invocation without advancing the product deadline
/// checkpoint or moving its original error conversion before span completion.
fn finish_query_provider_stage<T>(
    span: Option<DiagnosticSpanGuardV1>,
    result: &Result<T, CoreError>,
    budget: &RequestBudgetV1,
) {
    let Some(span) = span else {
        return;
    };
    let terminal = match result {
        Ok(_) => DiagnosticTerminalV1::Ok,
        Err(error)
            if budget.is_cancelled()
                || matches!(
                    error,
                    CoreError::Typed {
                        code: SearchPlaneErrorCodeV2::RequestCancelled,
                        ..
                    }
                ) =>
        {
            DiagnosticTerminalV1::Cancelled { code: 1 }
        }
        Err(_) => DiagnosticTerminalV1::Error { code: 1 },
    };
    let _finished = span.finish(terminal);
}

impl<P: TextEmbeddingProvider> EmbeddingProvider for TextEmbeddingBridge<P> {
    fn model_identity(&self) -> Digest {
        self.model_identity
    }
    fn dimension(&self) -> u32 {
        self.dimension
    }
    fn normalization(&self) -> VectorNormalization {
        self.normalization
    }
    fn output_policy(&self) -> Digest {
        self.output_policy
    }

    fn payload_reuse_identity(
        &self,
        recipe: &EmbeddingRecipe,
        payload: ObjectRef,
    ) -> Result<Option<Digest>, IndexError> {
        self.verify_descriptor()?;
        verify_provider_recipe(self, recipe)?;
        if !self.provider.supports_deterministic_payload_reuse() {
            return Ok(None);
        }
        let mut hash = Sha256::new();
        hash.update(REUSE_DOMAIN);
        hash.update(recipe.fingerprint().0);
        hash.update(self.model_identity.0);
        hash.update(self.output_policy.0);
        hash.update(payload.0.0);
        Ok(Some(Digest(hash.finalize().into())))
    }

    fn supports_member_reuse(&self) -> bool {
        self.provider.supports_deterministic_payload_reuse()
    }

    fn embed<'a>(&'a self, input: EmbeddingInput<'a>) -> IndexFuture<'a, EmbeddingOutput> {
        Box::pin(async move {
            self.validate_text_input(&input)?;
            budget_checkpoint(input.request_budget)?;
            if input.recipe.is_certified_chunks() {
                return self.embed_certified_chunks(&input);
            }
            let text = std::str::from_utf8(input.source_bytes).map_err(|_utf8_error| {
                IndexError::InvalidInput("semantic input is not UTF-8".into())
            })?;
            budget_checkpoint(input.request_budget)?;
            if let Some(policy) = self.chunk_policy {
                return self.embed_chunks(&input, policy);
            }
            let capacity_memory = input
                .request_budget
                .reserve_provider_memory_v1(
                    EmbeddingOutput::single_vector_capacity_bytes()?,
                    MemoryLifetime::Retained,
                )
                .map_err(|error| provider_error(&error, input.request_budget))?;
            let output = self
                .provider
                .embed_batch_within(&[text], input.request_budget)
                .map_err(|error| provider_error(&error, input.request_budget))?;
            budget_checkpoint(input.request_budget)?;
            self.verify_descriptor()?;
            if output.len() != 1 {
                return Err(IndexError::BackendViolation(
                    "text provider returned a non-singleton owner batch".into(),
                ));
            }
            let parts = output.into_parts();
            let memory = parts.1;
            let mut vectors = parts.0;
            let vector = vectors.pop().ok_or_else(|| {
                IndexError::BackendViolation("text provider omitted owner vector".into())
            })?;
            validate_bridge_vector(&vector, self.dimension, self.normalization)?;
            let mut output = Vec::new();
            output
                .try_reserve_exact(1)
                .map_err(|_error| IndexError::ResourceLimited)?;
            let mut id = Vec::new();
            id.try_reserve_exact(b"whole-file".len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            id.extend_from_slice(b"whole-file");
            output.push(EmbeddingVector { id, values: vector });
            Ok(EmbeddingOutput::from_parts(
                output,
                [memory, capacity_memory],
            ))
        })
    }
    fn embed_with_prior<'a>(
        &'a self,
        input: EmbeddingInput<'a>,
        prior: EmbeddingPrior<'a>,
    ) -> IndexFuture<'a, EmbeddingOutput> {
        Box::pin(async move {
            self.validate_text_input(&input)?;
            if !input.recipe.uses_chunks() {
                return self.embed(input).await;
            }
            self.embed_reusing_chunks(&input, prior)
        })
    }
}

fn budget_checkpoint(budget: &RequestBudgetV1) -> Result<(), IndexError> {
    if budget.is_cancelled() {
        Err(IndexError::Cancelled)
    } else if std::time::Instant::now() >= budget.deadline() {
        Err(IndexError::ResourceLimited)
    } else {
        Ok(())
    }
}

fn provider_error(error: &CoreError, budget: &RequestBudgetV1) -> IndexError {
    if let Err(interruption) = budget_checkpoint(budget) {
        return interruption;
    }
    match error {
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::ServerOverloaded,
            ..
        } => IndexError::ResourceLimited,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::RequestCancelled,
            ..
        } => IndexError::Cancelled,
        CoreError::InvalidContract(_)
        | CoreError::Typed { .. }
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)
        | CoreError::StorageIo { .. } => {
            IndexError::Storage(format!("text embedding provider: {error}"))
        }
    }
}

fn validate_bridge_vector(
    vector: &[f32],
    dimension: u32,
    normalization: VectorNormalization,
) -> Result<(), IndexError> {
    let norm = vector_norm(vector, dimension)
        .map_err(|error| IndexError::BackendViolation(format!("text provider vector: {error}")))?;
    if normalization == VectorNormalization::L2Unit && (norm - 1.0).abs() > 1e-3 {
        return Err(IndexError::BackendViolation(
            "text provider violated declared L2Unit normalization".into(),
        ));
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct OwnerArtifact {
    owner: OwnerKey,
    input: OwnerInputBinding,
    recipe: Digest,
    payload_ref: ObjectRef,
    reuse_identity: Option<Digest>,
    chunks: Vec<ChunkMember>,
    vectors: Vec<EmbeddingVector>,
}
quanta_index_contract::native_serde_struct!(OwnerArtifact {
    owner: OwnerKey,
    input: OwnerInputBinding,
    recipe: Digest,
    payload_ref: ObjectRef,
    reuse_identity: Option<Digest>,
    chunks: Vec<ChunkMember>,
    vectors: Vec<EmbeddingVector>,
});

pub struct SemanticCandidate {
    instance: IndexInstanceId,
    format: StorageFormatId,
    artifact: OwnerArtifact,
    _output_memory: [Option<MemoryLease>; 3],
    _membership_memory: Option<MemoryLease>,
}

struct OwnerArtifactJson {
    bytes: Vec<u8>,
    // The JSON backing drops before its temporary allocation charge.
    _memory: Option<MemoryLease>,
}

/// Counting borrows the artifact; encoding can only fill already admitted backing.
struct OwnerArtifactJsonWriter<'a> {
    output: Option<&'a mut Vec<u8>>,
    length: usize,
    maximum: usize,
    budget: &'a WorkBudget,
    failure: Option<IndexError>,
}

impl OwnerArtifactJsonWriter<'_> {
    fn refuse(&mut self, failure: IndexError) -> std::io::Error {
        self.failure = Some(failure);
        std::io::Error::other("semantic artifact encoding refused")
    }

    fn serialize(&mut self, artifact: &OwnerArtifact) -> Result<usize, IndexError> {
        if let Err(error) = serde_json::to_writer(&mut *self, artifact) {
            return Err(self.failure.take().unwrap_or_else(|| {
                IndexError::BackendViolation(format!("vector encoding: {error}"))
            }));
        }
        self.budget.checkpoint()?;
        Ok(self.length)
    }
}

impl std::io::Write for OwnerArtifactJsonWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if let Err(error) = self.budget.checkpoint() {
            return Err(self.refuse(error));
        }
        let Some(next) = self.length.checked_add(bytes.len()) else {
            return Err(self.refuse(IndexError::ResourceLimited));
        };
        if next > self.maximum {
            return Err(self.refuse(IndexError::BackendViolation(
                "vector encoding exceeds counted capacity".into(),
            )));
        }
        if let Some(output) = &mut self.output {
            if next > output.capacity() {
                return Err(self.refuse(IndexError::BackendViolation(
                    "vector encoding lost admitted backing".into(),
                )));
            }
            output.extend_from_slice(bytes);
        }
        self.length = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.budget.checkpoint().map_err(|error| self.refuse(error))
    }
}

fn encode_owner_artifact(
    artifact: &OwnerArtifact,
    budget: &mut WorkBudget,
) -> Result<OwnerArtifactJson, IndexError> {
    let estimated = artifact.vectors.iter().try_fold(1024u64, |total, vector| {
        let id = u64::try_from(vector.id.len()).map_err(|_error| IndexError::ResourceLimited)?;
        let values =
            u64::try_from(vector.values.len()).map_err(|_error| IndexError::ResourceLimited)?;
        total
            .checked_add(id.checked_mul(4).ok_or(IndexError::ResourceLimited)?)
            .and_then(|bytes| bytes.checked_add(values.checked_mul(24)?))
            .and_then(|bytes| bytes.checked_add(128))
            .ok_or(IndexError::ResourceLimited)
    })?;
    // Retain the existing cumulative work policy separately from live capacity.
    budget.charge(WorkUsage {
        bytes: estimated,
        ..WorkUsage::default()
    })?;
    let length = OwnerArtifactJsonWriter {
        output: None,
        length: 0,
        maximum: usize::MAX,
        budget,
        failure: None,
    }
    .serialize(artifact)?;
    let bytes = u64::try_from(length).map_err(|_error| IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes,
        ..WorkUsage::default()
    })?;
    let memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
    let mut encoded = OwnerArtifactJson {
        bytes: Vec::new(),
        _memory: memory,
    };
    encoded
        .bytes
        .try_reserve_exact(length)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let written = OwnerArtifactJsonWriter {
        output: Some(&mut encoded.bytes),
        length: 0,
        maximum: length,
        budget,
        failure: None,
    }
    .serialize(artifact)?;
    if written != length {
        return Err(IndexError::BackendViolation(
            "vector encoding differs from counted length".into(),
        ));
    }
    Ok(encoded)
}

pub struct NativeSemantic<P> {
    provider: P,
    recipe: EmbeddingRecipe,
    ann: bool,
}

fn reserve_json_decode(
    payload_bytes: usize,
    budget: &WorkBudget,
    lifetime: MemoryLifetime,
) -> Result<Option<MemoryLease>, IndexError> {
    // Logical allowance for decoded backing and parser scratch, not a hard RSS cap.
    let bytes = u64::try_from(payload_bytes)
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_mul(8)
        .ok_or(IndexError::ResourceLimited)?;
    if bytes == 0 {
        budget.checkpoint()?;
        Ok(None)
    } else {
        budget.reserve_memory(bytes, lifetime)
    }
}

fn load_source_record(
    reference: ObjectRef,
    context: &mut AttemptContext<'_>,
) -> Result<SourceRecordOwner, IndexError> {
    load_source_record_from_objects(reference, context.objects, context.budget)
}

fn load_source_record_from_objects(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<SourceRecordOwner, IndexError> {
    let stored = objects.get(reference, budget)?;
    if stored.registration.reference != reference {
        return Err(IndexError::CorruptObject(
            "source record reference mismatch".into(),
        ));
    }
    let source =
        SourceRecordOwner::decode_json(&stored.payload, budget, MemoryLifetime::Temporary)?;
    if stored.registration.children.as_slice() != [source.bytes] {
        return Err(IndexError::CorruptObject(
            "source record edges mismatch".into(),
        ));
    }
    Ok(source)
}

fn contribution_children(contribution: ProjectionOwnerContribution) -> ArtifactChildren {
    match (contribution.source, contribution.output) {
        (None, None) => ArtifactChildren::Empty,
        (Some(reference), None) | (None, Some(reference)) => ArtifactChildren::One([reference]),
        (Some(first), Some(second)) if first == second => ArtifactChildren::One([first]),
        (Some(first), Some(second)) => {
            let mut values = [first, second];
            values.sort_unstable();
            ArtifactChildren::Two(values)
        }
    }
}

fn load_contribution(
    reference: ObjectRef,
    context: &mut AttemptContext<'_>,
) -> Result<ProjectionOwnerContribution, IndexError> {
    load_contribution_from_objects(reference, context.objects, context.budget)
}

fn load_contribution_from_objects(
    reference: ObjectRef,
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
) -> Result<ProjectionOwnerContribution, IndexError> {
    let stored = objects.get(reference, budget)?;
    if stored.registration.reference != reference {
        return Err(IndexError::CorruptObject(
            "projection contribution reference mismatch".into(),
        ));
    }
    let _decode_memory =
        reserve_json_decode(stored.payload.len(), budget, MemoryLifetime::Temporary)?;
    let contribution: ProjectionOwnerContribution = serde_json::from_slice(&stored.payload)
        .map_err(|error| IndexError::CorruptObject(format!("projection contribution: {error}")))?;
    budget.checkpoint()?;
    if stored.registration.children.as_slice() != contribution_children(contribution).as_ref() {
        return Err(IndexError::CorruptObject(
            "projection contribution edges mismatch".into(),
        ));
    }
    Ok(contribution)
}

fn contribution_context(contribution: ProjectionOwnerContribution) -> Digest {
    contribution.materialization_context()
}

fn projection_context(binding: &ProjectionBinding) -> Digest {
    binding.materialization_context()
}

impl<P: EmbeddingProvider> NativeSemantic<P> {
    pub fn new(provider: P, recipe: EmbeddingRecipe) -> Result<Self, IndexError> {
        recipe.validate()?;
        verify_provider_recipe(&provider, &recipe)?;
        Ok(Self {
            provider,
            recipe,
            ann: false,
        })
    }

    /// Select the immutable multi-owner region layout for this adapter.
    /// Publication futures that seal a region must run inside a Tokio runtime.
    /// A missing runtime is refused before invoking the Lance backend.
    #[must_use]
    pub fn with_ann(mut self) -> Self {
        self.ann = true;
        self
    }

    #[must_use]
    pub fn recipe(&self) -> &EmbeddingRecipe {
        &self.recipe
    }
}

fn validate_semantic_instance<P: EmbeddingProvider>(
    provider: &P,
    recipe: &EmbeddingRecipe,
    instance: &InstanceSpec,
) -> Result<(), IndexError> {
    recipe.validate()?;
    verify_provider_recipe(provider, recipe)?;
    let domain_matches = match recipe.input_policy {
        EmbeddingInputPolicy::SourceUtf8 => instance.input_domain == InputDomain::SourceLocal,
        EmbeddingInputPolicy::ProjectionUtf8 {
            projection,
            producer,
            schema,
        }
        | EmbeddingInputPolicy::ProjectionChunks {
            projection,
            producer,
            schema,
        } => {
            instance.input_domain
                == (InputDomain::Projection {
                    projection,
                    producer,
                })
                && instance.schema == schema
        }
    };
    if instance.activation != GroupActivation::AsyncCoverage
        || instance.policy != recipe.fingerprint()
        || !domain_matches
    {
        return Err(IndexError::InvalidInput(
            "semantic instance differs from pinned recipe or input domain".into(),
        ));
    }
    Ok(())
}

impl<P: EmbeddingProvider> IndexOwnerBuildPort for NativeSemantic<P> {
    type Candidate = SemanticCandidate;

    fn validate_coverage(&self, coverage: &CoverageBinding) -> Result<(), IndexError> {
        if self.ann && coverage.owners.0.is_some() && coverage.searchable == coverage.owners.0 {
            return Err(IndexError::RebuildRequired);
        }
        Ok(())
    }

    fn prepare_searchable<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        prior: Option<&'a CoverageBinding>,
        owners: MapRoot,
        materialized: Option<(OwnerKey, ObjectRef)>,
        object_maps: Option<
            &'a std::sync::Arc<dyn quanta_index_core::index::ports::ObjectMapBatchPort>,
        >,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, quanta_index_core::index::SearchablePreparation> {
        Box::pin(async move {
            self.validate_instance(instance)?;
            if self.ann {
                ann_materialization::prepare(
                    instance,
                    &self.recipe,
                    prior,
                    owners,
                    materialized,
                    object_maps,
                    context,
                )
                .await
            } else {
                context.budget.checkpoint()?;
                Ok(quanta_index_core::index::SearchablePreparation::existing(
                    owners.0,
                ))
            }
        })
    }

    fn resource_demand(&self) -> ResourceNeed {
        ResourceNeed {
            provider_units: 1,
            ..ResourceNeed::default()
        }
    }

    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        validate_semantic_instance(&self.provider, &self.recipe, instance)
    }

    fn prepare_owner<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        owner: OwnerKey,
        input: OwnerInputBinding,
        prior: Option<OwnerMemberBinding>,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, Self::Candidate> {
        Box::pin(async move {
            self.validate_instance(instance)?;
            let fingerprint = self.recipe.fingerprint();
            if instance.policy != fingerprint || input.derivation_policy != fingerprint {
                return Err(IndexError::InvalidInput(
                    "instance or owner policy differs from embedding recipe".into(),
                ));
            }
            let (source, payload_ref) = match self.recipe.input_policy {
                EmbeddingInputPolicy::SourceUtf8 => {
                    if instance.input_domain != InputDomain::SourceLocal
                        || input.projection.is_some()
                        || input.producer_context != Digest([0; 32])
                    {
                        return Err(IndexError::InvalidInput(
                            "source input policy mismatch".into(),
                        ));
                    }
                    let source_ref = input.source.ok_or_else(|| {
                        IndexError::InvalidInput("deleted owner cannot be embedded".into())
                    })?;
                    let source = load_source_record(source_ref, context)?;
                    let payload_ref = source.bytes;
                    (Some(source), payload_ref)
                }
                EmbeddingInputPolicy::ProjectionUtf8 {
                    projection,
                    producer,
                    schema,
                }
                | EmbeddingInputPolicy::ProjectionChunks {
                    projection,
                    producer,
                    schema,
                } => {
                    if instance.input_domain
                        != (InputDomain::Projection {
                            projection,
                            producer,
                        })
                        || instance.schema != schema
                    {
                        return Err(IndexError::InvalidInput(
                            "projection input policy mismatch".into(),
                        ));
                    }
                    let reference = input.projection.ok_or_else(|| {
                        IndexError::InvalidInput("projection contribution is absent".into())
                    })?;
                    let contribution = load_contribution(reference, context)?;
                    if contribution.projection != projection
                        || contribution.producer != producer
                        || contribution.owner != owner
                        || contribution.source != input.source
                        || contribution.source_owner.is_some() != contribution.source.is_some()
                        || contribution_context(contribution) != input.producer_context
                    {
                        return Err(IndexError::InvalidInput(
                            "projection owner stamp mismatch".into(),
                        ));
                    }
                    let source = contribution
                        .source
                        .map(|reference| load_source_record(reference, context))
                        .transpose()?;
                    let output = contribution.output.ok_or_else(|| {
                        IndexError::InvalidInput("certified empty contribution has no work".into())
                    })?;
                    (source, output)
                }
            };
            context.budget.checkpoint()?;
            let bytes = context.objects.get(payload_ref, context.budget)?;
            if bytes.registration.reference != payload_ref
                || !bytes.registration.children.is_empty()
            {
                return Err(IndexError::CorruptObject(
                    "semantic input payload is not a leaf".into(),
                ));
            }
            let payload_len = u64::try_from(bytes.payload.len())
                .map_err(|_conversion_error| IndexError::ResourceLimited)?;
            if payload_len > self.recipe.max_source_bytes {
                return Err(IndexError::ResourceLimited);
            }
            if source.as_ref().is_some_and(|record| {
                matches!(self.recipe.input_policy, EmbeddingInputPolicy::SourceUtf8)
                    && record.byte_len != payload_len
            }) {
                return Err(IndexError::CorruptObject(
                    "source byte length mismatch".into(),
                ));
            }
            if !self.recipe.is_certified_chunks() && std::str::from_utf8(&bytes.payload).is_err() {
                return Err(IndexError::InvalidInput(
                    "semantic input is not UTF-8".into(),
                ));
            }
            // Authenticate membership before any provider-owned reuse hook.
            let chunk_plan = if self.recipe.is_certified_chunks() {
                Some(certified_chunks::plan_within_work(
                    &bytes.payload,
                    owner,
                    input,
                    &self.recipe,
                    context.budget,
                )?)
            } else {
                self.recipe
                    .chunk_policy
                    .map(|policy| {
                        plan_chunks_within_work(
                            &bytes.payload,
                            policy,
                            self.recipe.max_vectors,
                            context.budget,
                        )
                    })
                    .transpose()?
            };
            let reuse_identity = self
                .provider
                .payload_reuse_identity(&self.recipe, payload_ref)?;
            let (reusable, prior_artifact) = if let Some(previous) = prior {
                if previous.instance != instance.instance
                    || previous.format != instance.format
                    || previous.owner != owner
                    || previous.input.derivation_policy != fingerprint
                {
                    return Err(IndexError::CorruptObject(
                        "prior semantic member identity mismatch".into(),
                    ));
                }
                let old =
                    load_owner_artifact(context.objects, context.budget, &previous, &self.recipe)?;
                let (old_payload, old_source_len) = match self.recipe.input_policy {
                    EmbeddingInputPolicy::SourceUtf8 => {
                        if previous.input.projection.is_some()
                            || previous.input.producer_context != Digest([0; 32])
                        {
                            return Err(IndexError::CorruptObject(
                                "prior source input policy mismatch".into(),
                            ));
                        }
                        let source_ref = previous.input.source.ok_or_else(|| {
                            IndexError::CorruptObject("prior semantic source missing".into())
                        })?;
                        let old_source = load_source_record(source_ref, context)?;
                        (old_source.bytes, Some(old_source.byte_len))
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
                        let contribution_ref = previous.input.projection.ok_or_else(|| {
                            IndexError::CorruptObject(
                                "prior projection contribution missing".into(),
                            )
                        })?;
                        let contribution = load_contribution(contribution_ref, context)?;
                        if contribution.projection != projection
                            || contribution.producer != producer
                            || contribution.owner != owner
                            || contribution.source != previous.input.source
                            || contribution.source_owner.is_some() != contribution.source.is_some()
                            || contribution_context(contribution) != previous.input.producer_context
                        {
                            return Err(IndexError::CorruptObject(
                                "prior projection owner stamp mismatch".into(),
                            ));
                        }
                        if let Some(source_ref) = contribution.source {
                            let _source = load_source_record(source_ref, context)?;
                        }
                        (
                            contribution.output.ok_or_else(|| {
                                IndexError::CorruptObject("prior projection output missing".into())
                            })?,
                            None,
                        )
                    }
                };
                if old.payload_ref != old_payload {
                    return Err(IndexError::CorruptObject(
                        "prior semantic artifact differs from prior input".into(),
                    ));
                }
                let old_payload_len = if old_payload == payload_ref {
                    payload_len
                } else {
                    let stored = context.objects.get(old_payload, context.budget)?;
                    if stored.registration.reference != old_payload
                        || !stored.registration.children.is_empty()
                    {
                        return Err(IndexError::CorruptObject(
                            "prior semantic payload is not a leaf".into(),
                        ));
                    }
                    u64::try_from(stored.payload.len())
                        .map_err(|_conversion_error| IndexError::ResourceLimited)?
                };
                if old_payload_len > self.recipe.max_source_bytes
                    || old_source_len.is_some_and(|length| length != old_payload_len)
                {
                    return Err(IndexError::CorruptObject(
                        "prior semantic payload length mismatch".into(),
                    ));
                }
                if old_payload == payload_ref
                    && reuse_identity.is_some()
                    && reuse_identity == old.reuse_identity
                {
                    (
                        Some(EmbeddingOutput::from_parts(
                            old.artifact.vectors,
                            [old.decode_memory, None],
                        )),
                        None,
                    )
                } else {
                    let reusable_prior = if self.recipe.uses_chunks()
                        && old.reuse_identity.is_some()
                        && self.provider.supports_member_reuse()
                    {
                        Some(old)
                    } else {
                        None
                    };
                    (None, reusable_prior)
                }
            } else {
                (None, None)
            };
            let output_cells = u64::from(self.recipe.dimension)
                .checked_mul(u64::from(self.recipe.max_vectors))
                .ok_or(IndexError::ResourceLimited)?;
            let max_vector_id_bytes = MAX_VECTOR_ID
                .checked_mul(4)
                .and_then(|bytes| bytes.checked_add(128))
                .ok_or(IndexError::ResourceLimited)?;
            let max_vector_id_bytes = u64::try_from(max_vector_id_bytes)
                .map_err(|_conversion_error| IndexError::ResourceLimited)?;
            let max_encoded = output_cells
                .checked_mul(24)
                .and_then(|bytes| {
                    u64::from(self.recipe.max_vectors)
                        .checked_mul(max_vector_id_bytes)
                        .and_then(|ids| bytes.checked_add(ids))
                })
                .and_then(|bytes| bytes.checked_add(1024))
                .ok_or(IndexError::ResourceLimited)?;
            context.budget.charge(WorkUsage {
                bytes: max_encoded,
                ..WorkUsage::default()
            })?;
            let vectors = if let Some(vectors) = reusable {
                vectors
            } else {
                if let Some(old) = prior_artifact.as_ref() {
                    chunk_materialization::charge_chunk_reconciliation(
                        payload_len,
                        old.chunks.len(),
                        chunk_plan.as_ref().map_or(0, |plan| plan.members.len()),
                        self.recipe.dimension,
                        context.budget,
                    )?;
                }
                let selected_input = EmbeddingInput {
                    owner,
                    binding: input,
                    source: source.as_deref(),
                    source_bytes: &bytes.payload,
                    recipe: &self.recipe,
                    request_budget: context.budget.request(),
                };
                if let Some(old) = prior_artifact
                    .as_ref()
                    .filter(|_old| self.recipe.uses_chunks())
                {
                    let prior = EmbeddingPrior::new(
                        old.payload_ref,
                        old.reuse_identity,
                        &old.chunks,
                        &old.vectors,
                    );
                    self.provider
                        .embed_with_prior(selected_input, prior)
                        .await?
                } else {
                    self.provider.embed(selected_input).await?
                }
            };
            // Returned coordinates own their backing and leases. Prior JSON
            // custody is no longer needed during validation of the new output.
            drop(prior_artifact);
            context.budget.checkpoint()?;
            verify_provider_recipe(&self.provider, &self.recipe).map_err(|_descriptor_error| {
                IndexError::BackendViolation(
                    "embedding provider descriptor changed during call".into(),
                )
            })?;
            if self
                .provider
                .payload_reuse_identity(&self.recipe, payload_ref)?
                != reuse_identity
            {
                return Err(IndexError::BackendViolation(
                    "embedding provider reuse identity changed during call".into(),
                ));
            }
            validate_vectors(&vectors, &self.recipe, context.budget)?;
            if let Some(plan) = &chunk_plan {
                validate_chunk_bijection(&plan.members, &vectors)?;
            }
            let (chunks, membership_memory) = match chunk_plan {
                Some(plan) => (plan.members, plan.memory),
                None => (Vec::new(), None),
            };
            let parts = vectors.into_parts();
            let output_memory = parts.1;
            let vectors = parts.0;
            Ok(SemanticCandidate {
                instance: instance.instance,
                format: instance.format,
                artifact: OwnerArtifact {
                    owner,
                    input,
                    recipe: fingerprint,
                    payload_ref,
                    reuse_identity,
                    chunks,
                    vectors,
                },
                _output_memory: output_memory,
                _membership_memory: membership_memory,
            })
        })
    }

    fn verify_owner<'a>(
        &'a self,
        candidate: Self::Candidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, OwnerMemberBinding> {
        Box::pin(async move {
            // Capture the complete owner. Disjoint async field capture would
            // otherwise leave the unseen vector leases outside this future.
            let candidate_owner = candidate;
            context.budget.checkpoint()?;
            if candidate_owner.artifact.recipe != self.recipe.fingerprint()
                || candidate_owner.artifact.input.derivation_policy != self.recipe.fingerprint()
                || match self.recipe.input_policy {
                    EmbeddingInputPolicy::SourceUtf8 => {
                        candidate_owner.artifact.input.projection.is_some()
                    }
                    EmbeddingInputPolicy::ProjectionUtf8 { .. }
                    | EmbeddingInputPolicy::ProjectionChunks { .. } => {
                        candidate_owner.artifact.input.projection.is_none()
                    }
                }
            {
                return Err(IndexError::BackendViolation(
                    "recipe changed after prepare".into(),
                ));
            }
            validate_vectors(
                &candidate_owner.artifact.vectors,
                &self.recipe,
                context.budget,
            )?;
            validate_chunk_artifact(
                context.objects,
                &candidate_owner.artifact,
                &self.recipe,
                context.budget,
            )?;
            let payload = encode_owner_artifact(&candidate_owner.artifact, context.budget)?;
            let children = artifact_children(&candidate_owner.artifact);
            let artifact = context.put(&payload.bytes, children.as_ref())?;
            Ok(OwnerMemberBinding {
                instance: candidate_owner.instance,
                owner: candidate_owner.artifact.owner,
                input: candidate_owner.artifact.input,
                format: candidate_owner.format,
                artifact,
                logical_commitment: commitment(&payload.bytes),
            })
        })
    }
}

fn verify_provider_recipe(
    provider: &dyn EmbeddingProvider,
    recipe: &EmbeddingRecipe,
) -> Result<(), IndexError> {
    // Called before and after embedding. This detects visible descriptor drift;
    // the provider contract remains responsible for computation provenance
    // within a call, including transient ABA changes.
    if provider.model_identity() != recipe.model_identity
        || provider.dimension() != recipe.dimension
        || provider.normalization() != recipe.normalization
        || provider.output_policy() != recipe.output_policy
    {
        return Err(IndexError::InvalidInput(
            "embedding provider does not match pinned recipe".into(),
        ));
    }
    Ok(())
}

fn commitment(payload: &[u8]) -> Digest {
    let mut hash = Sha256::new();
    hash.update(COMMIT_DOMAIN);
    hash.update(payload);
    Digest(hash.finalize().into())
}

enum ArtifactChildren {
    Empty,
    One([ObjectRef; 1]),
    Two([ObjectRef; 2]),
    Three([ObjectRef; 3]),
}

impl AsRef<[ObjectRef]> for ArtifactChildren {
    fn as_ref(&self) -> &[ObjectRef] {
        match self {
            Self::Empty => &[],
            Self::One(values) => values,
            Self::Two(values) => values,
            Self::Three(values) => values,
        }
    }
}

fn artifact_children(artifact: &OwnerArtifact) -> ArtifactChildren {
    let mut values = [
        artifact.payload_ref,
        artifact.input.source.unwrap_or(artifact.payload_ref),
        artifact.input.projection.unwrap_or(artifact.payload_ref),
    ];
    values.sort_unstable();
    match values {
        [first, _, last] if first == last => ArtifactChildren::One([first]),
        [first, second, last] if first == second || second == last => {
            ArtifactChildren::Two([first, last])
        }
        values => ArtifactChildren::Three(values),
    }
}

fn load_owner_artifact(
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
    member: &OwnerMemberBinding,
    recipe: &EmbeddingRecipe,
) -> Result<LoadedOwnerArtifact, IndexError> {
    load_owner_artifact_scoped(objects, budget, member, recipe, MemoryLifetime::Retained)
}
fn load_query_owner_artifact(
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
    member: &OwnerMemberBinding,
    recipe: &EmbeddingRecipe,
) -> Result<LoadedOwnerArtifact, IndexError> {
    load_owner_artifact_scoped(objects, budget, member, recipe, MemoryLifetime::Temporary)
}

struct LoadedOwnerArtifact {
    artifact: OwnerArtifact,
    // Partial decoding and moved-vector iteration keep this allowance alive.
    decode_memory: Option<MemoryLease>,
}

impl std::ops::Deref for LoadedOwnerArtifact {
    type Target = OwnerArtifact;
    fn deref(&self) -> &Self::Target {
        &self.artifact
    }
}

fn load_owner_artifact_scoped(
    objects: &dyn ImmutableObjectPort,
    budget: &mut WorkBudget,
    member: &OwnerMemberBinding,
    recipe: &EmbeddingRecipe,
    lifetime: MemoryLifetime,
) -> Result<LoadedOwnerArtifact, IndexError> {
    let stored = objects.get(member.artifact, budget)?;
    if stored.registration.reference != member.artifact
        || commitment(&stored.payload) != member.logical_commitment
    {
        return Err(IndexError::CorruptObject(
            "owner vector commitment mismatch".into(),
        ));
    }
    // Covers geometric vector/ID backing and JSON parser scratch, including
    // malformed over-bound arrays before the independent recipe validation.
    let memory = reserve_json_decode(stored.payload.len(), budget, lifetime)?;
    let artifact: OwnerArtifact = serde_json::from_slice(&stored.payload)
        .map_err(|error| IndexError::CorruptObject(format!("owner vectors: {error}")))?;
    budget.checkpoint()?;
    let loaded = LoadedOwnerArtifact {
        artifact,
        decode_memory: memory,
    };
    validate_owner_artifact(&stored, &loaded.artifact, member, recipe, budget)?;
    validate_chunk_artifact(objects, &loaded.artifact, recipe, budget)?;
    Ok(loaded)
}

fn validate_owner_artifact(
    stored: &quanta_index_core::index::StoredObject,
    artifact: &OwnerArtifact,
    member: &OwnerMemberBinding,
    recipe: &EmbeddingRecipe,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let mut children = [
        Some(artifact.payload_ref),
        artifact.input.source,
        artifact.input.projection,
    ];
    children.sort_unstable();
    let mut prior = None;
    let children = children.into_iter().flatten().filter(|child| {
        let distinct = prior != Some(*child);
        prior = Some(*child);
        distinct
    });
    if artifact.owner != member.owner
        || artifact.input != member.input
        || artifact.recipe != recipe.fingerprint()
        || !stored.registration.children.iter().copied().eq(children)
    {
        return Err(IndexError::CorruptObject(
            "owner artifact binding mismatch".into(),
        ));
    }
    validate_vectors(&artifact.vectors, recipe, budget)?;
    Ok(())
}

fn validate_vectors(
    vectors: &[EmbeddingVector],
    recipe: &EmbeddingRecipe,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    // This persisted policy promises one vector from the complete UTF-8 input.
    // Enforce its shape at prepare, verification and artifact readback; a
    // provider cannot turn omitted output into a successfully covered owner.
    if recipe.output_policy == whole_file_output_policy()
        && (vectors.len() != 1
            || vectors
                .first()
                .is_none_or(|vector| vector.id.as_slice() != b"whole-file"))
    {
        return Err(IndexError::BackendViolation(
            "provider violated the canonical whole-file vector policy".into(),
        ));
    }
    if vectors.len()
        > usize::try_from(recipe.max_vectors)
            .map_err(|_conversion_error| IndexError::ResourceLimited)?
    {
        return Err(IndexError::BackendViolation(
            "provider exceeded vector bound".into(),
        ));
    }
    let mut previous: Option<&[u8]> = None;
    for vector in vectors {
        budget.charge(WorkUsage {
            mutations: 1,
            ..WorkUsage::default()
        })?;
        if vector.id.is_empty()
            || vector.id.len() > MAX_VECTOR_ID
            || previous.is_some_and(|id| id >= vector.id.as_slice())
            || vector.values.len()
                != usize::try_from(recipe.dimension)
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?
        {
            return Err(IndexError::BackendViolation(
                "provider returned invalid vector identity or dimension".into(),
            ));
        }
        previous = Some(&vector.id);
        let norm = vector.values.iter().try_fold(0.0f64, |sum, value| {
            if !value.is_finite() {
                return Err(IndexError::BackendViolation("nonfinite vector".into()));
            }
            Ok(f64::from(*value).mul_add(f64::from(*value), sum))
        })?;
        if !norm.is_finite() || norm <= 0.0 {
            return Err(IndexError::BackendViolation(
                "zero or invalid vector".into(),
            ));
        }
        if recipe.normalization == VectorNormalization::L2Unit && (norm.sqrt() - 1.0).abs() > 1e-3 {
            return Err(IndexError::BackendViolation(
                "provider violated unit norm".into(),
            ));
        }
    }
    Ok(())
}

/// Bounded server policy for one registered semantic reader. Clients supply
/// query text and `top_k`; they cannot choose a recipe or execution need.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeSemanticReadConfig {
    pub page_size: u32,
    pub max_owners: u32,
    pub max_query_bytes: usize,
    pub need: ResourceNeed,
}

impl NativeSemanticReadConfig {
    pub fn validate(self) -> Result<(), IndexError> {
        if self.page_size == 0
            || self.page_size > MAX_PAGE
            || self.max_owners == 0
            || self.max_owners > MAX_OWNER_SCAN
            || self.max_query_bytes == 0
            || self.max_query_bytes > MAX_QUERY_BYTES
            || self.need.cpu_units == 0
            || self.need.io_bytes == 0
            || self.need.memory_bytes == 0
            || self.need.provider_units == 0
        {
            return Err(IndexError::InvalidInput(
                "native semantic read policy exceeds bounded service limits".into(),
            ));
        }
        Ok(())
    }
}

pub struct NativeSemanticReader<P> {
    bridge: Arc<TextEmbeddingBridge<P>>,
    recipe: EmbeddingRecipe,
    config: NativeSemanticReadConfig,
    ann_runtime: Option<Arc<tokio::runtime::Runtime>>,
}

impl<P: TextEmbeddingProvider> NativeSemanticReader<P> {
    pub fn new(
        bridge: Arc<TextEmbeddingBridge<P>>,
        recipe: EmbeddingRecipe,
        config: NativeSemanticReadConfig,
    ) -> Result<Self, IndexError> {
        recipe.validate()?;
        config.validate()?;
        verify_provider_recipe(bridge.as_ref(), &recipe)?;
        Ok(Self {
            bridge,
            recipe,
            config,
            ann_runtime: None,
        })
    }

    /// Use the product region layout; the caller owns the bounded async runtime.
    #[must_use]
    pub fn with_ann(mut self, runtime: Arc<tokio::runtime::Runtime>) -> Self {
        self.ann_runtime = Some(runtime);
        self
    }

    fn validate_request(&self, request: &SemanticReadRequest) -> Result<(), IndexError> {
        if request.overlay.len()
            > usize::try_from(self.config.max_owners)
                .map_err(|_error| IndexError::ResourceLimited)?
        {
            return Err(IndexError::ResourceLimited);
        }
        request.validate_lexical_scope()?;
        let request = &request.query;
        if request.query_text.trim().is_empty()
            || request.query_text.len() > self.config.max_query_bytes
            || request.top_k == 0
            || request.top_k > MAX_TOP_K
            || !request.constraints.language_any_of.is_empty()
            || request.generation.is_some()
            || request.generation_selector.is_some()
        {
            return Err(IndexError::InvalidInput(
                "native semantic supports bounded query text and exact source-path constraints"
                    .into(),
            ));
        }
        // The same input matrix applies when an empty selected universe skips
        // embedding. Reuse the existing authority instead of inferring tokens.
        quanta_index_core::SemanticAdmissionEngine::admit_input_text(
            quanta_index_core::SemanticInputClass::QueryText,
            &request.query_text,
        )
        .map_err(|error| IndexError::InvalidInput(error.to_string()))?;
        Ok(())
    }
}

impl<P: TextEmbeddingProvider> IndexQueryPort<SemanticReadRequest, SearchResult>
    for NativeSemanticReader<P>
{
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        validate_semantic_instance(self.bridge.as_ref(), &self.recipe, instance)
    }

    fn reservation(&self, request: &SemanticReadRequest) -> Result<ResourceNeed, IndexError> {
        self.validate_request(request)?;
        request.validate_overlay(&self.recipe, None)?;
        Ok(self.config.need)
    }

    fn query(
        &self,
        request: &SemanticReadRequest,
        context: &mut QueryReadContext<'_>,
    ) -> Result<QueryOutcome<SearchResult>, IndexError> {
        context.budget.checkpoint()?;
        self.validate_request(request)?;
        request.validate_overlay(&self.recipe, Some(context.budget))?;
        self.validate_instance(context.instance)?;
        let mut read = ReadContext {
            objects: context.objects,
            maps: context.maps,
            budget: &mut *context.budget,
        };
        let (coverage, projection) = load_selected_search(
            &context.ticket.view,
            context.instance,
            context.instance.format,
            &self.recipe,
            &mut read,
        )?;
        let lexical = lexical_scope::prepare(
            request,
            context.ticket.view.identity,
            &coverage,
            projection.as_ref(),
            context.ticket.view.source.owners,
            &self.recipe,
            self.config.max_owners,
            &mut read,
        )?;
        if request.query.constraints.repo_relative_path_exact.is_some()
            && (projection.is_none() || self.ann_runtime.is_some())
            && matches!(coverage.status, CoverageStatus::Complete)
        {
            validate_path_roots(
                context.ticket.view.source.owners,
                context.ticket.view.source.paths,
            )?;
        }
        if self.ann_runtime.is_some() {
            if coverage.owners.0.is_some() && coverage.searchable == coverage.owners.0 {
                return Err(IndexError::RebuildRequired);
            }
        } else if coverage.searchable != coverage.owners.0 {
            return Err(IndexError::RebuildRequired);
        }
        if lexical
            .as_ref()
            .is_some_and(|scope| scope.owners.is_empty() && scope.overlay_owners == 0)
        {
            self.bridge.verify_descriptor()?;
            read.budget.checkpoint()?;
            let exhaustive = matches!(coverage.status, CoverageStatus::Complete)
                && coverage.covered == Some(coverage.desired);
            return QueryOutcome::async_ranked(
                context.ticket,
                context.instance,
                &coverage,
                exhaustive,
                SearchResult::from_parts(
                    Vec::new(),
                    0,
                    0,
                    NativeSemanticTraversal::ExactExhausted,
                    exhaustive,
                    None,
                ),
            );
        }
        if request.overlay.is_empty()
            && matches!(coverage.status, CoverageStatus::Complete)
            && coverage.covered == Some(coverage.desired)
            && coverage.desired.root == MapRoot(None)
            && coverage.owners == MapRoot(None)
        {
            // The selected scope has no owner vectors. Preserve live provider
            // descriptor and request gates without paying for an embedding.
            self.bridge.verify_descriptor()?;
            read.budget.checkpoint()?;
            return QueryOutcome::async_ranked(
                context.ticket,
                context.instance,
                &coverage,
                true,
                SearchResult::from_parts(
                    Vec::new(),
                    0,
                    0,
                    NativeSemanticTraversal::ExactExhausted,
                    true,
                    None,
                ),
            );
        }
        let vector = self
            .bridge
            .embed_query(&request.query.query_text, read.budget.request())?;
        let query = SearchQuery {
            lexical_eligibility: request.lexical_eligibility.as_ref(),
            lexical_owners: lexical.as_ref().map(|scope| scope.owners.as_slice()),
            exact_path: request
                .query
                .constraints
                .repo_relative_path_exact
                .as_ref()
                .map(quanta_index_contract::ExactRepoRelativePathV1::as_str),
            coverage: &coverage,
            projection,
            source_owners: context.ticket.view.source.owners,
            source_paths: context.ticket.view.source.paths,
            instance: context.instance.instance,
            format: context.instance.format,
            recipe: &self.recipe,
            vector: vector
                .first()
                .ok_or_else(|| IndexError::BackendViolation("query vector missing".into()))?,
            top_k: request.query.top_k,
            page_size: self.config.page_size,
            max_owners: self.config.max_owners,
            overlay: Some(overlay::OverlaySearch {
                documents: &request.overlay,
                bridge: self.bridge.as_ref(),
            }),
        };
        let result = if let Some(runtime) = &self.ann_runtime {
            crate::run_blocking(runtime, ann_query::search(&query, &mut read))?
        } else {
            if coverage.searchable != coverage.owners.0 {
                return Err(IndexError::RebuildRequired);
            }
            match lexical_scope::search(&query, &mut read)? {
                Some(result) => result,
                None => match search_selected_exact_path(&query, &mut read)? {
                    Some(result) => result,
                    None => search_exact(&query, &mut read)?,
                },
            }
        };
        let exhaustive = result.exhaustive;
        QueryOutcome::async_ranked(
            context.ticket,
            context.instance,
            &coverage,
            exhaustive,
            result,
        )
    }
}

struct SearchQuery<'a> {
    pub lexical_eligibility: Option<&'a SemanticLexicalEligibility>,
    pub lexical_owners: Option<&'a [selected_owner::SelectedOwner]>,
    pub exact_path: Option<&'a str>,
    pub coverage: &'a CoverageBinding,
    pub projection: Option<ProjectionBinding>,
    pub source_owners: MapRoot,
    pub source_paths: MapRoot,
    pub instance: IndexInstanceId,
    pub format: StorageFormatId,
    pub recipe: &'a EmbeddingRecipe,
    pub vector: &'a [f32],
    pub top_k: u32,
    pub page_size: u32,
    pub max_owners: u32,
    pub overlay: Option<overlay::OverlaySearch<'a>>,
}

pub struct ReadContext<'a> {
    pub objects: &'a dyn ImmutableObjectPort,
    pub maps: &'a dyn PersistentMapPort,
    pub budget: &'a mut WorkBudget,
}

/// The public exact-search entry point acquires a catalog lease and resolves
/// coverage from that exact view. The lease remains live through all reads.
#[derive(Clone, Copy)]
pub struct TicketSearchQuery<'a> {
    pub selection: ViewSelection,
    pub instance: IndexInstanceId,
    pub format: StorageFormatId,
    pub recipe: &'a EmbeddingRecipe,
    pub vector: &'a [f32],
    pub top_k: u32,
    pub page_size: u32,
    pub max_owners: u32,
}

pub fn search_exact_acquired(
    catalog: &dyn CatalogReadPort,
    session: &dyn ReadSessionLease,
    ticket_lease: Duration,
    query: TicketSearchQuery<'_>,
    context: &mut ReadContext<'_>,
) -> Result<SearchResult, IndexError> {
    context.budget.checkpoint()?;
    let lease = SearchLease {
        catalog,
        session,
        ticket: catalog.acquire_ticket(session, query.selection, ticket_lease, context.budget)?,
        released: false,
    };
    let result = catalog
        .validate_ticket(session, lease.ticket.lease)
        .and_then(|()| search_exact_at_view(&lease.ticket.view, &query, context))
        .and_then(|result| {
            catalog.validate_ticket(session, lease.ticket.lease)?;
            Ok(result)
        });
    lease.release()?;
    result
}

// This borrow keeps the injected catalog alive and releases on panic/unwind,
// as well as the explicit result paths. A failed release conservatively keeps
// the catalog pin rather than making reclamation unsafe.
struct SearchLease<'a> {
    catalog: &'a dyn CatalogReadPort,
    session: &'a dyn ReadSessionLease,
    ticket: quanta_index_core::index::TicketRecord,
    released: bool,
}

impl SearchLease<'_> {
    fn release(mut self) -> Result<(), IndexError> {
        let result = self.catalog.release_ticket(self.session, self.ticket.lease);
        self.released = result.is_ok();
        result
    }
}

impl Drop for SearchLease<'_> {
    fn drop(&mut self) {
        if !self.released {
            drop(self.catalog.release_ticket(self.session, self.ticket.lease));
        }
    }
}

fn search_exact_at_view(
    view: &quanta_index_core::index::CatalogView,
    query: &TicketSearchQuery<'_>,
    context: &mut ReadContext<'_>,
) -> Result<SearchResult, IndexError> {
    context.budget.checkpoint()?;
    if view
        .identity_digest()
        .map_err(|error| IndexError::CorruptObject(format!("view identity encoding: {error}")))?
        != view.identity.digest
    {
        return Err(IndexError::CorruptObject(
            "catalog view identity mismatch".into(),
        ));
    }
    let selected = match query.selection {
        ViewSelection::Exact(expected) => view.identity == expected,
        ViewSelection::Latest { workspace, epoch } => {
            view.identity.workspace == workspace && view.identity.epoch == epoch
        }
    };
    if !selected {
        return Err(IndexError::BackendViolation(
            "catalog ticket did not match requested view selection".into(),
        ));
    }
    let profile_object = context.objects.get(view.profile, context.budget)?;
    if profile_object.registration.reference != view.profile {
        return Err(IndexError::CorruptObject(
            "profile reference mismatch".into(),
        ));
    }
    let profile: IndexProfile = serde_json::from_slice(&profile_object.payload)
        .map_err(|error| IndexError::CorruptObject(format!("profile: {error}")))?;
    let spec = profile
        .instances
        .iter()
        .find(|spec| spec.instance == query.instance)
        .ok_or(IndexError::SnapshotUnavailable)?;
    let (coverage, projection) =
        load_selected_search(view, spec, query.format, query.recipe, context)?;
    search_exact(
        &SearchQuery {
            lexical_eligibility: None,
            lexical_owners: None,
            exact_path: None,
            coverage: &coverage,
            projection,
            source_owners: view.source.owners,
            source_paths: view.source.paths,
            instance: query.instance,
            format: query.format,
            recipe: query.recipe,
            vector: query.vector,
            top_k: query.top_k,
            page_size: query.page_size,
            max_owners: query.max_owners,
            overlay: None,
        },
        context,
    )
}

/// Resolve only the selected ticket's coverage and producer provenance. The
/// kernel already resolved the stored profile for a registered read port.
fn load_selected_search(
    view: &quanta_index_core::index::CatalogView,
    spec: &InstanceSpec,
    format: StorageFormatId,
    recipe: &EmbeddingRecipe,
    context: &mut ReadContext<'_>,
) -> Result<(CoverageBinding, Option<ProjectionBinding>), IndexError> {
    context.budget.checkpoint()?;
    if view
        .identity_digest()
        .map_err(|error| IndexError::CorruptObject(format!("view identity encoding: {error}")))?
        != view.identity.digest
    {
        return Err(IndexError::CorruptObject(
            "catalog view identity mismatch".into(),
        ));
    }
    if spec.activation != GroupActivation::AsyncCoverage
        || spec.format != format
        || spec.policy != recipe.fingerprint()
    {
        return Err(IndexError::InvalidInput(
            "semantic instance differs from ticket profile".into(),
        ));
    }
    let key = member_map_key(spec.instance);
    let coverage_ref = context
        .maps
        .lookup_object(view.coverage, &key, context.budget)?
        .ok_or(IndexError::SnapshotUnavailable)?;
    let stored = context.objects.get(coverage_ref, context.budget)?;
    if stored.registration.reference != coverage_ref {
        return Err(IndexError::CorruptObject(
            "coverage reference mismatch".into(),
        ));
    }
    let coverage =
        quanta_index_core::index::decode_coverage_binding(&stored.payload, context.budget)?;
    let missing = match coverage.status {
        CoverageStatus::Partial { missing } => missing.0,
        CoverageStatus::Pending { previous_missing } => previous_missing.0,
        CoverageStatus::Complete => None,
    };
    let children = ObjectChildren::from_references([
        coverage.desired.root.0,
        coverage.owners.0,
        missing,
        coverage.searchable,
    ]);
    // Pending provenance can refer to an older source cut, but the coverage
    // envelope must still match the selected profile and immutable object.
    if coverage.instance != spec.instance
        || coverage.desired.schema != spec.schema
        || coverage.desired.policy != spec.policy
        || stored.registration.children.as_slice() != children.as_slice()
    {
        return Err(IndexError::CorruptObject(
            "ticket coverage binding mismatch".into(),
        ));
    }
    if coverage.searchable != coverage.owners.0 {
        let reference = coverage
            .searchable
            .ok_or_else(|| IndexError::CorruptObject("semantic search directory absent".into()))?;
        let directory =
            ann_directory::AnnDirectory::load(reference, recipe.fingerprint(), context)?;
        if directory.owners != coverage.owners {
            return Err(IndexError::CorruptObject(
                "semantic directory selects different owners".into(),
            ));
        }
    }
    let projection = match (recipe.input_policy, spec.input_domain) {
        (EmbeddingInputPolicy::SourceUtf8, InputDomain::SourceLocal) => None,
        (
            EmbeddingInputPolicy::ProjectionUtf8 {
                projection,
                producer,
                schema,
            }
            | EmbeddingInputPolicy::ProjectionChunks {
                projection,
                producer,
                schema,
            },
            InputDomain::Projection {
                projection: actual_projection,
                producer: actual_producer,
            },
        ) if projection == actual_projection
            && producer == actual_producer
            && schema == spec.schema =>
        {
            let mut key = [0; 27];
            let (prefix, id) = key.split_at_mut(11);
            prefix.copy_from_slice(b"projection\0");
            id.copy_from_slice(&projection.0);
            let reference = context
                .maps
                .lookup_object(view.projections, &key, context.budget)?
                .ok_or(IndexError::SnapshotUnavailable)?;
            let object = context.objects.get(reference, context.budget)?;
            let binding = quanta_index_core::index::decode_projection_binding(
                &object.payload,
                context.budget,
            )?;
            let mut children = [
                Some(binding.certificate),
                binding.owners.0,
                binding.source.owners.0,
                binding.source.paths.0,
                binding.source.frontiers.0,
            ];
            children.sort_unstable();
            let mut previous = None;
            let children = children.into_iter().flatten().filter(|reference| {
                let fresh = previous != Some(*reference);
                previous = Some(*reference);
                fresh
            });
            if object.registration.reference != reference
                || !object.registration.children.iter().copied().eq(children)
                || binding.projection != projection
                || binding.producer != producer
                || binding.schema != schema
                || (binding.source != view.source
                    && !matches!(coverage.status, CoverageStatus::Pending { .. }))
            {
                return Err(IndexError::CorruptObject(
                    "ticket projection binding mismatch".into(),
                ));
            }
            Some(binding)
        }
        _ => {
            return Err(IndexError::InvalidInput(
                "semantic input policy differs from ticket profile".into(),
            ));
        }
    };
    if projection.is_some() && matches!(coverage.status, CoverageStatus::Pending { .. }) {
        return Err(IndexError::SnapshotUnavailable);
    }
    let desired_root = projection.map_or(view.source.owners, |binding| binding.owners);
    let desired_context = projection.as_ref().map(projection_context);
    if coverage.desired.root != desired_root
        || desired_context.is_some_and(|context| coverage.desired.context != context)
    {
        return Err(IndexError::CorruptObject(
            "ticket coverage binding mismatch".into(),
        ));
    }
    Ok((coverage, projection))
}

#[derive(Debug)]
pub struct SearchHit {
    pub owner: OwnerKey,
    pub vector_id: Vec<u8>,
    pub cosine: f64,
    /// Selected input membership; spans are UTF-8 payload byte coordinates.
    /// A projection span is not silently relabeled as a source-file coordinate.
    pub chunk: Option<NativeSemanticChunk>,
    /// Request-local editor text provenance; never a durable source object.
    pub overlay: Option<SemanticOverlayHit>,
    vector_id_memory: Option<MemoryLease>,
}

impl PartialEq for SearchHit {
    fn eq(&self, other: &Self) -> bool {
        self.owner == other.owner
            && self.vector_id == other.vector_id
            && self.cosine == other.cosine
            && self.chunk == other.chunk
            && self.overlay == other.overlay
    }
}

impl SearchHit {
    /// The producing owner admits ID backing before allocating or adopting it.
    #[must_use]
    pub fn from_parts(
        owner: OwnerKey,
        vector_id: Vec<u8>,
        cosine: f64,
        vector_id_memory: Option<MemoryLease>,
    ) -> Self {
        Self {
            owner,
            vector_id,
            cosine,
            chunk: None,
            overlay: None,
            vector_id_memory,
        }
    }

    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        OwnerKey,
        Vec<u8>,
        f64,
        Option<NativeSemanticChunk>,
        Option<MemoryLease>,
    ) {
        (
            self.owner,
            self.vector_id,
            self.cosine,
            self.chunk,
            self.vector_id_memory,
        )
    }
}

#[derive(Debug)]
pub struct SearchResult {
    pub hits: Vec<SearchHit>,
    pub scanned_owners: u32,
    pub skipped_stale: u32,
    pub traversal: NativeSemanticTraversal,
    pub exhaustive: bool,
    hit_memory: Option<MemoryLease>,
}

impl PartialEq for SearchResult {
    fn eq(&self, other: &Self) -> bool {
        self.hits == other.hits
            && self.scanned_owners == other.scanned_owners
            && self.skipped_stale == other.skipped_stale
            && self.traversal == other.traversal
            && self.exhaustive == other.exhaustive
    }
}

impl SearchResult {
    /// Caller admits hit backing before allocation; moves transfer its charge.
    #[must_use]
    pub fn from_parts(
        hits: Vec<SearchHit>,
        scanned_owners: u32,
        skipped_stale: u32,
        traversal: NativeSemanticTraversal,
        exhaustive: bool,
        hit_memory: Option<MemoryLease>,
    ) -> Self {
        Self {
            hits,
            scanned_owners,
            skipped_stale,
            traversal,
            exhaustive,
            hit_memory,
        }
    }

    #[must_use]
    pub fn into_parts(self) -> (Vec<SearchHit>, Option<MemoryLease>) {
        (self.hits, self.hit_memory)
    }
}

struct RankedHit(SearchHit);

impl PartialEq for RankedHit {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for RankedHit {}
impl PartialOrd for RankedHit {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for RankedHit {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .0
            .cosine
            .total_cmp(&self.0.cosine)
            .then_with(|| self.0.owner.cmp(&other.0.owner))
            .then_with(|| self.0.vector_id.cmp(&other.0.vector_id))
    }
}

/// Exact cosine oracle for bounded owner populations. Budget exhaustion and
/// object failures propagate; neither can be represented as an empty result.
fn validate_search_query(query: &SearchQuery<'_>) -> Result<f64, IndexError> {
    query.recipe.validate()?;
    let recipe_fingerprint = query.recipe.fingerprint();
    if query.instance != query.coverage.instance
        || query.coverage.desired.policy != recipe_fingerprint
        || query.top_k == 0
        || query.top_k > MAX_TOP_K
        || query.page_size == 0
        || query.page_size > MAX_PAGE
        || query.max_owners == 0
        || query.max_owners > MAX_OWNER_SCAN
    {
        return Err(IndexError::InvalidInput(
            "invalid exact semantic query".into(),
        ));
    }
    let query_norm = vector_norm(query.vector, query.recipe.dimension)?;
    if query.recipe.normalization == VectorNormalization::L2Unit && (query_norm - 1.0).abs() > 1e-3
    {
        return Err(IndexError::InvalidInput(
            "query vector violates declared L2Unit normalization".into(),
        ));
    }
    let global_current = query.coverage.covered == Some(query.coverage.desired);
    let declared_complete = matches!(query.coverage.status, CoverageStatus::Complete);
    if declared_complete && !global_current {
        return Err(IndexError::CorruptObject(
            "complete semantic coverage lacks exact input equality".into(),
        ));
    }
    if declared_complete
        && query.coverage.owners == MapRoot(None)
        && query.coverage.desired.root != MapRoot(None)
    {
        return Err(IndexError::CorruptObject(
            "complete semantic coverage omits source owners".into(),
        ));
    }
    Ok(query_norm)
}

fn search_exact(
    query: &SearchQuery<'_>,
    context: &mut ReadContext<'_>,
) -> Result<SearchResult, IndexError> {
    let query_norm = validate_search_query(query)?;
    let mut traversal = NativeSemanticTraversal::ExactExhausted;
    let mut scanned_owners = 0u32;
    let mut skipped_stale = 0u32;
    let overlay_rows = u32::try_from(
        query
            .overlay
            .as_ref()
            .map_or(0, |rows| rows.documents.len()),
    )
    .map_err(|_error| IndexError::ResourceLimited)?;
    let durable_limit = query
        .max_owners
        .checked_sub(overlay_rows)
        .ok_or(IndexError::ResourceLimited)?;
    let mut after: Option<[u8; 38]> = None;
    let top_k = usize::try_from(query.top_k).map_err(|_error| IndexError::ResourceLimited)?;
    let heap_bytes = top_k
        .checked_mul(std::mem::size_of::<RankedHit>())
        .ok_or(IndexError::ResourceLimited)?;
    let _heap_memory = context.budget.reserve_memory(
        u64::try_from(heap_bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let mut heap = BinaryHeap::new();
    heap.try_reserve_exact(top_k)
        .map_err(|_error| IndexError::ResourceLimited)?;
    loop {
        if durable_limit == 0 {
            if query.coverage.owners != MapRoot(None) {
                traversal = NativeSemanticTraversal::ExactCapped;
            }
            break;
        }
        context.budget.checkpoint()?;
        let page = context.maps.scan(
            query.coverage.owners,
            after.as_ref().map(<[u8; 38]>::as_slice),
            query.page_size,
            context.budget,
        )?;
        if page.entries.is_empty() && page.has_more {
            return Err(IndexError::BackendViolation(
                "map scan made no progress".into(),
            ));
        }
        let page_len = page.entries.len();
        for (offset, (key, reference)) in page.entries.iter().enumerate() {
            if scanned_owners == durable_limit {
                traversal = NativeSemanticTraversal::ExactCapped;
                break;
            }
            scanned_owners = scanned_owners
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            let selected = selected_owner::SelectedOwner::load(key, *reference, query, context)?;
            let owner = selected.coverage;
            let contribution = selected.contribution;
            if owner.covered == Some(owner.desired)
                && owner.searchable.is_none()
                && contribution.is_some_and(|value| value.output.is_none())
            {
                continue;
            }
            if owner.covered != Some(owner.desired) || owner.searchable.is_none() {
                skipped_stale = skipped_stale
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                continue;
            }
            let member = selected.current_member(query, context)?;
            if !selected.matches_exact_path(query, context)? {
                continue;
            }
            rank_member(
                &member,
                contribution,
                None,
                query,
                query_norm,
                top_k,
                &mut heap,
                context,
            )?;
            if scanned_owners == durable_limit
                && (offset.saturating_add(1) < page_len || page.has_more)
            {
                traversal = NativeSemanticTraversal::ExactCapped;
                break;
            }
        }
        if scanned_owners == durable_limit {
            if page.has_more {
                traversal = NativeSemanticTraversal::ExactCapped;
            }
            break;
        }
        if !page.has_more {
            break;
        }
        let last = page.entries.last().ok_or_else(|| {
            IndexError::BackendViolation("map scan omitted continuation key".into())
        })?;
        context.budget.charge(WorkUsage {
            bytes: u64::try_from(last.0.len())
                .map_err(|_conversion_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        after = Some(last.0.as_slice().try_into().map_err(|_error| {
            IndexError::CorruptObject("semantic owner cursor width differs".into())
        })?);
    }
    finish_ranked(
        query,
        query_norm,
        top_k,
        heap,
        scanned_owners,
        skipped_stale,
        traversal,
        context,
    )
}

fn finish_ranked(
    query: &SearchQuery<'_>,
    query_norm: f64,
    top_k: usize,
    mut heap: BinaryHeap<RankedHit>,
    mut scanned_owners: u32,
    skipped_stale: u32,
    traversal: NativeSemanticTraversal,
    context: &mut ReadContext<'_>,
) -> Result<SearchResult, IndexError> {
    let overlay_rows = if let Some(eligibility) = query.lexical_eligibility {
        eligibility.overlay_count()?
    } else {
        u32::try_from(
            query
                .overlay
                .as_ref()
                .map_or(0, |overlay| overlay.documents.len()),
        )
        .map_err(|_error| IndexError::ResourceLimited)?
    };
    if let Some(overlay) = query.overlay.as_ref() {
        overlay::rank_overlay(
            overlay::OverlaySearch {
                documents: overlay.documents,
                bridge: overlay.bridge,
            },
            query,
            query_norm,
            top_k,
            &mut heap,
            context,
        )?;
    }
    scanned_owners = scanned_owners
        .checked_add(overlay_rows)
        .ok_or(IndexError::ResourceLimited)?;
    let mut ranked = heap.into_vec();
    ranked.sort();
    let result_bytes = ranked
        .len()
        .checked_mul(std::mem::size_of::<SearchHit>())
        .ok_or(IndexError::ResourceLimited)?;
    let hit_memory = if result_bytes == 0 {
        None
    } else {
        context.budget.reserve_memory(
            u64::try_from(result_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Retained,
        )?
    };
    let complete = traversal.certifies_exhaustive(query.coverage, skipped_stale);
    let mut result = SearchResult::from_parts(
        Vec::new(),
        scanned_owners,
        skipped_stale,
        traversal,
        complete,
        hit_memory,
    );
    result
        .hits
        .try_reserve_exact(ranked.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    for hit in ranked {
        result.hits.push(hit.0);
    }
    Ok(result)
}

fn rank_member(
    member: &OwnerMemberBinding,
    contribution: Option<ProjectionOwnerContribution>,
    only: Option<&[u8]>,
    query: &SearchQuery<'_>,
    query_norm: f64,
    top_k: usize,
    heap: &mut BinaryHeap<RankedHit>,
    context: &mut ReadContext<'_>,
) -> Result<(), IndexError> {
    let recipe_fingerprint = query.recipe.fingerprint();
    let artifact =
        load_query_owner_artifact(context.objects, context.budget, member, query.recipe)?;
    if contribution.is_some_and(|value| value.output != Some(artifact.artifact.payload_ref)) {
        return Err(IndexError::CorruptObject(
            "owner artifact binding mismatch".into(),
        ));
    }
    if only.is_some_and(|id| {
        !artifact
            .vectors
            .iter()
            .any(|vector| vector.id.as_slice() == id)
    }) {
        return Err(IndexError::CorruptObject(
            "ANN candidate vector is absent from its member".into(),
        ));
    }
    if query
        .overlay
        .as_ref()
        .is_some_and(|overlay| overlay::masks(overlay.documents, member.owner))
    {
        return Ok(());
    }
    let chunk_lookup = ChunkLookup::new(&artifact.artifact.chunks, context.budget)?;
    for vector in artifact.artifact.vectors {
        if only.is_some_and(|id| id != vector.id.as_slice())
            || heap
                .iter()
                .any(|hit| hit.0.owner == member.owner && hit.0.vector_id == vector.id)
        {
            continue;
        }
        context.budget.charge(WorkUsage {
            mutations: 1,
            ..WorkUsage::default()
        })?;
        let cosine = cosine(query.vector, query_norm, &vector.values)?;
        let mut hit = RankedHit(SearchHit::from_parts(member.owner, vector.id, cosine, None));
        if let Some(member) = chunk_lookup.find(&artifact.artifact.chunks, &hit.0.vector_id)? {
            let span = match (member.location, query.recipe.input_policy) {
                (ChunkLocation::PayloadBytes { start, end }, EmbeddingInputPolicy::SourceUtf8) => {
                    NativeSemanticChunkSpan::SourcePayload {
                        start_byte: start,
                        end_byte: end,
                    }
                }
                (
                    ChunkLocation::PayloadBytes { start, end },
                    EmbeddingInputPolicy::ProjectionUtf8 { .. },
                ) => NativeSemanticChunkSpan::ProjectionPayload {
                    start_byte: start,
                    end_byte: end,
                },
                (
                    ChunkLocation::RenderedMember { ordinal },
                    EmbeddingInputPolicy::ProjectionChunks { .. },
                ) => NativeSemanticChunkSpan::RenderedMember { ordinal },
                _ => {
                    return Err(IndexError::CorruptObject(
                        "semantic member coordinate policy mismatch".into(),
                    ));
                }
            };
            let chunk = NativeSemanticChunk {
                id: member.id,
                payload: artifact.artifact.payload_ref,
                input: artifact.artifact.input,
                input_digest: member.input_digest,
                span,
            };
            chunk
                .validate_binding(&hit.0.vector_id, recipe_fingerprint)
                .map_err(|error| {
                    IndexError::CorruptObject(format!("selected chunk binding: {error}"))
                })?;
            hit.0.chunk = Some(chunk);
        }
        if heap.len() < top_k || heap.peek().is_some_and(|worst| hit < *worst) {
            // The decode allowance still owns these bytes; admit the
            // retained ID before transferring it beyond that lifetime.
            hit.0.vector_id_memory = context.budget.reserve_memory(
                u64::try_from(hit.0.vector_id.capacity())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Retained,
            )?;
            if heap.len() == top_k {
                drop(heap.pop());
            }
            heap.push(hit);
        }
    }
    Ok(())
}

fn vector_norm(values: &[f32], dimension: u32) -> Result<f64, IndexError> {
    if values.len()
        != usize::try_from(dimension).map_err(|_conversion_error| IndexError::ResourceLimited)?
    {
        return Err(IndexError::InvalidInput("vector dimension mismatch".into()));
    }
    let square = values.iter().try_fold(0.0f64, |sum, value| {
        if !value.is_finite() {
            return Err(IndexError::InvalidInput("nonfinite query vector".into()));
        }
        Ok(f64::from(*value).mul_add(f64::from(*value), sum))
    })?;
    if !square.is_finite() || square <= 0.0 {
        return Err(IndexError::InvalidInput("zero query vector".into()));
    }
    Ok(square.sqrt())
}

fn cosine(query: &[f32], query_norm: f64, candidate: &[f32]) -> Result<f64, IndexError> {
    let (dot, square) =
        query
            .iter()
            .zip(candidate)
            .fold((0.0f64, 0.0f64), |(dot, square), (q, c)| {
                (
                    f64::from(*q).mul_add(f64::from(*c), dot),
                    f64::from(*c).mul_add(f64::from(*c), square),
                )
            });
    if !square.is_finite() || square <= 0.0 {
        return Err(IndexError::CorruptObject(
            "invalid stored vector norm".into(),
        ));
    }
    let score = dot / (query_norm * square.sqrt());
    if !score.is_finite() {
        return Err(IndexError::CorruptObject("nonfinite cosine score".into()));
    }
    Ok(score.clamp(-1.0, 1.0))
}

#[cfg(test)]
mod tests {
    macro_rules! check {
        ($condition:expr $(,)?) => {
            if !$condition {
                return Err(format!("assertion failed: {}", stringify!($condition)).into());
            }
        };
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) if !(*left == *right) => {
                    return Err(format!(
                        "assertion failed: left == right; left: {left:?}, right: {right:?}"
                    )
                    .into());
                }
                _ => {}
            }
        }};
    }

    macro_rules! check_ne {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) if !(*left != *right) => {
                    return Err(format!(
                        "assertion failed: left != right; left: {left:?}, right: {right:?}"
                    )
                    .into());
                }
                _ => {}
            }
        }};
    }

    use std::{
        collections::BTreeMap,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering as AtomicOrdering},
        },
        time::{Duration, Instant},
    };

    use quanta_index_contract::{QueryConstraintSetV1, SemanticQueryRequest};
    use quanta_index_core::{
        RequestBudgetV1,
        index::{
            AttemptId, AttemptRef, CatalogView, CollectionId, GroupActivation, IndexKind,
            IndexQueryPort, InputBinding, MapBatchRequest, MapBatchWrite, MapEdit, MapPage,
            MapValue, MapWrite, ObjectBatchInput, OperationKey, OwnerCoverage, ProducerId,
            PublicationGroupId, QueryCoverage, QueryReadContext, RegisteredObject, SchemaId,
            SlotId, SourceCut, StableOwnerId, StoredObject, StreamId, TenantId, TicketId,
            TicketRecord, ViewRef, WorkLimits, WorkspaceKey, WorktreeId,
            effort::ResourceNeed,
            owner_map_key,
            ports::{ObjectCustodyPort, ObjectWriteSink},
        },
    };

    use super::*;

    mod ann_product_tests;
    mod certified_chunk_tests;
    mod chunk_tests;
    mod lexical_projected_tests;
    mod lexical_scope_tests;
    mod selected_owner_tests;

    fn test_runtime() -> Result<tokio::runtime::Runtime, std::io::Error> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
    }

    #[derive(Default)]
    struct MemoryStore(
        Mutex<BTreeMap<ObjectRef, StoredObject>>,
        Option<EncodingInstallObserver>,
    );

    struct EncodingInstallObserver {
        memory: Arc<EncodingMemory>,
        retained_bytes: u64,
        fail: bool,
    }

    struct EncodingMemory {
        capacity: u64,
        live: AtomicU64,
        reservations: AtomicUsize,
        installs: AtomicUsize,
        invalid: AtomicBool,
        cancel_on_reserve: Option<RequestBudgetV1>,
    }

    impl quanta_index_core::index::MemoryLeaseOwner for EncodingMemory {
        fn release(&self, bytes: u64) {
            if self
                .live
                .fetch_update(AtomicOrdering::SeqCst, AtomicOrdering::SeqCst, |live| {
                    live.checked_sub(bytes)
                })
                .is_err()
            {
                self.invalid.store(true, AtomicOrdering::SeqCst);
            }
        }
    }

    struct EncodingScope(Arc<EncodingMemory>);
    impl quanta_index_core::index::MemoryScopePort for EncodingScope {
        fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if lifetime != MemoryLifetime::Temporary {
                return Err(IndexError::BackendViolation(
                    "semantic JSON requested escaping capacity".into(),
                ));
            }
            let _previous = self.0.reservations.fetch_add(1, AtomicOrdering::SeqCst);
            let _previous_live = self
                .0
                .live
                .fetch_update(AtomicOrdering::SeqCst, AtomicOrdering::SeqCst, |live| {
                    live.checked_add(bytes)
                        .filter(|next| *next <= self.0.capacity)
                })
                .map_err(|_live| IndexError::ResourceLimited)?;
            if let Some(request) = &self.0.cancel_on_reserve {
                request.cancel_handle().cancel();
            }
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    impl MemoryStore {
        fn reference(payload: &[u8], children: &[ObjectRef]) -> ObjectRef {
            let mut hash = Sha256::new();
            hash.update(b"native-semantic-test-object\0");
            hash.update(payload);
            for child in children {
                hash.update((child.0).0);
            }
            ObjectRef(Digest(hash.finalize().into()))
        }

        fn insert(
            &self,
            payload: Vec<u8>,
            children: Vec<ObjectRef>,
        ) -> Result<ObjectRef, IndexError> {
            let reference = Self::reference(&payload, &children);
            let stored = StoredObject::new(
                RegisteredObject::unscoped(
                    reference,
                    u64::try_from(payload.len())
                        .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                    children,
                ),
                payload,
                None,
            );
            let mut objects = self
                .0
                .lock()
                .map_err(|_poison_error| IndexError::Storage("test lock".into()))?;
            drop(objects.insert(reference, stored));
            drop(objects);
            Ok(reference)
        }
    }

    impl ImmutableObjectPort for MemoryStore {
        fn identify(
            &self,
            payload: &[u8],
            children: &[ObjectRef],
            budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            budget.charge(WorkUsage {
                bytes: u64::try_from(payload.len())
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            Ok(RegisteredObject::unscoped(
                Self::reference(payload, children),
                u64::try_from(payload.len())
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                children.to_vec(),
            ))
        }

        fn put(
            &self,
            payload: &[u8],
            children: &[ObjectRef],
            budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            budget.charge(WorkUsage {
                writes: 1,
                bytes: u64::try_from(payload.len())
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            let reference = self.insert(payload.to_vec(), children.to_vec())?;
            Ok(self.get(reference, budget)?.registration.clone())
        }
        fn install(
            &self,
            payload: &[u8],
            children: &[ObjectRef],
            sink: &dyn ObjectWriteSink,
            budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            if let Some(observer) = &self.1 {
                let expected = u64::try_from(payload.len())
                    .map_err(|_error| IndexError::ResourceLimited)?
                    .checked_add(observer.retained_bytes)
                    .ok_or(IndexError::ResourceLimited)?;
                if observer.memory.live.load(AtomicOrdering::SeqCst) != expected {
                    return Err(IndexError::BackendViolation(
                        "candidate JSON or vector charge ended before install".into(),
                    ));
                }
                let _previous = observer
                    .memory
                    .installs
                    .fetch_add(1, AtomicOrdering::SeqCst);
                if observer.fail {
                    return Err(IndexError::Storage(
                        "injected candidate install failure".into(),
                    ));
                }
            }
            let registration = self.put(payload, children, budget)?;
            sink.reserve(&registration)?;
            sink.installed(&registration)?;
            Ok(registration)
        }
        fn install_batch(
            &self,
            inputs: &[ObjectBatchInput],
            sink: &dyn ObjectWriteSink,
            budget: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::RegisteredObjectBatch, IndexError> {
            if inputs.len() > 64 || self.1.is_some() {
                return Err(IndexError::BackendViolation(
                    "test batch exceeds fixture contract".into(),
                ));
            }
            let rows: Vec<_> = inputs
                .iter()
                .map(|input| self.identify(&input.payload, &input.children, budget))
                .collect::<Result<_, _>>()?;
            sink.reserve_batch(&rows)?;
            for (input, expected) in inputs.iter().zip(&rows) {
                let installed = self.put(&input.payload, &input.children, budget)?;
                if installed.reference != expected.reference {
                    return Err(IndexError::BackendViolation(
                        "test batch identity differs".into(),
                    ));
                }
            }
            sink.installed_batch(&rows)?;
            Ok(quanta_index_core::index::RegisteredObjectBatch::unscoped(
                rows,
            ))
        }
        fn get(
            &self,
            reference: ObjectRef,
            budget: &mut WorkBudget,
        ) -> Result<StoredObject, IndexError> {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            let objects = self
                .0
                .lock()
                .map_err(|_poison_error| IndexError::Storage("test lock".into()))?;
            objects
                .get(&reference)
                .cloned()
                .ok_or_else(|| IndexError::CorruptObject("test object missing".into()))
        }
    }

    type MapEntries = Vec<(Vec<u8>, ObjectRef)>;
    type RootedMapEntries = Vec<(MapRoot, MapEntries)>;
    type RootedInlineMapEntries = Vec<(MapRoot, Vec<(Vec<u8>, MapValue)>)>;

    #[derive(Default)]
    struct TestMap {
        entries: MapEntries,
        rooted_entries: RootedMapEntries,
        inline_rooted_entries: RootedInlineMapEntries,
        fail: bool,
    }

    impl PersistentMapPort for TestMap {
        fn apply_stream(
            &self,
            _: &[MapRoot],
            _: &mut dyn quanta_index_core::index::ports::MapEditCursor,
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::StagedMapRoots, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test streamed map write".into(),
            ))
        }
        fn lookup_object(
            &self,
            root: MapRoot,
            key: &[u8],
            budget: &mut WorkBudget,
        ) -> Result<Option<ObjectRef>, IndexError> {
            self.lookup(root, key, budget)?
                .map(MapValue::object_ref)
                .transpose()
        }
        fn lookup(
            &self,
            root: MapRoot,
            key: &[u8],
            budget: &mut WorkBudget,
        ) -> Result<Option<MapValue>, IndexError> {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            if root.0.is_none() {
                return Ok(None);
            }
            if let Some((_, entries)) = self
                .inline_rooted_entries
                .iter()
                .find(|(entry_root, _)| *entry_root == root)
            {
                return Ok(entries
                    .iter()
                    .find(|(entry, _)| entry == key)
                    .map(|(_, value)| *value));
            }
            let entries = self
                .rooted_entries
                .iter()
                .find(|(entry_root, _entries)| *entry_root == root)
                .map_or(self.entries.as_slice(), |(_entry_root, entries)| {
                    entries.as_slice()
                });
            Ok(entries
                .iter()
                .find(|(entry, _)| entry == key)
                .map(|(_, reference)| MapValue::Object(*reference)))
        }

        fn scan(
            &self,
            root: MapRoot,
            after: Option<&[u8]>,
            limit: u32,
            budget: &mut WorkBudget,
        ) -> Result<MapPage, IndexError> {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            if self.fail {
                return Err(IndexError::Storage("injected scan failure".into()));
            }
            if root.0.is_none() {
                return Ok(MapPage::new(Vec::new(), false, Vec::new(), None));
            }
            if self
                .inline_rooted_entries
                .iter()
                .any(|(entry_root, _)| *entry_root == root)
            {
                return Err(IndexError::BackendViolation(
                    "test inline-root scan is unsupported".into(),
                ));
            }
            let entries = self
                .rooted_entries
                .iter()
                .find(|(entry_root, _entries)| *entry_root == root)
                .map_or(self.entries.as_slice(), |(_entry_root, entries)| {
                    entries.as_slice()
                });
            let filtered: Vec<_> = entries
                .iter()
                .filter(|(key, _)| after.is_none_or(|after| key.as_slice() > after))
                .cloned()
                .collect();
            let limit =
                usize::try_from(limit).map_err(|_conversion_error| IndexError::ResourceLimited)?;
            let has_more = filtered.len() > limit;
            Ok(MapPage::new(
                filtered
                    .into_iter()
                    .take(limit)
                    .map(|(key, value)| (key, value.into()))
                    .collect(),
                has_more,
                Vec::new(),
                None,
            ))
        }
        fn apply(
            &self,
            _base: MapRoot,
            _edits: &[MapEdit],
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<MapWrite, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test map write".into(),
            ))
        }
        fn apply_batch(
            &self,
            _requests: &[MapBatchRequest],
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<MapBatchWrite, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test map batch write".into(),
            ))
        }
    }

    struct TestCustody;
    impl ObjectCustodyPort for TestCustody {
        fn reserve_objects(
            &self,
            _attempt: AttemptRef,
            _objects: &[RegisteredObject],
        ) -> Result<(), IndexError> {
            Ok(())
        }
        fn register_objects(
            &self,
            _attempt: AttemptRef,
            _objects: &[RegisteredObject],
        ) -> Result<(), IndexError> {
            Ok(())
        }
    }

    struct NoScratch;
    impl quanta_index_core::index::ScratchSpacePort for NoScratch {
        fn allocate(
            &self,
            _request: quanta_index_core::index::ScratchRequest,
            _budget: &mut WorkBudget,
        ) -> Result<Box<dyn quanta_index_core::index::ScratchAllocation>, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected semantic scratch allocation".into(),
            ))
        }
    }

    struct FixedProvider {
        calls: Arc<AtomicUsize>,
        output: Vec<EmbeddingVector>,
        descriptor: EmbeddingRecipe,
    }
    impl EmbeddingProvider for FixedProvider {
        fn model_identity(&self) -> Digest {
            self.descriptor.model_identity
        }
        fn dimension(&self) -> u32 {
            self.descriptor.dimension
        }
        fn normalization(&self) -> VectorNormalization {
            self.descriptor.normalization
        }
        fn output_policy(&self) -> Digest {
            self.descriptor.output_policy
        }
        fn embed<'a>(&'a self, input: EmbeddingInput<'a>) -> IndexFuture<'a, EmbeddingOutput> {
            let _previous = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            if input.request_budget.is_cancelled()
                || input.request_budget.deadline() <= Instant::now()
            {
                return Box::pin(async { Err(IndexError::Cancelled) });
            }
            Box::pin(async { Ok(EmbeddingOutput::unscoped(self.output.clone())) })
        }
    }

    struct DriftingProvider {
        calls: Arc<AtomicUsize>,
        descriptor: EmbeddingRecipe,
    }

    impl EmbeddingProvider for DriftingProvider {
        fn model_identity(&self) -> Digest {
            if self.calls.load(AtomicOrdering::SeqCst) == 0 {
                self.descriptor.model_identity
            } else {
                Digest([99; 32])
            }
        }
        fn dimension(&self) -> u32 {
            self.descriptor.dimension
        }
        fn normalization(&self) -> VectorNormalization {
            self.descriptor.normalization
        }
        fn output_policy(&self) -> Digest {
            self.descriptor.output_policy
        }
        fn embed<'a>(&'a self, _input: EmbeddingInput<'a>) -> IndexFuture<'a, EmbeddingOutput> {
            let _previous = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            Box::pin(async {
                Ok(EmbeddingOutput::unscoped(vec![EmbeddingVector {
                    id: b"whole-file".to_vec(),
                    values: vec![1.0, 0.0],
                }]))
            })
        }
    }

    #[test]
    fn descriptor_drift_during_embedding_does_not_prepare_an_owner()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = test_runtime()?;
        let store = MemoryStore::default();
        let maps = TestMap::default();
        let custody = TestCustody;
        let calls = Arc::new(AtomicUsize::new(0));
        let pinned = recipe();
        let adapter = NativeSemantic::new(
            DriftingProvider {
                calls: Arc::clone(&calls),
                descriptor: pinned.clone(),
            },
            pinned.clone(),
        )?;
        let (source, _) = source(&store)?;
        let mut work = budget()?;
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        };
        let input = OwnerInputBinding {
            source: Some(source),
            projection: None,
            producer_context: Digest([0; 32]),
            derivation_policy: pinned.fingerprint(),
        };
        check!(matches!(
            crate::run_blocking(
                &runtime,
                adapter.prepare_owner(&instance(&pinned), owner(1), input, None, &mut context)
            ),
            Err(IndexError::BackendViolation(_))
        ));
        check_eq!(calls.load(AtomicOrdering::SeqCst), 1);
        Ok(())
    }

    struct FixedTextProvider {
        calls: Arc<AtomicUsize>,
        deadline: Instant,
        normalization: EmbeddingNormalization,
    }

    impl TextEmbeddingProvider for FixedTextProvider {
        fn embed_batch(
            &self,
            texts: &[&str],
        ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }

        fn embed_batch_within(
            &self,
            texts: &[&str],
            budget: &RequestBudgetV1,
        ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
            if budget.deadline() != self.deadline || budget.is_cancelled() {
                return Err(CoreError::InvalidContract(
                    "original budget was not forwarded".into(),
                ));
            }
            let _previous = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            let memory = quanta_index_core::TextEmbeddingBatch::reserve_output(
                texts.len(),
                self.dimension(),
                budget,
            )?;
            let vectors = self.embed_batch(texts)?.into_unscoped()?;
            Ok(quanta_index_core::TextEmbeddingBatch::from_parts(
                vectors, memory,
            ))
        }

        fn model_id(&self) -> &'static str {
            "fixed-test-model"
        }
        fn model_revision(&self) -> &'static str {
            "revision-1"
        }
        fn dimension(&self) -> usize {
            2
        }
        fn normalization(&self) -> EmbeddingNormalization {
            self.normalization
        }
    }

    struct RefusingTextProvider {
        code: Option<SearchPlaneErrorCodeV2>,
        cancel_during_call: bool,
    }

    impl TextEmbeddingProvider for RefusingTextProvider {
        fn embed_batch(
            &self,
            _texts: &[&str],
        ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
            Err(self.code.map_or_else(
                || CoreError::InvalidContract("provider refusal".into()),
                |code| CoreError::Typed {
                    code,
                    message: "provider refusal".into(),
                },
            ))
        }

        fn embed_batch_within(
            &self,
            texts: &[&str],
            budget: &RequestBudgetV1,
        ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
            if self.cancel_during_call {
                budget.cancel_handle().cancel();
            }
            self.embed_batch(texts)
        }

        fn model_id(&self) -> &'static str {
            "refusing-test-model"
        }
        fn model_revision(&self) -> &'static str {
            "revision-1"
        }
        fn dimension(&self) -> usize {
            2
        }
        fn normalization(&self) -> EmbeddingNormalization {
            EmbeddingNormalization::L2Unit
        }
    }

    #[test]
    fn text_bridge_preserves_provider_admission_and_cancellation()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = test_runtime()?;
        let unrelated_error = CoreError::InvalidContract("provider refusal".into());
        for (code, cancel_during_call, expected) in [
            (
                Some(SearchPlaneErrorCodeV2::ServerOverloaded),
                false,
                IndexError::ResourceLimited,
            ),
            (
                Some(SearchPlaneErrorCodeV2::RequestCancelled),
                false,
                IndexError::Cancelled,
            ),
            (
                Some(SearchPlaneErrorCodeV2::ServerOverloaded),
                true,
                IndexError::Cancelled,
            ),
            (
                None,
                false,
                IndexError::Storage(format!("text embedding provider: {unrelated_error}")),
            ),
        ] {
            let bridge = TextEmbeddingBridge::new(RefusingTextProvider {
                code,
                cancel_during_call,
            })?;
            let query_request = RequestBudgetV1::for_duration(Duration::from_secs(30));
            check_eq!(
                bridge.embed_query("query", &query_request).err(),
                Some(expected.clone())
            );
            let source_request = RequestBudgetV1::for_duration(Duration::from_secs(30));
            let recipe = bridge.recipe(1024)?;
            let source = SourceRecord {
                path: "a.rs".into(),
                bytes: ObjectRef(Digest([1; 32])),
                byte_len: 6,
                text: quanta_index_contract::index::SourceTextAdmission::Excluded,
            };
            check_eq!(
                crate::run_blocking(
                    &runtime,
                    bridge.embed(EmbeddingInput {
                        owner: owner(1),
                        binding: OwnerInputBinding {
                            source: Some(source.bytes),
                            projection: None,
                            producer_context: Digest([0; 32]),
                            derivation_policy: recipe.fingerprint(),
                        },
                        source: Some(&source),
                        source_bytes: b"source",
                        recipe: &recipe,
                        request_budget: &source_request,
                    })
                )
                .err(),
                Some(expected)
            );
        }
        Ok(())
    }

    struct InterruptingTextProvider {
        calls: Arc<AtomicUsize>,
        deadline: Instant,
        cancel_during_call: bool,
        revision_changed: AtomicBool,
    }

    impl TextEmbeddingProvider for InterruptingTextProvider {
        fn embed_batch(
            &self,
            texts: &[&str],
        ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }

        fn embed_batch_within(
            &self,
            texts: &[&str],
            budget: &RequestBudgetV1,
        ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
            if budget.deadline() != self.deadline {
                return Err(CoreError::InvalidContract(
                    "original request deadline was not forwarded".into(),
                ));
            }
            let _previous = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            if self.cancel_during_call {
                budget.cancel_handle().cancel();
            } else {
                self.revision_changed.store(true, AtomicOrdering::SeqCst);
            }
            let memory = quanta_index_core::TextEmbeddingBatch::reserve_output(
                texts.len(),
                self.dimension(),
                budget,
            )?;
            let vectors = self.embed_batch(texts)?.into_unscoped()?;
            Ok(quanta_index_core::TextEmbeddingBatch::from_parts(
                vectors, memory,
            ))
        }

        fn model_id(&self) -> &'static str {
            "interrupting-test-model"
        }
        fn model_revision(&self) -> &str {
            if self.revision_changed.load(AtomicOrdering::SeqCst) {
                "revision-2"
            } else {
                "revision-1"
            }
        }
        fn dimension(&self) -> usize {
            2
        }
        fn normalization(&self) -> EmbeddingNormalization {
            EmbeddingNormalization::L2Unit
        }
    }

    #[test]
    fn text_bridge_pins_descriptor_and_forwards_original_budget()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = test_runtime()?;
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or("fixed test deadline overflow")?;
        let request = RequestBudgetV1::until(deadline);
        let calls = Arc::new(AtomicUsize::new(0));
        let bridge = TextEmbeddingBridge::new(FixedTextProvider {
            calls: Arc::clone(&calls),
            deadline,
            normalization: EmbeddingNormalization::L2Unit,
        })?;
        let recipe = bridge.recipe(1024)?;
        check_eq!(recipe.normalization, VectorNormalization::L2Unit);
        check_eq!(recipe.max_vectors, 1);
        let projection_recipe = bridge.projection_recipe(
            ProjectionId([2; 16]),
            ProducerId([3; 16]),
            SchemaId([4; 16]),
            1024,
        )?;
        check_ne!(recipe.fingerprint(), projection_recipe.fingerprint());
        check_eq!(recipe.model_identity, projection_recipe.model_identity);
        check_eq!(recipe.output_policy, projection_recipe.output_policy);
        check_eq!(
            bridge.embed_query("query", &request)?.first(),
            Some(&vec![1.0, 0.0])
        );
        let source = SourceRecord {
            path: "a.rs".into(),
            bytes: ObjectRef(Digest([1; 32])),
            byte_len: 6,
            text: quanta_index_contract::index::SourceTextAdmission::Excluded,
        };
        let vectors = crate::run_blocking(
            &runtime,
            bridge.embed(EmbeddingInput {
                owner: owner(1),
                binding: OwnerInputBinding {
                    source: Some(ObjectRef(Digest([1; 32]))),
                    projection: None,
                    producer_context: Digest([0; 32]),
                    derivation_policy: recipe.fingerprint(),
                },
                source: Some(&source),
                source_bytes: b"source",
                recipe: &recipe,
                request_budget: &request,
            }),
        )?;
        check_eq!(
            vectors,
            vec![EmbeddingVector {
                id: b"whole-file".to_vec(),
                values: vec![1.0, 0.0],
            }]
        );
        check_eq!(calls.load(AtomicOrdering::SeqCst), 2);
        check!(matches!(
            crate::run_blocking(
                &runtime,
                bridge.embed(EmbeddingInput {
                    owner: owner(1),
                    binding: OwnerInputBinding {
                        source: Some(ObjectRef(Digest([1; 32]))),
                        projection: None,
                        producer_context: Digest([0; 32]),
                        derivation_policy: recipe.fingerprint()
                    },
                    source: Some(&source),
                    source_bytes: &[0xff],
                    recipe: &recipe,
                    request_budget: &request,
                })
            ),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(calls.load(AtomicOrdering::SeqCst), 2);
        request.cancel_handle().cancel();
        check!(matches!(
            bridge.embed_query("query", &request),
            Err(IndexError::Cancelled)
        ));
        check_eq!(calls.load(AtomicOrdering::SeqCst), 2);
        Ok(())
    }

    fn owner(byte: u8) -> OwnerKey {
        OwnerKey {
            producer: ProducerId([1; 16]),
            stable_id: StableOwnerId([byte; 16]),
        }
    }

    fn recipe() -> EmbeddingRecipe {
        EmbeddingRecipe {
            input_policy: EmbeddingInputPolicy::SourceUtf8,
            model_identity: Digest([7; 32]),
            dimension: 2,
            normalization: VectorNormalization::L2Unit,
            chunk_policy: None,
            output_policy: Digest([8; 32]),
            max_vectors: 4,
            max_source_bytes: 1024,
        }
    }

    // Independent SHA-256 of the declared whole-file policy transcript,
    // including its trailing NUL byte. Keep this literal apart from the
    // implementation's policy-hash helper.
    const WHOLE_FILE_POLICY_GOLD: Digest = Digest([
        0x7d, 0x56, 0x75, 0xe1, 0x54, 0x06, 0x2f, 0xf5, 0x00, 0xb9, 0x61, 0x91, 0x9c, 0xa0, 0x32,
        0x48, 0x49, 0x23, 0x02, 0xf5, 0x91, 0xa6, 0xb9, 0x4e, 0xec, 0x08, 0x2d, 0xf2, 0x37, 0x2f,
        0xd6, 0x4d,
    ]);

    fn whole_file_recipe_gold() -> EmbeddingRecipe {
        let mut recipe = recipe();
        recipe.output_policy = WHOLE_FILE_POLICY_GOLD;
        recipe
    }

    fn fixed_whole_file_vector() -> EmbeddingVector {
        EmbeddingVector {
            id: b"whole-file".to_vec(),
            values: vec![1.0, 0.0],
        }
    }

    fn encoding_memory(
        capacity: u64,
        live: u64,
        cancel_on_reserve: Option<RequestBudgetV1>,
    ) -> Arc<EncodingMemory> {
        Arc::new(EncodingMemory {
            capacity,
            live: AtomicU64::new(live),
            reservations: AtomicUsize::new(0),
            installs: AtomicUsize::new(0),
            invalid: AtomicBool::new(false),
            cancel_on_reserve,
        })
    }

    fn encoding_artifact(policy: Digest) -> OwnerArtifact {
        OwnerArtifact {
            owner: OwnerKey {
                producer: ProducerId([0; 16]),
                stable_id: StableOwnerId([0; 16]),
            },
            input: OwnerInputBinding {
                source: None,
                projection: None,
                producer_context: Digest([0; 32]),
                derivation_policy: policy,
            },
            recipe: policy,
            payload_ref: ObjectRef(Digest([0; 32])),
            reuse_identity: None,
            chunks: Vec::new(),
            vectors: vec![EmbeddingVector {
                id: vec![1],
                values: vec![1.0, 0.0],
            }],
        }
    }

    #[test]
    fn owner_artifact_encoding_preserves_literal_json_and_children()
    -> Result<(), Box<dyn std::error::Error>> {
        // Fixed persisted wire gold, independent of the artifact serializer.
        const GOLDEN: &[u8] = concat!(
            "{\"owner\":{\"producer\":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],",
            "\"stable_id\":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]},",
            "\"input\":{\"source\":null,\"projection\":null,",
            "\"producer_context\":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],",
            "\"derivation_policy\":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]},",
            "\"recipe\":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],",
            "\"payload_ref\":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],",
            "\"reuse_identity\":null,\"chunks\":[],\"vectors\":[{\"id\":[1],\"values\":[1.0,0.0]}]}"
        ).as_bytes();
        // Independent SHA-256 of the literal JSON and the persisted v1 domain.
        const COMMITMENT: Digest = Digest([
            0xe6, 0x4b, 0xd5, 0x27, 0x89, 0x67, 0x85, 0x27, 0x27, 0x93, 0x7f, 0xdf, 0x6d, 0x55,
            0xf9, 0xe8, 0x14, 0x60, 0x91, 0x53, 0x84, 0x72, 0x32, 0x60, 0xb1, 0xa9, 0x37, 0x52,
            0x75, 0xc5, 0xab, 0x2a,
        ]);
        let memory = encoding_memory(4096, 0, None);
        let mut work = budget()?;
        work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
            EncodingScope(memory.clone()),
        ))));
        let mut artifact = encoding_artifact(Digest([0; 32]));
        let encoded = encode_owner_artifact(&artifact, &mut work)?;
        check_eq!(encoded.bytes.as_slice(), GOLDEN);
        check_eq!(commitment(&encoded.bytes), COMMITMENT);
        check_eq!(
            memory.live.load(AtomicOrdering::SeqCst),
            u64::try_from(GOLDEN.len())?
        );
        drop(encoded);
        check_eq!(memory.live.load(AtomicOrdering::SeqCst), 0);
        check!(!memory.invalid.load(AtomicOrdering::SeqCst));

        let zero = ObjectRef(Digest([0; 32]));
        let first = ObjectRef(Digest([1; 32]));
        let second = ObjectRef(Digest([2; 32]));
        check_eq!(artifact_children(&artifact).as_ref(), &[zero]);
        artifact.input.source = Some(zero);
        artifact.input.projection = Some(second);
        check_eq!(artifact_children(&artifact).as_ref(), &[zero, second]);
        artifact.payload_ref = second;
        artifact.input.projection = Some(first);
        check_eq!(
            artifact_children(&artifact).as_ref(),
            &[zero, first, second]
        );
        Ok(())
    }

    #[derive(Clone, Copy)]
    enum EncodingCase {
        Success,
        Denied,
        CancelledAfterAdmission,
        InstallFailed,
        DiscardBeforePoll,
    }

    fn verify_encoding_case(case: EncodingCase) -> Result<(), Box<dyn std::error::Error>> {
        const VECTOR_BYTES: u64 = 64;
        let recipe = recipe();
        let spec = instance(&recipe);
        let adapter = NativeSemantic::new(
            FixedProvider {
                calls: Arc::new(AtomicUsize::new(0)),
                output: vec![],
                descriptor: recipe.clone(),
            },
            recipe.clone(),
        )?;
        let mut work = budget()?;
        let memory = encoding_memory(
            if matches!(case, EncodingCase::Denied) {
                VECTOR_BYTES
            } else {
                4096
            },
            VECTOR_BYTES,
            matches!(case, EncodingCase::CancelledAfterAdmission).then(|| work.request().clone()),
        );
        let candidate = SemanticCandidate {
            instance: spec.instance,
            format: spec.format,
            _membership_memory: None,
            _output_memory: [
                Some(MemoryLease::new(VECTOR_BYTES, memory.clone())),
                None,
                None,
            ],
            artifact: encoding_artifact(recipe.fingerprint()),
        };
        work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
            EncodingScope(memory.clone()),
        ))));
        let store = MemoryStore(
            Mutex::default(),
            Some(EncodingInstallObserver {
                memory: memory.clone(),
                retained_bytes: VECTOR_BYTES,
                fail: matches!(case, EncodingCase::InstallFailed),
            }),
        );
        let maps = TestMap::default();
        let custody = TestCustody;
        let runtime = test_runtime()?;
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        };
        let future = adapter.verify_owner(candidate, &mut context);
        if matches!(case, EncodingCase::DiscardBeforePoll) {
            check_eq!(memory.live.load(AtomicOrdering::SeqCst), VECTOR_BYTES);
            drop(future);
            check_eq!(memory.live.load(AtomicOrdering::SeqCst), 0);
            check_eq!(memory.reservations.load(AtomicOrdering::SeqCst), 0);
            check_eq!(memory.installs.load(AtomicOrdering::SeqCst), 0);
            check!(!memory.invalid.load(AtomicOrdering::SeqCst));
            return Ok(());
        }
        let result = crate::run_blocking(&runtime, future);
        match case {
            EncodingCase::Success => {
                let member = result?;
                let stored = store.get(member.artifact, &mut work)?;
                check_eq!(member.logical_commitment, commitment(&stored.payload));
                check_eq!(memory.installs.load(AtomicOrdering::SeqCst), 1);
                check_eq!(work.usage().writes, 1);
            }
            EncodingCase::Denied => {
                check!(matches!(result, Err(IndexError::ResourceLimited)));
                check_eq!(work.usage().writes, 0);
            }
            EncodingCase::CancelledAfterAdmission => {
                check!(matches!(result, Err(IndexError::Cancelled)));
                check_eq!(work.usage().writes, 0);
            }
            EncodingCase::InstallFailed => {
                check!(matches!(result, Err(IndexError::Storage(_))));
                check_eq!(memory.installs.load(AtomicOrdering::SeqCst), 1);
                check_eq!(work.usage().writes, 0);
            }
            EncodingCase::DiscardBeforePoll => return Err("discard case was polled".into()),
        }
        if matches!(
            case,
            EncodingCase::Denied | EncodingCase::CancelledAfterAdmission
        ) {
            check_eq!(memory.installs.load(AtomicOrdering::SeqCst), 0);
            check!(
                store
                    .0
                    .lock()
                    .map_err(|_error| "test store lock")?
                    .is_empty()
            );
        }
        check_eq!(memory.reservations.load(AtomicOrdering::SeqCst), 1);
        check_eq!(memory.live.load(AtomicOrdering::SeqCst), 0);
        check!(!memory.invalid.load(AtomicOrdering::SeqCst));
        Ok(())
    }

    #[test]
    fn owner_artifact_encoding_keeps_json_and_vectors_through_install()
    -> Result<(), Box<dyn std::error::Error>> {
        verify_encoding_case(EncodingCase::Success)
    }

    #[test]
    fn owner_artifact_encoding_denial_precedes_store_mutation()
    -> Result<(), Box<dyn std::error::Error>> {
        verify_encoding_case(EncodingCase::Denied)
    }

    #[test]
    fn owner_artifact_encoding_cancel_after_admission_releases_both_owners()
    -> Result<(), Box<dyn std::error::Error>> {
        verify_encoding_case(EncodingCase::CancelledAfterAdmission)
    }

    #[test]
    fn owner_artifact_encoding_install_failure_releases_both_owners()
    -> Result<(), Box<dyn std::error::Error>> {
        verify_encoding_case(EncodingCase::InstallFailed)
    }

    #[test]
    fn owner_artifact_encoding_unpolled_future_keeps_candidate_charge()
    -> Result<(), Box<dyn std::error::Error>> {
        verify_encoding_case(EncodingCase::DiscardBeforePoll)
    }

    #[test]
    fn owner_artifact_encoding_writer_preserves_typed_failure_and_capacity()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::io::Write as _;
        let artifact = encoding_artifact(Digest([0; 32]));
        let work = budget()?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(4)?;
        let capacity = bytes.capacity();
        let mut writer = OwnerArtifactJsonWriter {
            output: Some(&mut bytes),
            length: 0,
            maximum: 4,
            budget: &work,
            failure: None,
        };
        check!(matches!(
            writer.serialize(&artifact),
            Err(IndexError::BackendViolation(_))
        ));
        check_eq!(bytes.capacity(), capacity);
        check!(bytes.len() <= 4);

        bytes.clear();
        let mut writer = OwnerArtifactJsonWriter {
            output: Some(&mut bytes),
            length: 0,
            maximum: 4,
            budget: &work,
            failure: None,
        };
        writer.write_all(b"{")?;
        work.request().cancel_handle().cancel();
        check!(matches!(
            writer.serialize(&artifact),
            Err(IndexError::Cancelled)
        ));
        check_eq!(bytes.as_slice(), b"{");
        check_eq!(bytes.capacity(), capacity);
        Ok(())
    }

    #[test]
    fn legacy_v1_recipe_policy_cannot_admit_new_artifact_format()
    -> Result<(), Box<dyn std::error::Error>> {
        // Independent golden from the prior v1 recipe hash, before the
        // persisted OwnerArtifact gained reuse_identity.
        const OLD_POLICY: Digest = Digest([
            0x0f, 0x0c, 0xa7, 0xf3, 0x4c, 0x92, 0x47, 0x53, 0xef, 0xb6, 0xad, 0x66, 0xd5, 0xeb,
            0xd7, 0xdb, 0x96, 0x25, 0x4f, 0x29, 0x6b, 0x3d, 0x4a, 0x89, 0xb6, 0xbe, 0x2c, 0xad,
            0x50, 0xaa, 0xae, 0x7c,
        ]);
        let recipe = recipe();
        let calls = Arc::new(AtomicUsize::new(0));
        let adapter = NativeSemantic::new(
            FixedProvider {
                calls: Arc::clone(&calls),
                output: vec![],
                descriptor: recipe.clone(),
            },
            recipe.clone(),
        )?;
        let mut old_instance = instance(&recipe);
        old_instance.policy = OLD_POLICY;
        check!(matches!(
            adapter.validate_instance(&old_instance),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(calls.load(AtomicOrdering::SeqCst), 0);
        // Independent prior v2 transcript: no chunk policy tag/member field.
        old_instance.policy = Digest([
            0xfc, 0x39, 0x12, 0x0e, 0xe4, 0x70, 0x07, 0x7b, 0x64, 0x20, 0xe8, 0xad, 0xd2, 0x06,
            0xf7, 0x77, 0x86, 0x3a, 0x5b, 0xed, 0x0c, 0x7f, 0x49, 0x4d, 0x69, 0x29, 0x2b, 0xdd,
            0xcc, 0x5d, 0xb6, 0x52,
        ]);
        check!(matches!(
            adapter.validate_instance(&old_instance),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(calls.load(AtomicOrdering::SeqCst), 0);
        Ok(())
    }

    fn instance(recipe: &EmbeddingRecipe) -> InstanceSpec {
        InstanceSpec {
            instance: IndexInstanceId([2; 16]),
            kind: IndexKind([3; 16]),
            group: PublicationGroupId([4; 16]),
            activation: GroupActivation::AsyncCoverage,
            input_domain: quanta_index_core::index::InputDomain::SourceLocal,
            schema: SchemaId([5; 16]),
            policy: recipe.fingerprint(),
            format: StorageFormatId([6; 16]),
        }
    }

    fn budget() -> Result<WorkBudget, Box<dyn std::error::Error>> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or("fixed test deadline overflow")?;
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 1000,
                writes: 1000,
                bytes: 100_000_000,
                mutations: 1000,
            },
            deadline,
        ))
    }

    fn attempt() -> AttemptRef {
        AttemptRef {
            operation: OperationKey {
                workspace: WorkspaceKey {
                    tenant: TenantId([1; 16]),
                    collection: CollectionId([1; 16]),
                    worktree: WorktreeId([1; 16]),
                    slot: SlotId([1; 16]),
                },
                epoch: 1,
                producer: ProducerId([1; 16]),
                stream: StreamId([1; 16]),
                stream_epoch: 1,
                sequence: 1,
            },
            attempt: AttemptId([1; 16]),
            fence: 1,
        }
    }

    fn source(store: &MemoryStore) -> Result<(ObjectRef, ObjectRef), Box<dyn std::error::Error>> {
        let bytes = store.insert(b"source".to_vec(), vec![])?;
        let record = SourceRecord {
            path: "a.rs".into(),
            bytes,
            byte_len: 6,
            text: quanta_index_contract::index::SourceTextAdmission::Excluded,
        };
        Ok((
            store.insert(serde_json::to_vec(&record)?, vec![bytes])?,
            bytes,
        ))
    }

    fn load_test_source(
        store: &MemoryStore,
        reference: ObjectRef,
        work: &mut WorkBudget,
    ) -> Result<SourceRecordOwner, IndexError> {
        load_source_record(
            reference,
            &mut AttemptContext {
                attempt: attempt(),
                objects: store,
                maps: &TestMap::default(),
                custody: &TestCustody,
                scratch: &NoScratch,
                budget: work,
            },
        )
    }

    #[test]
    fn source_record_decode_retains_path_charge_after_return()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = MemoryStore::default();
        let payload = store.insert(b"source".to_vec(), vec![])?;
        let record = SourceRecord {
            path: "src/\"源.rs".into(),
            bytes: payload,
            byte_len: 6,
            text: quanta_index_contract::index::SourceTextAdmission::Excluded,
        };
        let wire = serde_json::to_vec(&record)?;
        let expected = u64::try_from(wire.len())?
            .checked_mul(8)
            .ok_or("decode bound")?;
        let reference = store.insert(wire, vec![payload])?;
        let memory = encoding_memory(expected, 0, None);
        let mut work = budget()?;
        work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
            EncodingScope(memory.clone()),
        ))));
        let loaded = load_test_source(&store, reference, &mut work)?;
        check_eq!(loaded.path.as_str(), "src/\"源.rs");
        check_eq!(loaded.bytes, payload);
        check_eq!(memory.live.load(AtomicOrdering::SeqCst), expected);
        check_eq!(memory.reservations.load(AtomicOrdering::SeqCst), 1);
        drop(loaded);
        check_eq!(memory.live.load(AtomicOrdering::SeqCst), 0);
        check!(!memory.invalid.load(AtomicOrdering::SeqCst));
        Ok(())
    }

    #[test]
    fn source_record_decode_denial_precedes_malformed_json_and_error_refunds()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = MemoryStore::default();
        let reference = store.insert(b"{".to_vec(), vec![])?;
        for capacity in [0, 8] {
            let memory = encoding_memory(capacity, 0, None);
            let mut work = budget()?;
            work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
                EncodingScope(memory.clone()),
            ))));
            let result = load_test_source(&store, reference, &mut work);
            if capacity == 0 {
                check!(matches!(result, Err(IndexError::ResourceLimited)));
            } else {
                check!(matches!(result, Err(IndexError::CorruptObject(_))));
            }
            check_eq!(memory.reservations.load(AtomicOrdering::SeqCst), 1);
            check_eq!(memory.live.load(AtomicOrdering::SeqCst), 0);
            check!(!memory.invalid.load(AtomicOrdering::SeqCst));
        }
        Ok(())
    }

    #[test]
    fn source_record_decode_cancel_and_edge_failure_release_path_charge()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = MemoryStore::default();
        let (valid, _payload) = source(&store)?;
        let mut reading = budget()?;
        let bad_edges = store.insert(store.get(valid, &mut reading)?.payload.clone(), vec![])?;
        for cancel in [false, true] {
            let mut work = budget()?;
            let memory = encoding_memory(16 * 1024, 0, cancel.then(|| work.request().clone()));
            work.bind_memory_scope(Some(quanta_index_core::index::MemoryScope::new(Arc::new(
                EncodingScope(memory.clone()),
            ))));
            let result =
                load_test_source(&store, if cancel { valid } else { bad_edges }, &mut work);
            if cancel {
                check!(matches!(result, Err(IndexError::Cancelled)));
            } else {
                check!(matches!(result, Err(IndexError::CorruptObject(_))));
            }
            check_eq!(memory.reservations.load(AtomicOrdering::SeqCst), 1);
            check_eq!(memory.live.load(AtomicOrdering::SeqCst), 0);
            check!(!memory.invalid.load(AtomicOrdering::SeqCst));
        }
        Ok(())
    }

    #[test]
    fn projection_children_preserve_fixed_empty_alias_and_sorted_gold()
    -> Result<(), Box<dyn std::error::Error>> {
        let first = ObjectRef(Digest([1; 32]));
        let second = ObjectRef(Digest([2; 32]));
        let mut contribution = ProjectionOwnerContribution {
            projection: ProjectionId([0; 16]),
            producer: ProducerId([0; 16]),
            owner: owner(0),
            source_owner: None,
            source: None,
            dependency_stamp: Digest([0; 32]),
            output: None,
        };
        check!(contribution_children(contribution).as_ref().is_empty());
        contribution.output = Some(second);
        check_eq!(contribution_children(contribution).as_ref(), &[second]);
        contribution.source = Some(second);
        check_eq!(contribution_children(contribution).as_ref(), &[second]);
        contribution.source = Some(first);
        check_eq!(
            contribution_children(contribution).as_ref(),
            &[first, second]
        );
        contribution.source = Some(second);
        contribution.output = Some(first);
        check_eq!(
            contribution_children(contribution).as_ref(),
            &[first, second]
        );
        Ok(())
    }

    #[test]
    fn valid_different_prior_payload_reembeds_but_malformed_prior_fails_closed()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = test_runtime()?;
        let store = MemoryStore::default();
        let maps = TestMap::default();
        let custody = TestCustody;
        let recipe = recipe();
        let spec = instance(&recipe);
        let calls = Arc::new(AtomicUsize::new(0));
        let adapter = NativeSemantic::new(
            FixedProvider {
                calls: Arc::clone(&calls),
                output: vec![EmbeddingVector {
                    id: b"whole-file".to_vec(),
                    values: vec![1.0, 0.0],
                }],
                descriptor: recipe.clone(),
            },
            recipe.clone(),
        )?;
        let (old_source, old_payload) = source(&store)?;
        let new_payload = store.insert(b"changed".to_vec(), vec![])?;
        let new_record = SourceRecord {
            path: "b.rs".into(),
            bytes: new_payload,
            byte_len: 7,
            text: quanta_index_contract::index::SourceTextAdmission::Excluded,
        };
        let new_source = store.insert(serde_json::to_vec(&new_record)?, vec![new_payload])?;
        let old_input = OwnerInputBinding {
            source: Some(old_source),
            projection: None,
            producer_context: Digest([0; 32]),
            derivation_policy: recipe.fingerprint(),
        };
        let new_input = OwnerInputBinding {
            source: Some(new_source),
            ..old_input
        };
        let old_artifact = OwnerArtifact {
            owner: owner(1),
            input: old_input,
            recipe: recipe.fingerprint(),
            payload_ref: old_payload,
            reuse_identity: None,
            chunks: Vec::new(),
            vectors: vec![EmbeddingVector {
                id: b"whole-file".to_vec(),
                values: vec![0.0, 1.0],
            }],
        };
        let encoded = serde_json::to_vec(&old_artifact)?;
        let old_ref = store.insert(
            encoded.clone(),
            artifact_children(&old_artifact).as_ref().to_vec(),
        )?;
        let prior = OwnerMemberBinding {
            instance: spec.instance,
            owner: owner(1),
            input: old_input,
            format: spec.format,
            artifact: old_ref,
            logical_commitment: commitment(&encoded),
        };
        let mut work = budget()?;
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        };
        let candidate = crate::run_blocking(
            &runtime,
            adapter.prepare_owner(
                &spec,
                owner(1),
                new_input,
                Some(prior.clone()),
                &mut context,
            ),
        )?;
        check_eq!(candidate.artifact.payload_ref, new_payload);
        check_eq!(candidate.artifact.input, new_input);
        check_eq!(
            candidate
                .artifact
                .vectors
                .first()
                .ok_or("missing candidate vector")?
                .values,
            vec![1.0, 0.0]
        );
        check_eq!(calls.load(AtomicOrdering::SeqCst), 1);

        let bad_commitment = OwnerMemberBinding {
            logical_commitment: Digest([99; 32]),
            ..prior
        };
        check!(matches!(
            crate::run_blocking(
                &runtime,
                adapter.prepare_owner(
                    &spec,
                    owner(1),
                    new_input,
                    Some(bad_commitment),
                    &mut context,
                )
            ),
            Err(IndexError::CorruptObject(_))
        ));
        let missing_edge = store.insert(encoded, vec![old_payload])?;
        let bad_edges = OwnerMemberBinding {
            artifact: missing_edge,
            ..prior
        };
        check!(matches!(
            crate::run_blocking(
                &runtime,
                adapter.prepare_owner(&spec, owner(1), new_input, Some(bad_edges), &mut context,)
            ),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(calls.load(AtomicOrdering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn instance_admission_rejects_recipe_domain_and_projection_schema_without_derivation()
    -> Result<(), Box<dyn std::error::Error>> {
        let calls = Arc::new(AtomicUsize::new(0));
        let source_recipe = recipe();
        let provider = |descriptor: EmbeddingRecipe| FixedProvider {
            calls: Arc::clone(&calls),
            output: vec![],
            descriptor,
        };
        let source = NativeSemantic::new(provider(source_recipe.clone()), source_recipe.clone())?;
        let valid = instance(&source_recipe);
        source.validate_instance(&valid)?;
        let mut wrong = valid.clone();
        wrong.policy = Digest([0; 32]);
        check!(matches!(
            source.validate_instance(&wrong),
            Err(IndexError::InvalidInput(_))
        ));
        wrong = valid;
        wrong.activation = GroupActivation::CoreRequired;
        check!(matches!(
            source.validate_instance(&wrong),
            Err(IndexError::InvalidInput(_))
        ));
        let mut projection_recipe = source_recipe;
        let projection = ProjectionId([11; 16]);
        let producer = ProducerId([12; 16]);
        let schema = SchemaId([13; 16]);
        projection_recipe.input_policy = EmbeddingInputPolicy::ProjectionUtf8 {
            projection,
            producer,
            schema,
        };
        let adapter = NativeSemantic::new(
            provider(projection_recipe.clone()),
            projection_recipe.clone(),
        )?;
        let mut spec = instance(&projection_recipe);
        check!(matches!(
            adapter.validate_instance(&spec),
            Err(IndexError::InvalidInput(_))
        ));
        spec.input_domain = InputDomain::Projection {
            projection,
            producer,
        };
        spec.schema = schema;
        adapter.validate_instance(&spec)?;
        check!(matches!(
            source.validate_instance(&spec),
            Err(IndexError::InvalidInput(_))
        ));
        spec.schema = SchemaId([14; 16]);
        check!(matches!(
            adapter.validate_instance(&spec),
            Err(IndexError::InvalidInput(_))
        ));
        spec.schema = schema;
        spec.input_domain = InputDomain::Projection {
            projection,
            producer: ProducerId([15; 16]),
        };
        check!(matches!(
            adapter.validate_instance(&spec),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(calls.load(AtomicOrdering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn wrong_recipe_and_cancelled_request_do_not_call_provider()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = test_runtime()?;
        let store = MemoryStore::default();
        let maps = TestMap::default();
        let custody = TestCustody;
        let calls = Arc::new(AtomicUsize::new(0));
        let mut wrong_model = recipe();
        wrong_model.model_identity = Digest([0; 32]);
        check!(matches!(
            NativeSemantic::new(
                FixedProvider {
                    calls: Arc::clone(&calls),
                    output: vec![],
                    descriptor: wrong_model,
                },
                recipe(),
            ),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(calls.load(AtomicOrdering::SeqCst), 0);
        let adapter = NativeSemantic::new(
            FixedProvider {
                calls: Arc::clone(&calls),
                output: vec![],
                descriptor: recipe(),
            },
            recipe(),
        )?;
        let mut spec = instance(adapter.recipe());
        spec.policy = Digest([0; 32]);
        let input = OwnerInputBinding {
            source: Some(ObjectRef(Digest([9; 32]))),
            projection: None,
            producer_context: Digest([0; 32]),
            derivation_policy: recipe().fingerprint(),
        };
        let mut work = budget()?;
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        };
        check!(matches!(
            crate::run_blocking(
                &runtime,
                adapter.prepare_owner(&spec, owner(1), input, None, &mut context)
            ),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(calls.load(AtomicOrdering::SeqCst), 0);

        let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
        request.cancel_handle().cancel();
        let mut cancelled = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 100_000,
                mutations: 100,
            },
            request,
        );
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut cancelled,
        };
        check!(matches!(
            crate::run_blocking(
                &runtime,
                adapter.prepare_owner(
                    &instance(adapter.recipe()),
                    owner(1),
                    input,
                    None,
                    &mut context
                )
            ),
            Err(IndexError::Cancelled)
        ));
        check_eq!(calls.load(AtomicOrdering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn malformed_provider_result_cannot_seal() -> Result<(), Box<dyn std::error::Error>> {
        let runtime = test_runtime()?;
        let store = MemoryStore::default();
        let maps = TestMap::default();
        let custody = TestCustody;
        let (source, _bytes) = source(&store)?;
        let recipe = recipe();
        let calls = Arc::new(AtomicUsize::new(0));
        let adapter = NativeSemantic::new(
            FixedProvider {
                calls: Arc::clone(&calls),
                output: vec![EmbeddingVector {
                    id: b"a".to_vec(),
                    values: vec![f32::NAN, 0.0],
                }],
                descriptor: recipe.clone(),
            },
            recipe.clone(),
        )?;
        let input = OwnerInputBinding {
            source: Some(source),
            projection: None,
            producer_context: Digest([0; 32]),
            derivation_policy: recipe.fingerprint(),
        };
        let mut work = budget()?;
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            scratch: &NoScratch,
            budget: &mut work,
        };
        check!(matches!(
            crate::run_blocking(
                &runtime,
                adapter.prepare_owner(&instance(&recipe), owner(1), input, None, &mut context)
            ),
            Err(IndexError::BackendViolation(_))
        ));
        check_eq!(calls.load(AtomicOrdering::SeqCst), 1);
        Ok(())
    }

    #[test]
    fn declared_whole_file_policy_requires_one_canonical_vector_without_changing_generic_policy()
    -> Result<(), Box<dyn std::error::Error>> {
        let whole_file = whole_file_recipe_gold();
        check_eq!(whole_file_output_policy(), WHOLE_FILE_POLICY_GOLD);
        validate_vectors(&[fixed_whole_file_vector()], &whole_file, &mut budget()?)?;

        let distinct = vec![
            EmbeddingVector {
                id: b"a".to_vec(),
                values: vec![1.0, 0.0],
            },
            EmbeddingVector {
                id: b"b".to_vec(),
                values: vec![0.0, 1.0],
            },
        ];
        for vectors in [
            Vec::new(),
            distinct.clone(),
            vec![EmbeddingVector {
                id: b"other".to_vec(),
                values: vec![1.0, 0.0],
            }],
        ] {
            check!(matches!(
                validate_vectors(&vectors, &whole_file, &mut budget()?),
                Err(IndexError::BackendViolation(_))
            ));
        }

        // An opaque provider policy can still declare zero or multiple
        // vectors; the singleton rule belongs only to the known transcript.
        let generic = recipe();
        validate_vectors(&[], &generic, &mut budget()?)?;
        validate_vectors(&distinct, &generic, &mut budget()?)?;
        Ok(())
    }

    #[test]
    fn declared_whole_file_provider_shape_refuses_before_owner_artifact_put()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = test_runtime()?;
        let recipe = whole_file_recipe_gold();
        let spec = instance(&recipe);
        let bad_outputs = [
            Vec::new(),
            vec![
                EmbeddingVector {
                    id: b"a".to_vec(),
                    values: vec![1.0, 0.0],
                },
                EmbeddingVector {
                    id: b"b".to_vec(),
                    values: vec![0.0, 1.0],
                },
            ],
            vec![EmbeddingVector {
                id: b"other".to_vec(),
                values: vec![1.0, 0.0],
            }],
        ];
        for output in bad_outputs {
            let store = MemoryStore::default();
            let (source, _payload) = semantic_source(&store, "a.rs", b"source")?;
            let before = store.0.lock().map_err(|_error| "test store lock")?.len();
            let calls = Arc::new(AtomicUsize::new(0));
            let adapter = NativeSemantic::new(
                FixedProvider {
                    calls: Arc::clone(&calls),
                    output,
                    descriptor: recipe.clone(),
                },
                recipe.clone(),
            )?;
            let input = OwnerInputBinding {
                source: Some(source),
                projection: None,
                producer_context: Digest([0; 32]),
                derivation_policy: recipe.fingerprint(),
            };
            let mut work = budget()?;
            let maps = TestMap::default();
            let custody = TestCustody;
            let scratch = NoScratch;
            let mut context = AttemptContext {
                attempt: attempt(),
                objects: &store,
                maps: &maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            };
            check!(matches!(
                crate::run_blocking(
                    &runtime,
                    adapter.prepare_owner(&spec, owner(1), input, None, &mut context)
                ),
                Err(IndexError::BackendViolation(_))
            ));
            check_eq!(calls.load(AtomicOrdering::SeqCst), 1);
            check_eq!(work.usage().writes, 0);
            let after = store.0.lock().map_err(|_error| "test store lock")?.len();
            check_eq!(after, before);
        }
        Ok(())
    }

    #[test]
    fn declared_whole_file_prepare_verify_and_readback_share_the_shape_gate()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = test_runtime()?;
        let recipe = whole_file_recipe_gold();
        let spec = instance(&recipe);
        let store = MemoryStore::default();
        let (source, _payload) = semantic_source(&store, "a.rs", b"source")?;
        let calls = Arc::new(AtomicUsize::new(0));
        let adapter = NativeSemantic::new(
            FixedProvider {
                calls: Arc::clone(&calls),
                output: vec![fixed_whole_file_vector()],
                descriptor: recipe.clone(),
            },
            recipe.clone(),
        )?;
        let input = OwnerInputBinding {
            source: Some(source),
            projection: None,
            producer_context: Digest([0; 32]),
            derivation_policy: recipe.fingerprint(),
        };
        let maps = TestMap::default();
        let custody = TestCustody;
        let scratch = NoScratch;
        let mut work = budget()?;
        let member = {
            let mut context = AttemptContext {
                attempt: attempt(),
                objects: &store,
                maps: &maps,
                custody: &custody,
                scratch: &scratch,
                budget: &mut work,
            };
            let candidate = crate::run_blocking(
                &runtime,
                adapter.prepare_owner(&spec, owner(1), input, None, &mut context),
            )?;
            let member =
                crate::run_blocking(&runtime, adapter.verify_owner(candidate, &mut context))?;
            check_eq!(context.budget.usage().writes, 1);

            let mut forged = crate::run_blocking(
                &runtime,
                adapter.prepare_owner(&spec, owner(1), input, None, &mut context),
            )?;
            forged.artifact.vectors.clear();
            let object_count = store.0.lock().map_err(|_error| "test store lock")?.len();
            check!(matches!(
                crate::run_blocking(&runtime, adapter.verify_owner(forged, &mut context)),
                Err(IndexError::BackendViolation(_))
            ));
            check_eq!(context.budget.usage().writes, 1);
            let after = store.0.lock().map_err(|_error| "test store lock")?.len();
            check_eq!(after, object_count);
            check_eq!(calls.load(AtomicOrdering::SeqCst), 2);
            member
        };
        let loaded = load_owner_artifact(&store, &mut work, &member, &recipe)?;
        let gold = [fixed_whole_file_vector()];
        check_eq!(loaded.vectors.as_slice(), gold.as_slice());
        Ok(())
    }

    #[test]
    fn declared_whole_file_persisted_artifact_refuses_shape_with_valid_binding_and_commitment()
    -> Result<(), Box<dyn std::error::Error>> {
        let recipe = whole_file_recipe_gold();
        let spec = instance(&recipe);
        for vectors in [
            Vec::new(),
            vec![
                EmbeddingVector {
                    id: b"a".to_vec(),
                    values: vec![1.0, 0.0],
                },
                EmbeddingVector {
                    id: b"b".to_vec(),
                    values: vec![0.0, 1.0],
                },
            ],
            vec![EmbeddingVector {
                id: b"other".to_vec(),
                values: vec![1.0, 0.0],
            }],
        ] {
            let store = MemoryStore::default();
            let mut artifact = encoding_artifact(recipe.fingerprint());
            artifact.vectors = vectors;
            let payload = serde_json::to_vec(&artifact)?;
            let reference = store.insert(
                payload.clone(),
                artifact_children(&artifact).as_ref().to_vec(),
            )?;
            let member = OwnerMemberBinding {
                instance: spec.instance,
                owner: artifact.owner,
                input: artifact.input,
                format: spec.format,
                artifact: reference,
                logical_commitment: commitment(&payload),
            };
            check!(matches!(
                load_owner_artifact(&store, &mut budget()?, &member, &recipe),
                Err(IndexError::BackendViolation(_))
            ));
        }
        Ok(())
    }

    #[test]
    fn exact_cosine_matches_fixed_ranking_and_reports_partial_or_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = MemoryStore::default();
        let recipe = recipe();
        let instance = instance(&recipe);
        let (source, source_bytes) = source(&store)?;
        let input = OwnerInputBinding {
            source: Some(source),
            projection: None,
            producer_context: Digest([0; 32]),
            derivation_policy: recipe.fingerprint(),
        };
        let candidates = [([1.0, 0.0], 1u8), ([0.8, 0.6], 2u8), ([0.0, 1.0], 3u8)];
        let mut entries = Vec::new();
        for (values, id) in candidates {
            let artifact = OwnerArtifact {
                owner: owner(id),
                input,
                recipe: recipe.fingerprint(),
                payload_ref: source_bytes,
                reuse_identity: None,
                chunks: Vec::new(),
                vectors: vec![EmbeddingVector {
                    id: vec![id],
                    values: values.to_vec(),
                }],
            };
            let artifact_payload = serde_json::to_vec(&artifact)?;
            let artifact_ref = store.insert(
                artifact_payload.clone(),
                artifact_children(&artifact).as_ref().to_vec(),
            )?;
            let member = OwnerMemberBinding {
                instance: instance.instance,
                owner: owner(id),
                input,
                format: instance.format,
                artifact: artifact_ref,
                logical_commitment: commitment(&artifact_payload),
            };
            let mut member_children = vec![artifact_ref, source];
            member_children.sort_unstable();
            member_children.dedup();
            let member_ref = store.insert(serde_json::to_vec(&member)?, member_children)?;
            let coverage = OwnerCoverage {
                owner: owner(id),
                desired: input,
                covered: Some(input),
                searchable: Some(member_ref),
            };
            let mut coverage_children = vec![member_ref, source];
            coverage_children.sort_unstable();
            coverage_children.dedup();
            let coverage_ref = store.insert(serde_json::to_vec(&coverage)?, coverage_children)?;
            entries.push((owner_map_key(owner(id)).to_vec(), coverage_ref));
        }
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let source_root = MapRoot(Some(ObjectRef(Digest([0xfb; 32]))));
        let source_entries = [1, 2, 3]
            .into_iter()
            .map(|id| (owner_map_key(owner(id)).to_vec(), source))
            .collect();
        let maps = TestMap {
            entries,
            rooted_entries: vec![(source_root, source_entries)],
            inline_rooted_entries: Vec::new(),
            fail: false,
        };
        let coverage = CoverageBinding {
            instance: instance.instance,
            desired: quanta_index_core::index::InputBinding {
                root: source_root,
                schema: instance.schema,
                policy: recipe.fingerprint(),
                context: Digest([0; 32]),
            },
            covered: None,
            searchable: None,
            status: CoverageStatus::Partial {
                missing: MapRoot(Some(source)),
            },
            owners: MapRoot(Some(source)),
        };
        let mut work = budget()?;
        let mut read = ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        };
        let result = search_exact(
            &SearchQuery {
                lexical_eligibility: None,
                lexical_owners: None,
                exact_path: None,
                coverage: &coverage,
                projection: None,
                source_owners: source_root,
                source_paths: MapRoot(None),
                instance: instance.instance,
                format: instance.format,
                recipe: &recipe,
                vector: &[1.0, 0.0],
                top_k: 2,
                page_size: 2,
                max_owners: 10,
                overlay: None,
            },
            &mut read,
        )?;
        check_eq!(
            result
                .hits
                .iter()
                .map(|hit| hit.vector_id.clone())
                .collect::<Vec<_>>(),
            vec![vec![1], vec![2]]
        );
        check!(!result.exhaustive);
        check!(result.traversal == NativeSemanticTraversal::ExactExhausted);
        let mut fully_covered = coverage;
        fully_covered.covered = Some(fully_covered.desired);
        fully_covered.status = CoverageStatus::Complete;
        let complete = search_exact(
            &SearchQuery {
                lexical_eligibility: None,
                lexical_owners: None,
                exact_path: None,
                coverage: &fully_covered,
                projection: None,
                source_owners: source_root,
                source_paths: MapRoot(None),
                instance: instance.instance,
                format: instance.format,
                recipe: &recipe,
                vector: &[1.0, 0.0],
                top_k: 2,
                page_size: 2,
                max_owners: 10,
                overlay: None,
            },
            &mut read,
        )?;
        check!(complete.exhaustive);
        check!(complete.traversal == NativeSemanticTraversal::ExactExhausted);
        let truncated = search_exact(
            &SearchQuery {
                lexical_eligibility: None,
                lexical_owners: None,
                exact_path: None,
                coverage: &coverage,
                projection: None,
                source_owners: source_root,
                source_paths: MapRoot(None),
                instance: instance.instance,
                format: instance.format,
                recipe: &recipe,
                vector: &[1.0, 0.0],
                top_k: 2,
                page_size: 2,
                max_owners: 1,
                overlay: None,
            },
            &mut read,
        )?;
        check_eq!(truncated.scanned_owners, 1);
        check!(!truncated.exhaustive);
        check!(truncated.traversal == NativeSemanticTraversal::ExactCapped);
        let mut stale_entries = maps.entries.clone();
        let stale_owner = OwnerCoverage {
            owner: owner(3),
            desired: input,
            covered: None,
            searchable: None,
        };
        let stale_ref = store.insert(serde_json::to_vec(&stale_owner)?, vec![source])?;
        let stale_key = owner_map_key(owner(3));
        if let Some((_, reference)) = stale_entries
            .iter_mut()
            .find(|(key, _)| key.as_slice() == stale_key.as_slice())
        {
            *reference = stale_ref;
        } else {
            return Err("missing test owner".into());
        }
        let stale_map = TestMap {
            entries: stale_entries,
            rooted_entries: maps.rooted_entries.clone(),
            inline_rooted_entries: Vec::new(),
            fail: false,
        };
        let mut read = ReadContext {
            objects: &store,
            maps: &stale_map,
            budget: &mut work,
        };
        let stale = search_exact(
            &SearchQuery {
                lexical_eligibility: None,
                lexical_owners: None,
                exact_path: None,
                coverage: &fully_covered,
                projection: None,
                source_owners: source_root,
                source_paths: MapRoot(None),
                instance: instance.instance,
                format: instance.format,
                recipe: &recipe,
                vector: &[1.0, 0.0],
                top_k: 2,
                page_size: 2,
                max_owners: 10,
                overlay: None,
            },
            &mut read,
        )?;
        check_eq!(stale.skipped_stale, 1);
        check!(!stale.exhaustive);
        check!(stale.traversal == NativeSemanticTraversal::ExactExhausted);
        let mut first_stale_entries = maps.entries.clone();
        let first_stale = OwnerCoverage {
            owner: owner(1),
            desired: input,
            covered: None,
            searchable: None,
        };
        let reference = store.insert(serde_json::to_vec(&first_stale)?, vec![source])?;
        let (_, first_reference) = first_stale_entries.first_mut().ok_or("first owner")?;
        *first_reference = reference;
        let first_stale_map = TestMap {
            entries: first_stale_entries,
            rooted_entries: maps.rooted_entries.clone(),
            inline_rooted_entries: Vec::new(),
            fail: false,
        };
        let mut read = ReadContext {
            objects: &store,
            maps: &first_stale_map,
            budget: &mut work,
        };
        let capped_stale = search_exact(
            &SearchQuery {
                lexical_eligibility: None,
                lexical_owners: None,
                exact_path: None,
                coverage: &fully_covered,
                projection: None,
                source_owners: source_root,
                source_paths: MapRoot(None),
                instance: instance.instance,
                format: instance.format,
                recipe: &recipe,
                vector: &[1.0, 0.0],
                top_k: 2,
                page_size: 1,
                max_owners: 1,
                overlay: None,
            },
            &mut read,
        )?;
        check!(capped_stale.hits.is_empty());
        check_eq!(capped_stale.scanned_owners, 1);
        check_eq!(capped_stale.skipped_stale, 1);
        check!(!capped_stale.exhaustive);
        check_eq!(capped_stale.traversal, NativeSemanticTraversal::ExactCapped);
        let failing = TestMap {
            entries: vec![],
            rooted_entries: Vec::new(),
            inline_rooted_entries: Vec::new(),
            fail: true,
        };
        let mut read = ReadContext {
            objects: &store,
            maps: &failing,
            budget: &mut work,
        };
        check!(matches!(
            search_exact(
                &SearchQuery {
                    lexical_eligibility: None,
                    lexical_owners: None,
                    exact_path: None,
                    coverage: &coverage,
                    projection: None,
                    source_owners: source_root,
                    source_paths: MapRoot(None),
                    instance: instance.instance,
                    format: instance.format,
                    recipe: &recipe,
                    vector: &[1.0, 0.0],
                    top_k: 2,
                    page_size: 2,
                    max_owners: 10,
                    overlay: None,
                },
                &mut read
            ),
            Err(IndexError::Storage(_))
        ));
        Ok(())
    }

    struct StaticCatalog {
        ticket: TicketRecord,
        releases: Arc<AtomicUsize>,
        validations: AtomicUsize,
        expires_on_validation: Option<usize>,
    }

    struct StaticSession;
    impl ReadSessionLease for StaticSession {
        fn binding(&self) -> quanta_index_core::index::ReadSessionRef {
            quanta_index_core::index::ReadSessionRef { id: 1, fence: 1 }
        }
    }

    fn test_ticket_lease(id: TicketId) -> quanta_index_core::index::ReadTicketLease {
        quanta_index_core::index::ReadTicketLease {
            id,
            session: StaticSession.binding(),
            fence: 1,
            expires_at_ms: 1_000,
        }
    }

    impl CatalogReadPort for StaticCatalog {
        fn open_read_session(&self) -> Result<Arc<dyn ReadSessionLease>, IndexError> {
            Ok(Arc::new(StaticSession))
        }
        fn acquire_ticket(
            &self,
            session: &dyn ReadSessionLease,
            _selection: ViewSelection,
            _lease: Duration,
            budget: &WorkBudget,
        ) -> Result<TicketRecord, IndexError> {
            budget.checkpoint()?;
            if session.binding() != self.ticket.lease.session {
                return Err(IndexError::FenceLost);
            }
            Ok(self.ticket.clone())
        }
        fn renew_ticket(
            &self,
            _session: &dyn ReadSessionLease,
            _ticket: quanta_index_core::index::ReadTicketLease,
            _lease: Duration,
        ) -> Result<quanta_index_core::index::ReadTicketLease, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test renewal".into(),
            ))
        }
        fn validate_ticket(
            &self,
            session: &dyn ReadSessionLease,
            ticket: quanta_index_core::index::ReadTicketLease,
        ) -> Result<(), IndexError> {
            if ticket != self.ticket.lease || session.binding() != ticket.session {
                return Err(IndexError::FenceLost);
            }
            let ordinal = self
                .validations
                .fetch_add(1, AtomicOrdering::SeqCst)
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if self
                .expires_on_validation
                .is_some_and(|expiry| ordinal >= expiry)
            {
                return Err(IndexError::SnapshotUnavailable);
            }
            Ok(())
        }
        fn release_ticket(
            &self,
            session: &dyn ReadSessionLease,
            ticket: quanta_index_core::index::ReadTicketLease,
        ) -> Result<(), IndexError> {
            // Release remains legal after expiry; it still checks the binding.
            if ticket != self.ticket.lease || session.binding() != ticket.session {
                return Err(IndexError::FenceLost);
            }
            let _previous = self.releases.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(())
        }
        fn recover_read_sessions(
            &self,
            _after: Option<quanta_index_core::index::ReadSessionRef>,
            _limit: u32,
            _budget: &mut WorkBudget,
        ) -> Result<quanta_index_core::index::ReadSessionRecoveryPage, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test recovery".into(),
            ))
        }
    }

    #[test]
    fn acquired_exact_search_resolves_coverage_from_ticket_and_releases_lease()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = MemoryStore::default();
        let recipe = recipe();
        let spec = instance(&recipe);
        let profile = store.insert(
            serde_json::to_vec(&IndexProfile {
                instances: vec![spec.clone()],
            })?,
            vec![],
        )?;
        let desired = InputBinding {
            root: MapRoot(None),
            schema: spec.schema,
            policy: recipe.fingerprint(),
            context: Digest([0; 32]),
        };
        let coverage = CoverageBinding {
            instance: spec.instance,
            desired,
            covered: Some(desired),
            searchable: None,
            status: CoverageStatus::Complete,
            owners: MapRoot(None),
        };
        let coverage_ref = store.insert(serde_json::to_vec(&coverage)?, vec![])?;
        let mut key = b"member\0".to_vec();
        key.extend_from_slice(&spec.instance.0);
        let maps = TestMap {
            entries: vec![(key, coverage_ref)],
            rooted_entries: Vec::new(),
            inline_rooted_entries: Vec::new(),
            fail: false,
        };
        let workspace = attempt().operation.workspace;
        let mut view = CatalogView {
            identity: ViewRef {
                workspace,
                epoch: 1,
                sequence: 1,
                digest: Digest([0; 32]),
            },
            profile,
            source: SourceCut {
                owners: MapRoot(None),
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            },
            members: MapRoot(None),
            coverage: MapRoot(Some(coverage_ref)),
            projections: MapRoot(None),
        };
        view.identity.digest = view.identity_digest()?;
        let releases = Arc::new(AtomicUsize::new(0));
        let catalog = StaticCatalog {
            ticket: TicketRecord {
                lease: test_ticket_lease(TicketId([5; 16])),
                view: view.clone(),
            },
            releases: Arc::clone(&releases),
            validations: AtomicUsize::new(0),
            expires_on_validation: None,
        };
        let mut work = budget()?;
        let mut context = ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        };
        let selection = ViewSelection::Exact(view.identity);
        let query = || TicketSearchQuery {
            selection,
            instance: spec.instance,
            format: spec.format,
            recipe: &recipe,
            vector: &[1.0, 0.0],
            top_k: 1,
            page_size: 1,
            max_owners: 1,
        };
        let result = search_exact_acquired(
            &catalog,
            &StaticSession,
            Duration::from_secs(1),
            query(),
            &mut context,
        )?;
        check!(result.hits.is_empty());
        check!(result.exhaustive);
        check_eq!(releases.load(AtomicOrdering::SeqCst), 1);
        check_eq!(catalog.validations.load(AtomicOrdering::SeqCst), 2);
        // Both an expired input and expiry after successful search must refuse
        // the result and release the acquired ticket exactly once.
        for expiry in [1, 2] {
            let expired_releases = Arc::new(AtomicUsize::new(0));
            let expired_catalog = StaticCatalog {
                ticket: catalog.ticket.clone(),
                releases: Arc::clone(&expired_releases),
                validations: AtomicUsize::new(0),
                expires_on_validation: Some(expiry),
            };
            check!(matches!(
                search_exact_acquired(
                    &expired_catalog,
                    &StaticSession,
                    Duration::from_secs(1),
                    query(),
                    &mut context,
                ),
                Err(IndexError::SnapshotUnavailable)
            ));
            check_eq!(
                expired_catalog.validations.load(AtomicOrdering::SeqCst),
                expiry
            );
            check_eq!(expired_releases.load(AtomicOrdering::SeqCst), 1);
        }
        let mut bad_view = CatalogView {
            coverage: MapRoot(None),
            projections: MapRoot(None),
            ..view
        };
        bad_view.identity.digest = bad_view.identity_digest()?;
        let bad_catalog = StaticCatalog {
            ticket: TicketRecord {
                lease: test_ticket_lease(TicketId([6; 16])),
                view: bad_view,
            },
            releases: Arc::clone(&releases),
            validations: AtomicUsize::new(0),
            expires_on_validation: None,
        };
        check!(matches!(
            search_exact_acquired(
                &bad_catalog,
                &StaticSession,
                Duration::from_secs(1),
                query(),
                &mut context
            ),
            Err(IndexError::BackendViolation(_))
        ));
        let bad_selection = ViewSelection::Exact(bad_catalog.ticket.view.identity);
        check!(matches!(
            search_exact_acquired(
                &bad_catalog,
                &StaticSession,
                Duration::from_secs(1),
                TicketSearchQuery {
                    selection: bad_selection,
                    ..query()
                },
                &mut context,
            ),
            Err(IndexError::SnapshotUnavailable)
        ));
        check_eq!(releases.load(AtomicOrdering::SeqCst), 3);
        Ok(())
    }

    fn semantic_request(text: &str, top_k: u32) -> SemanticQueryRequest {
        SemanticQueryRequest {
            query_text: text.into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            lexical_scope: None,
            top_k,
        }
    }

    fn semantic_read_need() -> ResourceNeed {
        ResourceNeed {
            cpu_units: 1,
            io_bytes: 1_048_576,
            memory_bytes: 1_048_576,
            provider_units: 1,
        }
    }

    fn semantic_read_config(max_owners: u32) -> NativeSemanticReadConfig {
        NativeSemanticReadConfig {
            page_size: 2,
            max_owners,
            max_query_bytes: 64,
            need: semantic_read_need(),
        }
    }

    fn semantic_ticket(
        source_owners: MapRoot,
        coverage: MapRoot,
        sequence: u64,
    ) -> Result<TicketRecord, IndexError> {
        let mut view = CatalogView {
            identity: ViewRef {
                workspace: attempt().operation.workspace,
                epoch: 1,
                sequence,
                digest: Digest([0; 32]),
            },
            // A native reader receives the already resolved InstanceSpec. This
            // intentionally absent profile detects a redundant profile read.
            profile: ObjectRef(Digest([0xfa; 32])),
            source: SourceCut {
                owners: source_owners,
                paths: MapRoot(None),
                frontiers: MapRoot(None),
            },
            projections: MapRoot(None),
            members: MapRoot(None),
            coverage,
        };
        view.identity.digest = view
            .identity_digest()
            .map_err(|error| IndexError::InvalidInput(format!("test view identity: {error}")))?;
        Ok(TicketRecord {
            lease: test_ticket_lease(TicketId(
                [u8::try_from(sequence).map_err(|_error| IndexError::ResourceLimited)?; 16],
            )),
            view,
        })
    }

    fn semantic_source(
        store: &MemoryStore,
        path: &str,
        bytes: &[u8],
    ) -> Result<(ObjectRef, ObjectRef), Box<dyn std::error::Error>> {
        let payload = store.insert(bytes.to_vec(), vec![])?;
        let record = SourceRecord {
            path: path.into(),
            bytes: payload,
            byte_len: u64::try_from(bytes.len())?,
            text: quanta_index_contract::index::SourceTextAdmission::Utf8 {
                policy: Digest([9; 32]),
            },
        };
        Ok((
            store.insert(serde_json::to_vec(&record)?, vec![payload])?,
            payload,
        ))
    }

    fn semantic_owner_vector(
        store: &MemoryStore,
        spec: &InstanceSpec,
        recipe: &EmbeddingRecipe,
        source: ObjectRef,
        payload: ObjectRef,
        id: u8,
        values: Vec<f32>,
    ) -> Result<(Vec<u8>, ObjectRef), Box<dyn std::error::Error>> {
        semantic_owner_vector_for_owner(store, spec, recipe, source, payload, owner(id), values)
    }

    fn semantic_owner_vector_for_owner(
        store: &MemoryStore,
        spec: &InstanceSpec,
        recipe: &EmbeddingRecipe,
        source: ObjectRef,
        payload: ObjectRef,
        selected_owner: OwnerKey,
        values: Vec<f32>,
    ) -> Result<(Vec<u8>, ObjectRef), Box<dyn std::error::Error>> {
        let input = OwnerInputBinding {
            source: Some(source),
            projection: None,
            producer_context: Digest([0; 32]),
            derivation_policy: recipe.fingerprint(),
        };
        let artifact = OwnerArtifact {
            owner: selected_owner,
            input,
            recipe: recipe.fingerprint(),
            payload_ref: payload,
            reuse_identity: None,
            chunks: Vec::new(),
            vectors: vec![EmbeddingVector {
                id: b"whole-file".to_vec(),
                values,
            }],
        };
        let encoded = serde_json::to_vec(&artifact)?;
        let artifact_ref = store.insert(
            encoded.clone(),
            artifact_children(&artifact).as_ref().to_vec(),
        )?;
        let member = OwnerMemberBinding {
            instance: spec.instance,
            owner: selected_owner,
            input,
            format: spec.format,
            artifact: artifact_ref,
            logical_commitment: commitment(&encoded),
        };
        let mut member_children = vec![artifact_ref, source];
        member_children.sort_unstable();
        member_children.dedup();
        let member_ref = store.insert(serde_json::to_vec(&member)?, member_children)?;
        let coverage = OwnerCoverage {
            owner: selected_owner,
            desired: input,
            covered: Some(input),
            searchable: Some(member_ref),
        };
        let mut coverage_children = vec![member_ref, source];
        coverage_children.sort_unstable();
        coverage_children.dedup();
        let coverage_ref = store.insert(serde_json::to_vec(&coverage)?, coverage_children)?;
        Ok((owner_map_key(selected_owner).to_vec(), coverage_ref))
    }

    fn semantic_group_coverage(
        store: &MemoryStore,
        spec: &InstanceSpec,
        source: ObjectRef,
        status: CoverageStatus,
    ) -> Result<(CoverageBinding, ObjectRef), Box<dyn std::error::Error>> {
        let desired = InputBinding {
            root: MapRoot(Some(source)),
            schema: spec.schema,
            policy: spec.policy,
            context: Digest([0; 32]),
        };
        let owners = MapRoot(Some(
            store.insert(b"semantic test owner root".to_vec(), vec![source])?,
        ));
        let binding = CoverageBinding {
            instance: spec.instance,
            desired,
            covered: matches!(status, CoverageStatus::Complete).then_some(desired),
            searchable: owners.0,
            status,
            owners,
        };
        let mut children = vec![source];
        children.extend(owners.0);
        match status {
            CoverageStatus::Partial { missing } => children.extend(missing.0),
            CoverageStatus::Pending { previous_missing } => children.extend(previous_missing.0),
            CoverageStatus::Complete => {}
        }
        children.sort_unstable();
        children.dedup();
        let reference = store.insert(serde_json::to_vec(&binding)?, children)?;
        Ok((binding, reference))
    }

    #[test]
    fn native_semantic_reader_rejects_invalid_and_cancelled_requests_before_provider()
    -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or("fixed test deadline overflow")?;
        let calls = Arc::new(AtomicUsize::new(0));
        let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
            calls: Arc::clone(&calls),
            deadline,
            normalization: EmbeddingNormalization::L2Unit,
        })?);
        let recipe = bridge.recipe(1024)?;
        let spec = instance(&recipe);
        let builder = NativeSemantic::new(Arc::clone(&bridge), recipe.clone())?;
        builder.validate_instance(&spec)?;
        let reader =
            NativeSemanticReader::new(Arc::clone(&bridge), recipe, semantic_read_config(8))?;
        reader.validate_instance(&spec)?;
        let ticket = semantic_ticket(MapRoot(None), MapRoot(None), 1)?;
        let store = MemoryStore::default();
        let maps = TestMap::default();
        let mut constrained = semantic_request("query", 1);
        let _inserted = constrained
            .constraints
            .language_any_of
            .insert(quanta_index_contract::lex::LanguageCode::new("rust")?);
        let mut scoped = semantic_request("query", 1);
        scoped.lexical_scope = Some(quanta_index_contract::TextQueryRequest {
            syntax: quanta_index_contract::TextQuerySyntax::Native,
            query_text: "needle".into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k: 1,
            cursor: None,
        });
        for request in [
            semantic_request("", 1),
            semantic_request("x", 0),
            semantic_request("x", 4097),
            semantic_request(&"x".repeat(65), 1),
            constrained,
            scoped,
        ] {
            let request: SemanticReadRequest = request.into();
            let mut work = budget()?;
            check!(matches!(
                reader.query(
                    &request,
                    &mut QueryReadContext {
                        ticket: &ticket,
                        instance: &spec,
                        objects: &store,
                        maps: &maps,
                        budget: &mut work,
                    },
                ),
                Err(IndexError::InvalidInput(_))
            ));
        }
        check_eq!(calls.load(AtomicOrdering::SeqCst), 0);
        let request_budget = RequestBudgetV1::until(deadline);
        request_budget.cancel_handle().cancel();
        let mut work = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 100_000,
                mutations: 100,
            },
            request_budget,
        );
        check!(matches!(
            reader.query(
                &semantic_request("query", 1).into(),
                &mut QueryReadContext {
                    ticket: &ticket,
                    instance: &spec,
                    objects: &store,
                    maps: &maps,
                    budget: &mut work,
                },
            ),
            Err(IndexError::Cancelled)
        ));
        check_eq!(calls.load(AtomicOrdering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn native_semantic_reader_ranks_pinned_owners_and_keeps_coverage_distinct_from_scan_cap()
    -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or("fixed test deadline overflow")?;
        let calls = Arc::new(AtomicUsize::new(0));
        let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
            calls: Arc::clone(&calls),
            deadline,
            normalization: EmbeddingNormalization::L2Unit,
        })?);
        let recipe = bridge.recipe(1024)?;
        let spec = instance(&recipe);
        let builder = NativeSemantic::new(Arc::clone(&bridge), recipe.clone())?;
        builder.validate_instance(&spec)?;
        let reader = NativeSemanticReader::new(
            Arc::clone(&bridge),
            recipe.clone(),
            semantic_read_config(8),
        )?;
        let capped = NativeSemanticReader::new(
            Arc::clone(&bridge),
            recipe.clone(),
            semantic_read_config(1),
        )?;
        let store = MemoryStore::default();
        let (old_source, old_payload) = semantic_source(&store, "old.rs", b"old")?;
        let old_owners = vec![
            semantic_owner_vector(
                &store,
                &spec,
                &recipe,
                old_source,
                old_payload,
                1,
                vec![1.0, 0.0],
            )?,
            semantic_owner_vector(
                &store,
                &spec,
                &recipe,
                old_source,
                old_payload,
                2,
                vec![0.0, 1.0],
            )?,
        ];
        let (old_binding, old_coverage) =
            semantic_group_coverage(&store, &spec, old_source, CoverageStatus::Complete)?;
        let old_ticket =
            semantic_ticket(MapRoot(Some(old_source)), MapRoot(Some(old_coverage)), 1)?;
        // Publish a different source/coverage root after the old ticket was
        // selected. The reader must continue to use the old ticket's roots.
        let (new_source, new_payload) = semantic_source(&store, "new.rs", b"new")?;
        let new_owners = vec![semantic_owner_vector(
            &store,
            &spec,
            &recipe,
            new_source,
            new_payload,
            3,
            vec![0.0, 1.0],
        )?];
        let (new_binding, new_coverage) =
            semantic_group_coverage(&store, &spec, new_source, CoverageStatus::Complete)?;
        let new_ticket =
            semantic_ticket(MapRoot(Some(new_source)), MapRoot(Some(new_coverage)), 2)?;
        let (pending_binding, pending_coverage) = semantic_group_coverage(
            &store,
            &spec,
            old_source,
            CoverageStatus::Pending {
                previous_missing: MapRoot(None),
            },
        )?;
        let pending_ticket = semantic_ticket(
            MapRoot(Some(old_source)),
            MapRoot(Some(pending_coverage)),
            3,
        )?;
        let member_key = {
            let mut key = b"member\0".to_vec();
            key.extend_from_slice(&spec.instance.0);
            key
        };
        let maps = TestMap {
            entries: Vec::new(),
            rooted_entries: vec![
                (
                    MapRoot(Some(old_source)),
                    old_owners
                        .iter()
                        .map(|(key, _)| (key.clone(), old_source))
                        .collect(),
                ),
                (
                    MapRoot(Some(new_source)),
                    new_owners
                        .iter()
                        .map(|(key, _)| (key.clone(), new_source))
                        .collect(),
                ),
                (old_binding.owners, old_owners),
                (new_binding.owners, new_owners),
                (
                    MapRoot(Some(old_coverage)),
                    vec![(member_key.clone(), old_coverage)],
                ),
                (
                    MapRoot(Some(new_coverage)),
                    vec![(member_key.clone(), new_coverage)],
                ),
                (
                    MapRoot(Some(pending_coverage)),
                    vec![(member_key, pending_coverage)],
                ),
            ],
            inline_rooted_entries: Vec::new(),
            fail: false,
        };
        let request: SemanticReadRequest = semantic_request("query", 2).into();
        let mut work = WorkBudget::with_request(
            WorkLimits {
                reads: 1000,
                writes: 1000,
                bytes: 100_000_000,
                mutations: 1000,
            },
            RequestBudgetV1::until(deadline),
        );
        let mut run = |reader: &NativeSemanticReader<_>, ticket: &TicketRecord| {
            reader.query(
                &request,
                &mut QueryReadContext {
                    ticket,
                    instance: &spec,
                    objects: &store,
                    maps: &maps,
                    budget: &mut work,
                },
            )
        };
        let old = run(&reader, &old_ticket)?;
        check_eq!(old.view(), old_ticket.view.identity);
        check_eq!(old.coverage(), &QueryCoverage::Async(Box::new(old_binding)));
        check!(old.exhaustive());
        check!(old.result().traversal == NativeSemanticTraversal::ExactExhausted);
        check_eq!(old.result().scanned_owners, 2);
        check_eq!(old.result().hits.len(), 2);
        for hit in &old.result().hits {
            check_eq!(hit.vector_id.as_slice(), b"whole-file");
        }
        check_eq!(
            old.result()
                .hits
                .first()
                .ok_or("missing first old hit")?
                .owner,
            owner(1)
        );
        check_eq!(
            old.result()
                .hits
                .first()
                .ok_or("missing first old hit")?
                .cosine
                .total_cmp(&1.0),
            Ordering::Equal
        );
        check_eq!(
            old.result()
                .hits
                .get(1)
                .ok_or("missing second old hit")?
                .owner,
            owner(2)
        );
        check_eq!(
            old.result()
                .hits
                .get(1)
                .ok_or("missing second old hit")?
                .cosine
                .total_cmp(&0.0),
            Ordering::Equal
        );
        let latest = run(&reader, &new_ticket)?;
        check_eq!(latest.view(), new_ticket.view.identity);
        check_eq!(latest.result().hits.len(), 1);
        check_eq!(
            latest
                .result()
                .hits
                .first()
                .ok_or("missing latest hit")?
                .owner,
            owner(3)
        );
        let old_again = run(&reader, &old_ticket)?;
        check_eq!(old_again.result().hits, old.result().hits);
        let limited = run(&capped, &old_ticket)?;
        check_eq!(
            limited.coverage(),
            &QueryCoverage::Async(Box::new(old_binding))
        );
        check!(!limited.exhaustive());
        check!(limited.result().traversal == NativeSemanticTraversal::ExactCapped);
        check_eq!(limited.result().scanned_owners, 1);
        let pending = run(&reader, &pending_ticket)?;
        check_eq!(
            pending.coverage(),
            &QueryCoverage::Async(Box::new(pending_binding))
        );
        check!(!pending.exhaustive());
        check!(pending.result().traversal == NativeSemanticTraversal::ExactExhausted);
        check_eq!(calls.load(AtomicOrdering::SeqCst), 5);
        Ok(())
    }

    #[test]
    fn native_semantic_overlay_replaces_deletes_and_ranks_with_durable_candidates()
    -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or("fixed test deadline overflow")?;
        let calls = Arc::new(AtomicUsize::new(0));
        let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
            calls: Arc::clone(&calls),
            deadline,
            normalization: EmbeddingNormalization::L2Unit,
        })?);
        let recipe = bridge.recipe(1024)?;
        let spec = instance(&recipe);
        let reader = NativeSemanticReader::new(bridge, recipe.clone(), semantic_read_config(5))?;
        let store = MemoryStore::default();
        let (source, payload) = semantic_source(&store, "old.rs", b"old")?;
        let (second_source, second_payload) = semantic_source(&store, "old2.rs", b"old")?;
        let rows = vec![
            semantic_owner_vector(&store, &spec, &recipe, source, payload, 1, vec![0.0, 1.0])?,
            semantic_owner_vector(
                &store,
                &spec,
                &recipe,
                second_source,
                second_payload,
                2,
                vec![1.0, 0.0],
            )?,
        ];
        let (binding, coverage) =
            semantic_group_coverage(&store, &spec, source, CoverageStatus::Complete)?;
        let mut ticket = semantic_ticket(MapRoot(Some(source)), MapRoot(Some(coverage)), 1)?;
        let path_root = MapRoot(Some(
            store.insert(b"overlay source paths".to_vec(), vec![])?,
        ));
        ticket.view.source.paths = path_root;
        ticket.view.identity.digest = ticket.view.identity_digest()?;
        let mut member_key = b"member\0".to_vec();
        member_key.extend_from_slice(&spec.instance.0);
        let maps = TestMap {
            entries: Vec::new(),
            rooted_entries: vec![
                (
                    MapRoot(Some(source)),
                    vec![
                        (owner_map_key(owner(1)).to_vec(), source),
                        (owner_map_key(owner(2)).to_vec(), second_source),
                    ],
                ),
                (binding.owners, rows),
                (MapRoot(Some(coverage)), vec![(member_key, coverage)]),
            ],
            inline_rooted_entries: vec![(
                path_root,
                vec![
                    (
                        b"path\0old.rs".to_vec(),
                        quanta_index_core::index::path_owner_map_value(owner(1))?,
                    ),
                    (
                        b"path\0old2.rs".to_vec(),
                        quanta_index_core::index::path_owner_map_value(owner(2))?,
                    ),
                ],
            )],
            fail: false,
        };
        let mut request = SemanticReadRequest {
            lexical_eligibility: None,
            query: semantic_request("query", 2),
            overlay: vec![
                SemanticOverlayDocument {
                    path: Some(Arc::<str>::from("replacement.rs")),
                    owner: owner(1),
                    revision: 7,
                    text: Some(Arc::<str>::from("replacement")),
                },
                SemanticOverlayDocument {
                    path: None,
                    owner: owner(2),
                    revision: 8,
                    text: None,
                },
                SemanticOverlayDocument {
                    path: Some(Arc::<str>::from("new.rs")),
                    owner: owner(3),
                    revision: 9,
                    text: Some(Arc::<str>::from("new")),
                },
            ],
            overlay_memory: None,
        };
        let mut work = WorkBudget::with_request(
            WorkLimits {
                reads: 1000,
                writes: 1000,
                bytes: 100_000_000,
                mutations: 1000,
            },
            RequestBudgetV1::until(deadline),
        );
        let result = reader.query(
            &request,
            &mut QueryReadContext {
                ticket: &ticket,
                instance: &spec,
                objects: &store,
                maps: &maps,
                budget: &mut work,
            },
        )?;
        check_eq!(result.coverage(), &QueryCoverage::Async(Box::new(binding)));
        check!(result.exhaustive());
        check_eq!(result.result().scanned_owners, 5);
        check_eq!(result.result().skipped_stale, 0);
        check_eq!(result.result().hits.len(), 2);
        let first = result
            .result()
            .hits
            .first()
            .ok_or("missing first overlay hit")?;
        let second = result
            .result()
            .hits
            .get(1)
            .ok_or("missing second overlay hit")?;
        check_eq!(first.owner, owner(1));
        check_eq!(
            first.overlay,
            Some(SemanticOverlayHit {
                revision: 7,
                chunk: None
            })
        );
        check_eq!(first.vector_id.as_slice(), b"whole-file");
        check_eq!(second.owner, owner(3));
        check_eq!(
            second.overlay,
            Some(SemanticOverlayHit {
                revision: 9,
                chunk: None
            })
        );
        check_eq!(calls.load(AtomicOrdering::SeqCst), 3);
        request.query.constraints = QueryConstraintSetV1::from_exact_repo_relative_path(
            quanta_index_contract::ExactRepoRelativePathV1::new("new.rs")?,
        );
        let filtered = reader.query(
            &request,
            &mut QueryReadContext {
                ticket: &ticket,
                instance: &spec,
                objects: &store,
                maps: &maps,
                budget: &mut work,
            },
        )?;
        check_eq!(filtered.result().hits.len(), 1);
        let hit = filtered
            .result()
            .hits
            .first()
            .ok_or("filtered overlay hit")?;
        check_eq!(hit.owner, owner(3));
        check_eq!(
            hit.overlay,
            Some(SemanticOverlayHit {
                revision: 9,
                chunk: None
            })
        );
        check!(filtered.exhaustive());
        // Only the matching editor text is embedded; an excluded replacement
        // still masks its selected durable owner before candidate ranking.
        check_eq!(calls.load(AtomicOrdering::SeqCst), 5);
        Ok(())
    }

    #[test]
    fn native_semantic_overlay_rejects_uncertified_projection_and_owner_overflow_before_provider()
    -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or("fixed test deadline overflow")?;
        let calls = Arc::new(AtomicUsize::new(0));
        let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
            calls: Arc::clone(&calls),
            deadline,
            normalization: EmbeddingNormalization::L2Unit,
        })?);
        let projected = bridge.projection_recipe(
            ProjectionId([1; 16]),
            ProducerId([2; 16]),
            SchemaId([3; 16]),
            1024,
        )?;
        let projected_reader =
            NativeSemanticReader::new(Arc::clone(&bridge), projected, semantic_read_config(5))?;
        let one = SemanticReadRequest {
            lexical_eligibility: None,
            query: semantic_request("query", 1),
            overlay: vec![SemanticOverlayDocument {
                path: None,
                owner: owner(1),
                revision: 1,
                text: Some(Arc::<str>::from("uncertified editor text")),
            }],
            overlay_memory: None,
        };
        check!(matches!(
            projected_reader.reservation(&one),
            Err(IndexError::SnapshotUnavailable)
        ));
        let source_reader = NativeSemanticReader::new(
            Arc::clone(&bridge),
            bridge.recipe(1024)?,
            semantic_read_config(1),
        )?;
        let over_limit = SemanticReadRequest {
            lexical_eligibility: None,
            query: semantic_request("query", 1),
            overlay: vec![
                SemanticOverlayDocument {
                    path: None,
                    owner: owner(1),
                    revision: 1,
                    text: None,
                },
                SemanticOverlayDocument {
                    path: None,
                    owner: owner(2),
                    revision: 2,
                    text: None,
                },
            ],
            overlay_memory: None,
        };
        check!(matches!(
            source_reader.reservation(&over_limit),
            Err(IndexError::ResourceLimited)
        ));
        let zero_revision = SemanticReadRequest {
            lexical_eligibility: None,
            query: semantic_request("query", 1),
            overlay: vec![SemanticOverlayDocument {
                path: None,
                owner: owner(1),
                revision: 0,
                text: Some(Arc::<str>::from("unsaved")),
            }],
            overlay_memory: None,
        };
        check!(matches!(
            source_reader.reservation(&zero_revision),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(calls.load(AtomicOrdering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn native_semantic_overlay_chunks_keep_source_utf8_spans_without_durable_source_refs()
    -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or("fixed test deadline overflow")?;
        let calls = Arc::new(AtomicUsize::new(0));
        let bridge = Arc::new(TextEmbeddingBridge::with_utf8_chunks(
            FixedTextProvider {
                calls: Arc::clone(&calls),
                deadline,
                normalization: EmbeddingNormalization::L2Unit,
            },
            Utf8ChunkPolicy { max_utf8_bytes: 4 },
        )?);
        let recipe = bridge.chunk_recipe(1024, 2)?;
        let spec = instance(&recipe);
        let reader = NativeSemanticReader::new(bridge, recipe, semantic_read_config(2))?;
        let empty_input = InputBinding {
            root: MapRoot(None),
            schema: spec.schema,
            policy: spec.policy,
            context: Digest([0; 32]),
        };
        let binding = CoverageBinding {
            instance: spec.instance,
            desired: empty_input,
            covered: Some(empty_input),
            searchable: None,
            status: CoverageStatus::Complete,
            owners: MapRoot(None),
        };
        let store = MemoryStore::default();
        let coverage = store.insert(serde_json::to_vec(&binding)?, vec![])?;
        let ticket = semantic_ticket(MapRoot(None), MapRoot(Some(coverage)), 1)?;
        let mut key = b"member\0".to_vec();
        key.extend_from_slice(&spec.instance.0);
        let maps = TestMap {
            entries: vec![(key, coverage)],
            rooted_entries: Vec::new(),
            inline_rooted_entries: Vec::new(),
            fail: false,
        };
        let request = SemanticReadRequest {
            lexical_eligibility: None,
            query: semantic_request("query", 2),
            overlay: vec![SemanticOverlayDocument {
                path: None,
                owner: owner(1),
                revision: 12,
                text: Some(Arc::<str>::from("abcdefgh")),
            }],
            overlay_memory: None,
        };
        let mut work = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            RequestBudgetV1::until(deadline),
        );
        let result = reader.query(
            &request,
            &mut QueryReadContext {
                ticket: &ticket,
                instance: &spec,
                objects: &store,
                maps: &maps,
                budget: &mut work,
            },
        )?;
        check_eq!(result.coverage(), &QueryCoverage::Async(Box::new(binding)));
        check!(result.exhaustive());
        check_eq!(result.result().scanned_owners, 1);
        check_eq!(result.result().hits.len(), 2);
        let spans = [(0, 4, b"abcd".as_slice()), (4, 8, b"efgh".as_slice())];
        for (start, end, input) in spans {
            let hit = result
                .result()
                .hits
                .iter()
                .find(|hit| {
                    hit.overlay
                        .and_then(|overlay| overlay.chunk)
                        .is_some_and(|chunk| chunk.start == start && chunk.end == end)
                })
                .ok_or("missing fixed overlay UTF-8 span")?;
            let overlay = hit.overlay.ok_or("overlay origin missing")?;
            let chunk = overlay.chunk.ok_or("overlay chunk missing")?;
            check_eq!(overlay.revision, 12);
            check_eq!(chunk.input_digest, Digest(Sha256::digest(input).into()));
            check_eq!(hit.vector_id.as_slice(), chunk.id.0.as_slice());
            check_eq!(hit.chunk, None);
        }
        check_eq!(calls.load(AtomicOrdering::SeqCst), 2);
        Ok(())
    }

    #[test]
    fn native_semantic_reader_refuses_inflight_cancel_and_provider_revision_drift()
    -> Result<(), Box<dyn std::error::Error>> {
        for cancel_during_call in [true, false] {
            let deadline = Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or("fixed test deadline overflow")?;
            let original = RequestBudgetV1::until(deadline);
            let calls = Arc::new(AtomicUsize::new(0));
            let bridge = Arc::new(TextEmbeddingBridge::new(InterruptingTextProvider {
                calls: Arc::clone(&calls),
                deadline,
                cancel_during_call,
                revision_changed: AtomicBool::new(false),
            })?);
            let recipe = bridge.recipe(1024)?;
            let spec = instance(&recipe);
            let reader = NativeSemanticReader::new(bridge, recipe, semantic_read_config(1))?;
            let store = MemoryStore::default();
            let (source, _payload) = semantic_source(&store, "a.rs", b"source")?;
            let (_binding, coverage) =
                semantic_group_coverage(&store, &spec, source, CoverageStatus::Complete)?;
            let ticket = semantic_ticket(MapRoot(Some(source)), MapRoot(Some(coverage)), 1)?;
            let mut key = b"member\0".to_vec();
            key.extend_from_slice(&spec.instance.0);
            let maps = TestMap {
                entries: Vec::new(),
                rooted_entries: vec![(MapRoot(Some(coverage)), vec![(key, coverage)])],
                inline_rooted_entries: Vec::new(),
                fail: false,
            };
            let mut work = WorkBudget::with_request(
                WorkLimits {
                    reads: 100,
                    writes: 100,
                    bytes: 100_000,
                    mutations: 100,
                },
                original.clone(),
            );
            let result = reader.query(
                &semantic_request("query", 1).into(),
                &mut QueryReadContext {
                    ticket: &ticket,
                    instance: &spec,
                    objects: &store,
                    maps: &maps,
                    budget: &mut work,
                },
            );
            check_eq!(calls.load(AtomicOrdering::SeqCst), 1);
            if cancel_during_call {
                check!(matches!(result, Err(IndexError::Cancelled)));
                check!(original.is_cancelled());
            } else {
                check!(matches!(result, Err(IndexError::BackendViolation(_))));
                check!(!original.is_cancelled());
            }
        }
        Ok(())
    }

    #[test]
    fn native_semantic_reader_skips_provider_for_complete_empty_selected_scope()
    -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or("fixed test deadline overflow")?;
        let calls = Arc::new(AtomicUsize::new(0));
        let bridge = Arc::new(TextEmbeddingBridge::new(FixedTextProvider {
            calls: Arc::clone(&calls),
            deadline,
            normalization: EmbeddingNormalization::L2Unit,
        })?);
        let recipe = bridge.recipe(1024)?;
        let spec = instance(&recipe);
        let reader = NativeSemanticReader::new(bridge, recipe, semantic_read_config(1))?;
        let empty_input = InputBinding {
            root: MapRoot(None),
            schema: spec.schema,
            policy: spec.policy,
            context: Digest([0; 32]),
        };
        let coverage = CoverageBinding {
            instance: spec.instance,
            desired: empty_input,
            covered: Some(empty_input),
            searchable: None,
            status: CoverageStatus::Complete,
            owners: MapRoot(None),
        };
        let store = MemoryStore::default();
        let coverage_ref = store.insert(serde_json::to_vec(&coverage)?, vec![])?;
        let ticket = semantic_ticket(MapRoot(None), MapRoot(Some(coverage_ref)), 1)?;
        let mut key = b"member\0".to_vec();
        key.extend_from_slice(&spec.instance.0);
        let maps = TestMap {
            entries: vec![(key, coverage_ref)],
            rooted_entries: Vec::new(),
            inline_rooted_entries: Vec::new(),
            fail: false,
        };
        let mut work = WorkBudget::with_request(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 100_000,
                mutations: 100,
            },
            RequestBudgetV1::until(deadline),
        );
        let result = reader.query(
            &semantic_request("query", 1).into(),
            &mut QueryReadContext {
                ticket: &ticket,
                instance: &spec,
                objects: &store,
                maps: &maps,
                budget: &mut work,
            },
        )?;
        check_eq!(result.view(), ticket.view.identity);
        check_eq!(result.coverage(), &QueryCoverage::Async(Box::new(coverage)));
        check!(result.exhaustive());
        check!(result.result().hits.is_empty());
        check_eq!(result.result().scanned_owners, 0);
        check_eq!(result.result().skipped_stale, 0);
        check!(result.result().traversal == NativeSemanticTraversal::ExactExhausted);
        check_eq!(calls.load(AtomicOrdering::SeqCst), 0);
        Ok(())
    }
}
