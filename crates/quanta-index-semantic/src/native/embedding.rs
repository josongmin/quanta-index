//! Native, owner-local exact vector materialization and bounded cosine oracle.
//! No ANN generation or producer dependency inference is hidden in this module.

use std::{cmp::Ordering, collections::BinaryHeap};

use quanta_index_contract::EmbeddingNormalization;
use quanta_index_core::index::{
    AttemptContext, CoverageBinding, CoverageStatus, Digest, GroupActivation, IndexError,
    IndexInstanceId, IndexProfile, InputDomain, InstanceSpec, MapRoot, ObjectRef, OwnerCoverage,
    OwnerInputBinding, OwnerKey, OwnerMemberBinding, ProducerId, ProjectionBinding, ProjectionId,
    ProjectionOwnerContribution, SchemaId, SourceRecord, StorageFormatId, ViewSelection,
    WorkBudget, WorkUsage, owner_map_key,
    ports::{
        CatalogReadPort, ImmutableObjectPort, IndexFuture, IndexOwnerBuildPort, PersistentMapPort,
    },
};
use quanta_index_core::{CoreError, RequestBudgetV1, TextEmbeddingProvider};
use sha2::{Digest as _, Sha256};

const RECIPE_DOMAIN: &[u8] = b"quanta-index-native-semantic-recipe-v1\0";
const MODEL_DOMAIN: &[u8] = b"quanta-index-native-semantic-text-model-v1\0";
// One vector from the complete selected UTF-8 payload. The domain stays
// stable for SourceUtf8; input selection is separately pinned by the recipe.
const WHOLE_FILE_POLICY: &[u8] = b"quanta-index-native-semantic-utf8-whole-file-one-vector-v1\0";
const COMMIT_DOMAIN: &[u8] = b"quanta-index-native-semantic-owner-v1\0";
const MAX_DIMENSION: u32 = 4096;
const MAX_VECTORS: u32 = 4096;
const MAX_CELLS: u64 = 200_000;
const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_VECTOR_ID: usize = 1024;
const MAX_PAGE: u32 = 4096;
const MAX_TOP_K: u32 = 4096;
const MAX_OWNER_SCAN: u32 = 100_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmbeddingInputPolicy {
    SourceUtf8,
    ProjectionUtf8 {
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
    pub max_vectors: u32,
    pub max_source_bytes: u64,
}

impl EmbeddingRecipe {
    pub fn validate(&self) -> Result<(), IndexError> {
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

pub struct EmbeddingInput<'a> {
    pub owner: OwnerKey,
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
    fn embed<'a>(&'a self, input: EmbeddingInput<'a>) -> IndexFuture<'a, Vec<EmbeddingVector>>;
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
        let output_policy = Digest(Sha256::digest(WHOLE_FILE_POLICY).into());
        Ok(Self {
            provider,
            model_id,
            model_revision,
            model_identity,
            dimension,
            normalization,
            output_policy,
        })
    }

    pub fn recipe(&self, max_source_bytes: u64) -> Result<EmbeddingRecipe, IndexError> {
        let recipe = EmbeddingRecipe {
            input_policy: EmbeddingInputPolicy::SourceUtf8,
            model_identity: self.model_identity,
            dimension: self.dimension,
            normalization: self.normalization,
            output_policy: self.output_policy,
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
    ) -> Result<Vec<f32>, IndexError> {
        self.verify_descriptor()?;
        budget_checkpoint(budget)?;
        let mut vectors = self
            .provider
            .embed_batch_within(&[text], budget)
            .map_err(|error| provider_error(&error, budget))?;
        budget_checkpoint(budget)?;
        self.verify_descriptor()?;
        if vectors.len() != 1 {
            return Err(IndexError::BackendViolation(
                "text provider returned a non-singleton query batch".into(),
            ));
        }
        let vector = vectors.pop().ok_or_else(|| {
            IndexError::BackendViolation("text provider omitted query vector".into())
        })?;
        validate_bridge_vector(&vector, self.dimension, self.normalization)?;
        Ok(vector)
    }
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

    fn embed<'a>(&'a self, input: EmbeddingInput<'a>) -> IndexFuture<'a, Vec<EmbeddingVector>> {
        Box::pin(async move {
            self.verify_descriptor()?;
            if input.recipe.model_identity != self.model_identity
                || input.recipe.dimension != self.dimension
                || input.recipe.normalization != self.normalization
                || input.recipe.output_policy != self.output_policy
                || (matches!(input.recipe.input_policy, EmbeddingInputPolicy::SourceUtf8)
                    && input.source.is_none())
            {
                return Err(IndexError::InvalidInput(
                    "text provider recipe mismatch".into(),
                ));
            }
            let text = std::str::from_utf8(input.source_bytes).map_err(|_utf8_error| {
                IndexError::InvalidInput("semantic input is not UTF-8".into())
            })?;
            budget_checkpoint(input.request_budget)?;
            let mut vectors = self
                .provider
                .embed_batch_within(&[text], input.request_budget)
                .map_err(|error| provider_error(&error, input.request_budget))?;
            budget_checkpoint(input.request_budget)?;
            self.verify_descriptor()?;
            if vectors.len() != 1 {
                return Err(IndexError::BackendViolation(
                    "text provider returned a non-singleton owner batch".into(),
                ));
            }
            let vector = vectors.pop().ok_or_else(|| {
                IndexError::BackendViolation("text provider omitted owner vector".into())
            })?;
            validate_bridge_vector(&vector, self.dimension, self.normalization)?;
            Ok(vec![EmbeddingVector {
                id: b"whole-file".to_vec(),
                values: vector,
            }])
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
    budget_checkpoint(budget)
        .err()
        .unwrap_or_else(|| IndexError::Storage(format!("text embedding provider: {error}")))
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
    vectors: Vec<EmbeddingVector>,
}
quanta_index_contract::native_serde_struct!(OwnerArtifact {
    owner: OwnerKey,
    input: OwnerInputBinding,
    recipe: Digest,
    payload_ref: ObjectRef,
    vectors: Vec<EmbeddingVector>,
});

pub struct SemanticCandidate {
    instance: IndexInstanceId,
    format: StorageFormatId,
    artifact: OwnerArtifact,
}

pub struct NativeSemantic<P> {
    provider: P,
    recipe: EmbeddingRecipe,
}

fn load_source_record(
    reference: ObjectRef,
    context: &mut AttemptContext<'_>,
) -> Result<SourceRecord, IndexError> {
    let stored = context.objects.get(reference, context.budget)?;
    if stored.registration.reference != reference {
        return Err(IndexError::CorruptObject(
            "source record reference mismatch".into(),
        ));
    }
    let source: SourceRecord = serde_json::from_slice(&stored.payload)
        .map_err(|error| IndexError::CorruptObject(format!("source record: {error}")))?;
    if stored.registration.children.as_slice() != [source.bytes] {
        return Err(IndexError::CorruptObject(
            "source record edges mismatch".into(),
        ));
    }
    Ok(source)
}

fn contribution_children(contribution: ProjectionOwnerContribution) -> Vec<ObjectRef> {
    contribution.object_children()
}

fn load_contribution(
    reference: ObjectRef,
    context: &mut AttemptContext<'_>,
) -> Result<ProjectionOwnerContribution, IndexError> {
    let stored = context.objects.get(reference, context.budget)?;
    if stored.registration.reference != reference {
        return Err(IndexError::CorruptObject(
            "projection contribution reference mismatch".into(),
        ));
    }
    let contribution: ProjectionOwnerContribution = serde_json::from_slice(&stored.payload)
        .map_err(|error| IndexError::CorruptObject(format!("projection contribution: {error}")))?;
    if stored.registration.children != contribution_children(contribution) {
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
        Ok(Self { provider, recipe })
    }

    #[must_use]
    pub fn recipe(&self) -> &EmbeddingRecipe {
        &self.recipe
    }
}

impl<P: EmbeddingProvider> IndexOwnerBuildPort for NativeSemantic<P> {
    type Candidate = SemanticCandidate;

    fn prepare_owner<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        owner: OwnerKey,
        input: OwnerInputBinding,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, Self::Candidate> {
        Box::pin(async move {
            self.recipe.validate()?;
            verify_provider_recipe(&self.provider, &self.recipe)?;
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
                    (Some(source.clone()), source.bytes)
                }
                EmbeddingInputPolicy::ProjectionUtf8 {
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
            if std::str::from_utf8(&bytes.payload).is_err() {
                return Err(IndexError::InvalidInput(
                    "semantic input is not UTF-8".into(),
                ));
            }
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
            let vectors = self
                .provider
                .embed(EmbeddingInput {
                    owner,
                    source: source.as_ref(),
                    source_bytes: &bytes.payload,
                    recipe: &self.recipe,
                    request_budget: context.budget.request(),
                })
                .await?;
            context.budget.checkpoint()?;
            verify_provider_recipe(&self.provider, &self.recipe).map_err(|_descriptor_error| {
                IndexError::BackendViolation(
                    "embedding provider descriptor changed during call".into(),
                )
            })?;
            validate_vectors(&vectors, &self.recipe, context.budget)?;
            Ok(SemanticCandidate {
                instance: instance.instance,
                format: instance.format,
                artifact: OwnerArtifact {
                    owner,
                    input,
                    recipe: fingerprint,
                    payload_ref,
                    vectors,
                },
            })
        })
    }

    fn verify_owner<'a>(
        &'a self,
        candidate: Self::Candidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, OwnerMemberBinding> {
        Box::pin(async move {
            context.budget.checkpoint()?;
            if candidate.artifact.recipe != self.recipe.fingerprint()
                || candidate.artifact.input.derivation_policy != self.recipe.fingerprint()
                || match self.recipe.input_policy {
                    EmbeddingInputPolicy::SourceUtf8 => {
                        candidate.artifact.input.projection.is_some()
                    }
                    EmbeddingInputPolicy::ProjectionUtf8 { .. } => {
                        candidate.artifact.input.projection.is_none()
                    }
                }
            {
                return Err(IndexError::BackendViolation(
                    "recipe changed after prepare".into(),
                ));
            }
            validate_vectors(&candidate.artifact.vectors, &self.recipe, context.budget)?;
            let estimated =
                candidate
                    .artifact
                    .vectors
                    .iter()
                    .try_fold(1024u64, |total, vector| {
                        let id = u64::try_from(vector.id.len())
                            .map_err(|_conversion_error| IndexError::ResourceLimited)?;
                        let values = u64::try_from(vector.values.len())
                            .map_err(|_conversion_error| IndexError::ResourceLimited)?;
                        total
                            .checked_add(id.checked_mul(4).ok_or(IndexError::ResourceLimited)?)
                            .and_then(|bytes| bytes.checked_add(values.checked_mul(24)?))
                            .and_then(|bytes| bytes.checked_add(128))
                            .ok_or(IndexError::ResourceLimited)
                    })?;
            context.budget.charge(WorkUsage {
                bytes: estimated,
                ..WorkUsage::default()
            })?;
            let payload = serde_json::to_vec(&candidate.artifact).map_err(|error| {
                IndexError::BackendViolation(format!("vector encoding: {error}"))
            })?;
            context.budget.charge(WorkUsage {
                bytes: u64::try_from(payload.len())
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            let mut children = vec![candidate.artifact.payload_ref];
            children.extend(candidate.artifact.input.source);
            children.extend(candidate.artifact.input.projection);
            children.sort_unstable();
            children.dedup();
            let artifact = context.put(&payload, &children)?;
            Ok(OwnerMemberBinding {
                instance: candidate.instance,
                owner: candidate.artifact.owner,
                input: candidate.artifact.input,
                format: candidate.format,
                artifact,
                logical_commitment: commitment(&payload),
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

fn validate_vectors(
    vectors: &[EmbeddingVector],
    recipe: &EmbeddingRecipe,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
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

struct SearchQuery<'a> {
    pub coverage: &'a CoverageBinding,
    pub projection: Option<ProjectionBinding>,
    pub source_owners: MapRoot,
    pub instance: IndexInstanceId,
    pub format: StorageFormatId,
    pub recipe: &'a EmbeddingRecipe,
    pub vector: &'a [f32],
    pub top_k: u32,
    pub page_size: u32,
    pub max_owners: u32,
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
    query: TicketSearchQuery<'_>,
    context: &mut ReadContext<'_>,
) -> Result<SearchResult, IndexError> {
    context.budget.checkpoint()?;
    let lease = SearchLease {
        catalog,
        ticket: catalog.acquire_ticket(query.selection)?,
        released: false,
    };
    let result = search_exact_at_view(&lease.ticket.view, &query, context);
    lease.release()?;
    result
}

// This borrow keeps the injected catalog alive and releases on panic/unwind,
// as well as the explicit result paths. A failed release conservatively keeps
// the catalog pin rather than making reclamation unsafe.
struct SearchLease<'a> {
    catalog: &'a dyn CatalogReadPort,
    ticket: quanta_index_core::index::TicketRecord,
    released: bool,
}

impl SearchLease<'_> {
    fn release(mut self) -> Result<(), IndexError> {
        let result = self.catalog.release_ticket(self.ticket.id);
        self.released = result.is_ok();
        result
    }
}

impl Drop for SearchLease<'_> {
    fn drop(&mut self) {
        if !self.released {
            drop(self.catalog.release_ticket(self.ticket.id));
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
    if spec.activation != GroupActivation::AsyncCoverage
        || spec.format != query.format
        || spec.policy != query.recipe.fingerprint()
    {
        return Err(IndexError::InvalidInput(
            "semantic instance differs from ticket profile".into(),
        ));
    }
    let mut key = b"member\0".to_vec();
    key.extend_from_slice(&query.instance.0);
    let coverage_ref = context
        .maps
        .lookup(view.coverage, &key, context.budget)?
        .ok_or(IndexError::SnapshotUnavailable)?;
    let stored = context.objects.get(coverage_ref, context.budget)?;
    if stored.registration.reference != coverage_ref {
        return Err(IndexError::CorruptObject(
            "coverage reference mismatch".into(),
        ));
    }
    let coverage: CoverageBinding = serde_json::from_slice(&stored.payload)
        .map_err(|error| IndexError::CorruptObject(format!("coverage binding: {error}")))?;
    let mut children = Vec::new();
    children.extend(coverage.desired.root.0);
    children.extend(coverage.owners.0);
    if let CoverageStatus::Partial { missing } = coverage.status {
        children.extend(missing.0);
    }
    children.extend(coverage.searchable);
    children.sort_unstable();
    children.dedup();
    let projection = match (query.recipe.input_policy, spec.input_domain) {
        (EmbeddingInputPolicy::SourceUtf8, InputDomain::SourceLocal) => None,
        (
            EmbeddingInputPolicy::ProjectionUtf8 {
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
            let mut key = b"projection\0".to_vec();
            key.extend_from_slice(&projection.0);
            let reference = context
                .maps
                .lookup(view.projections, &key, context.budget)?
                .ok_or(IndexError::SnapshotUnavailable)?;
            let object = context.objects.get(reference, context.budget)?;
            let binding: ProjectionBinding =
                serde_json::from_slice(&object.payload).map_err(|error| {
                    IndexError::CorruptObject(format!("projection binding: {error}"))
                })?;
            if object.registration.reference != reference
                || object.registration.children != binding.object_children()
                || binding.projection != projection
                || binding.producer != producer
                || binding.schema != schema
                || binding.source != view.source
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
    if coverage.instance != query.instance
        || coverage.desired.root != desired_root
        || coverage.desired.schema != spec.schema
        || coverage.desired.policy != spec.policy
        || desired_context.is_some_and(|context| coverage.desired.context != context)
        || coverage.searchable != coverage.owners.0
        || stored.registration.children != children
    {
        return Err(IndexError::CorruptObject(
            "ticket coverage binding mismatch".into(),
        ));
    }
    search_exact(
        &SearchQuery {
            coverage: &coverage,
            projection,
            source_owners: view.source.owners,
            instance: query.instance,
            format: query.format,
            recipe: query.recipe,
            vector: query.vector,
            top_k: query.top_k,
            page_size: query.page_size,
            max_owners: query.max_owners,
        },
        context,
    )
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub owner: OwnerKey,
    pub vector_id: Vec<u8>,
    pub cosine: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchResult {
    pub hits: Vec<SearchHit>,
    pub scanned_owners: u32,
    pub skipped_stale: u32,
    pub partial: bool,
    pub exhaustive: bool,
}

#[derive(Clone)]
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
fn search_exact(
    query: &SearchQuery<'_>,
    context: &mut ReadContext<'_>,
) -> Result<SearchResult, IndexError> {
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
    let mut complete = declared_complete && global_current;
    let mut scanned_owners = 0u32;
    let mut skipped_stale = 0u32;
    let mut after: Option<Vec<u8>> = None;
    let mut heap = BinaryHeap::new();
    loop {
        context.budget.checkpoint()?;
        let page = context.maps.scan(
            query.coverage.owners,
            after.as_deref(),
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
            if scanned_owners == query.max_owners {
                complete = false;
                break;
            }
            scanned_owners = scanned_owners
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            let stored = context.objects.get(*reference, context.budget)?;
            let owner: OwnerCoverage = serde_json::from_slice(&stored.payload)
                .map_err(|error| IndexError::CorruptObject(format!("owner coverage: {error}")))?;
            if *key != owner_map_key(owner.owner) {
                return Err(IndexError::CorruptObject(
                    "coverage owner key mismatch".into(),
                ));
            }
            let contribution = if let Some(binding) = query.projection {
                let current = context.maps.lookup(binding.owners, key, context.budget)?;
                if current != owner.desired.projection {
                    return Err(IndexError::CorruptObject(
                        "projection owner root differs from coverage".into(),
                    ));
                }
                let reference = current.ok_or_else(|| {
                    IndexError::CorruptObject("projection owner is absent".into())
                })?;
                let object = context.objects.get(reference, context.budget)?;
                let contribution: ProjectionOwnerContribution =
                    serde_json::from_slice(&object.payload).map_err(|error| {
                        IndexError::CorruptObject(format!("projection owner: {error}"))
                    })?;
                if object.registration.reference != reference
                    || object.registration.children != contribution.object_children()
                    || contribution.owner != owner.owner
                    || contribution.projection != binding.projection
                    || contribution.producer != binding.producer
                    || contribution.source != owner.desired.source
                    || contribution.source_owner.is_some() != contribution.source.is_some()
                    || contribution_context(contribution) != owner.desired.producer_context
                {
                    return Err(IndexError::CorruptObject(
                        "projection owner stamp mismatch".into(),
                    ));
                }
                if let (Some(source_owner), Some(source)) =
                    (contribution.source_owner, contribution.source)
                {
                    let current_source = context.maps.lookup(
                        query.source_owners,
                        &owner_map_key(source_owner),
                        context.budget,
                    )?;
                    if current_source != Some(source) {
                        return Err(IndexError::CorruptObject(
                            "projection source owner stamp mismatch".into(),
                        ));
                    }
                }
                Some(contribution)
            } else {
                if owner.desired.projection.is_some() {
                    return Err(IndexError::CorruptObject(
                        "source owner has projection input".into(),
                    ));
                }
                None
            };
            if owner.desired.derivation_policy != recipe_fingerprint {
                return Err(IndexError::CorruptObject(
                    "owner derivation policy mismatch".into(),
                ));
            }
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
                complete = false;
                continue;
            }
            let member_ref = owner.searchable.ok_or_else(|| {
                IndexError::CorruptObject("current owner has no searchable artifact".into())
            })?;
            if !stored.registration.children.contains(&member_ref) {
                return Err(IndexError::CorruptObject(
                    "owner coverage lacks member edge".into(),
                ));
            }
            let member_object = context.objects.get(member_ref, context.budget)?;
            let member: OwnerMemberBinding = serde_json::from_slice(&member_object.payload)
                .map_err(|error| IndexError::CorruptObject(format!("owner member: {error}")))?;
            let expected_input = owner.desired;
            if member.instance != query.instance
                || member.format != query.format
                || member.owner != owner.owner
                || member.input != expected_input
                || !member_object
                    .registration
                    .children
                    .contains(&member.artifact)
            {
                return Err(IndexError::CorruptObject(
                    "owner member binding mismatch".into(),
                ));
            }
            let artifact_object = context.objects.get(member.artifact, context.budget)?;
            if commitment(&artifact_object.payload) != member.logical_commitment {
                return Err(IndexError::CorruptObject(
                    "owner vector commitment mismatch".into(),
                ));
            }
            let artifact: OwnerArtifact = serde_json::from_slice(&artifact_object.payload)
                .map_err(|error| IndexError::CorruptObject(format!("owner vectors: {error}")))?;
            if artifact.owner != owner.owner
                || artifact.input != expected_input
                || artifact.recipe != recipe_fingerprint
                || !artifact_object
                    .registration
                    .children
                    .contains(&artifact.payload_ref)
                || owner
                    .desired
                    .source
                    .is_some_and(|source| !artifact_object.registration.children.contains(&source))
                || owner.desired.projection.is_some_and(|projection| {
                    !artifact_object.registration.children.contains(&projection)
                })
                || contribution.is_some_and(|value| value.output != Some(artifact.payload_ref))
            {
                return Err(IndexError::CorruptObject(
                    "owner artifact binding mismatch".into(),
                ));
            }
            validate_vectors(&artifact.vectors, query.recipe, context.budget)?;
            for vector in artifact.vectors {
                context.budget.charge(WorkUsage {
                    mutations: 1,
                    ..WorkUsage::default()
                })?;
                let cosine = cosine(query.vector, query_norm, &vector.values)?;
                let hit = RankedHit(SearchHit {
                    owner: owner.owner,
                    vector_id: vector.id,
                    cosine,
                });
                let top_k = usize::try_from(query.top_k)
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?;
                if heap.len() < top_k {
                    heap.push(hit);
                } else if heap.peek().is_some_and(|worst| hit < *worst) {
                    drop(heap.pop());
                    heap.push(hit);
                }
            }
            if scanned_owners == query.max_owners
                && (offset.saturating_add(1) < page_len || page.has_more)
            {
                complete = false;
                break;
            }
        }
        if scanned_owners == query.max_owners {
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
        after = Some(last.0.clone());
    }
    let mut ranked = heap.into_vec();
    ranked.sort();
    Ok(SearchResult {
        hits: ranked.into_iter().map(|hit| hit.0).collect(),
        scanned_owners,
        skipped_stale,
        partial: !complete,
        exhaustive: complete,
    })
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
    use std::{
        collections::BTreeMap,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering as AtomicOrdering},
        },
        time::{Duration, Instant},
    };

    use futures::executor::block_on;
    use quanta_index_core::{
        RequestBudgetV1,
        index::{
            AttemptId, AttemptRef, CatalogView, CollectionId, GroupActivation, IndexKind,
            InputBinding, MapBatchRequest, MapBatchWrite, MapEdit, MapPage, MapWrite,
            ObjectBatchInput, OperationKey, ProducerId, PublicationGroupId, RegisteredObject,
            SchemaId, SlotId, SourceCut, StableOwnerId, StoredObject, StreamId, TenantId, TicketId,
            TicketRecord, ViewRef, WorkLimits, WorkspaceKey, WorktreeId,
            ports::{ObjectCustodyPort, ObjectWriteSink},
        },
    };

    use super::*;

    #[derive(Default)]
    struct MemoryStore(Mutex<BTreeMap<ObjectRef, StoredObject>>);

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
            let stored = StoredObject {
                registration: RegisteredObject {
                    reference,
                    bytes: u64::try_from(payload.len())
                        .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                    children,
                },
                payload,
            };
            let mut objects = self
                .0
                .lock()
                .map_err(|_poison_error| IndexError::Storage("test lock".into()))?;
            drop(objects.insert(reference, stored));
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
            Ok(RegisteredObject {
                reference: Self::reference(payload, children),
                bytes: u64::try_from(payload.len())
                    .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                children: children.to_vec(),
            })
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
            Ok(self.get(reference, budget)?.registration)
        }
        fn install(
            &self,
            payload: &[u8],
            children: &[ObjectRef],
            sink: &dyn ObjectWriteSink,
            budget: &mut WorkBudget,
        ) -> Result<RegisteredObject, IndexError> {
            let registration = self.put(payload, children, budget)?;
            sink.reserve(&registration)?;
            sink.installed(&registration)?;
            Ok(registration)
        }
        fn install_batch(
            &self,
            _inputs: &[ObjectBatchInput],
            _sink: &dyn ObjectWriteSink,
            _budget: &mut WorkBudget,
        ) -> Result<Vec<RegisteredObject>, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test object batch write".into(),
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

    #[derive(Default)]
    struct TestMap {
        entries: Vec<(Vec<u8>, ObjectRef)>,
        fail: bool,
    }

    impl PersistentMapPort for TestMap {
        fn apply_stream(
            &self,
            _: &[MapRoot],
            _: &mut dyn quanta_index_core::index::ports::MapEditCursor,
            _: &dyn ObjectWriteSink,
            _: &mut WorkBudget,
        ) -> Result<Vec<MapRoot>, IndexError> {
            Err(IndexError::BackendViolation(
                "unexpected test streamed map write".into(),
            ))
        }
        fn lookup(
            &self,
            root: MapRoot,
            key: &[u8],
            budget: &mut WorkBudget,
        ) -> Result<Option<ObjectRef>, IndexError> {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            if root.0.is_none() {
                return Ok(None);
            }
            Ok(self
                .entries
                .iter()
                .find(|(entry, _)| entry == key)
                .map(|(_, reference)| *reference))
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
                return Ok(MapPage {
                    entries: Vec::new(),
                    has_more: false,
                });
            }
            let filtered: Vec<_> = self
                .entries
                .iter()
                .filter(|(key, _)| after.is_none_or(|after| key.as_slice() > after))
                .cloned()
                .collect();
            let limit =
                usize::try_from(limit).map_err(|_conversion_error| IndexError::ResourceLimited)?;
            Ok(MapPage {
                has_more: filtered.len() > limit,
                entries: filtered.into_iter().take(limit).collect(),
            })
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
        fn embed<'a>(&'a self, input: EmbeddingInput<'a>) -> IndexFuture<'a, Vec<EmbeddingVector>> {
            let _previous = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            if input.request_budget.is_cancelled()
                || input.request_budget.deadline() <= Instant::now()
            {
                return Box::pin(async { Err(IndexError::Cancelled) });
            }
            Box::pin(async { Ok(self.output.clone()) })
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
        fn embed<'a>(
            &'a self,
            _input: EmbeddingInput<'a>,
        ) -> IndexFuture<'a, Vec<EmbeddingVector>> {
            let _previous = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            Box::pin(async {
                Ok(vec![EmbeddingVector {
                    id: b"whole-file".to_vec(),
                    values: vec![1.0, 0.0],
                }])
            })
        }
    }

    #[test]
    fn descriptor_drift_during_embedding_does_not_prepare_an_owner()
    -> Result<(), Box<dyn std::error::Error>> {
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
        let mut work = budget();
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            budget: &mut work,
        };
        let input = OwnerInputBinding {
            source: Some(source),
            projection: None,
            producer_context: Digest([0; 32]),
            derivation_policy: pinned.fingerprint(),
        };
        assert!(matches!(
            block_on(adapter.prepare_owner(&instance(&pinned), owner(1), input, &mut context)),
            Err(IndexError::BackendViolation(_))
        ));
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
        Ok(())
    }

    struct FixedTextProvider {
        calls: Arc<AtomicUsize>,
        deadline: Instant,
        normalization: EmbeddingNormalization,
    }

    impl TextEmbeddingProvider for FixedTextProvider {
        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }

        fn embed_batch_within(
            &self,
            texts: &[&str],
            budget: &RequestBudgetV1,
        ) -> Result<Vec<Vec<f32>>, CoreError> {
            if budget.deadline() != self.deadline || budget.is_cancelled() {
                return Err(CoreError::InvalidContract(
                    "original budget was not forwarded".into(),
                ));
            }
            let _previous = self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            self.embed_batch(texts)
        }

        fn model_id(&self) -> &str {
            "fixed-test-model"
        }
        fn model_revision(&self) -> &str {
            "revision-1"
        }
        fn dimension(&self) -> usize {
            2
        }
        fn normalization(&self) -> EmbeddingNormalization {
            self.normalization
        }
    }

    #[test]
    fn text_bridge_pins_descriptor_and_forwards_original_budget()
    -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(30);
        let request = RequestBudgetV1::until(deadline);
        let calls = Arc::new(AtomicUsize::new(0));
        let bridge = TextEmbeddingBridge::new(FixedTextProvider {
            calls: Arc::clone(&calls),
            deadline,
            normalization: EmbeddingNormalization::L2Unit,
        })?;
        let recipe = bridge.recipe(1024)?;
        assert_eq!(recipe.normalization, VectorNormalization::L2Unit);
        assert_eq!(recipe.max_vectors, 1);
        let projection_recipe = bridge.projection_recipe(
            ProjectionId([2; 16]),
            ProducerId([3; 16]),
            SchemaId([4; 16]),
            1024,
        )?;
        assert_ne!(recipe.fingerprint(), projection_recipe.fingerprint());
        assert_eq!(recipe.model_identity, projection_recipe.model_identity);
        assert_eq!(recipe.output_policy, projection_recipe.output_policy);
        assert_eq!(bridge.embed_query("query", &request)?, vec![1.0, 0.0]);
        let source = SourceRecord {
            path: "a.rs".into(),
            bytes: ObjectRef(Digest([1; 32])),
            byte_len: 6,
        };
        let vectors = block_on(bridge.embed(EmbeddingInput {
            owner: owner(1),
            source: Some(&source),
            source_bytes: b"source",
            recipe: &recipe,
            request_budget: &request,
        }))?;
        assert_eq!(
            vectors,
            vec![EmbeddingVector {
                id: b"whole-file".to_vec(),
                values: vec![1.0, 0.0],
            }]
        );
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 2);
        assert!(matches!(
            block_on(bridge.embed(EmbeddingInput {
                owner: owner(1),
                source: Some(&source),
                source_bytes: &[0xff],
                recipe: &recipe,
                request_budget: &request,
            })),
            Err(IndexError::InvalidInput(_))
        ));
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 2);
        request.cancel_handle().cancel();
        assert!(matches!(
            bridge.embed_query("query", &request),
            Err(IndexError::Cancelled)
        ));
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 2);
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
            output_policy: Digest([8; 32]),
            max_vectors: 4,
            max_source_bytes: 1024,
        }
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

    fn budget() -> WorkBudget {
        WorkBudget::new(
            WorkLimits {
                reads: 1000,
                writes: 1000,
                bytes: 100_000_000,
                mutations: 1000,
            },
            Instant::now() + Duration::from_secs(30),
        )
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
        };
        Ok((
            store.insert(serde_json::to_vec(&record)?, vec![bytes])?,
            bytes,
        ))
    }

    #[test]
    fn wrong_recipe_and_cancelled_request_do_not_call_provider()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = MemoryStore::default();
        let maps = TestMap::default();
        let custody = TestCustody;
        let calls = Arc::new(AtomicUsize::new(0));
        let mut wrong_model = recipe();
        wrong_model.model_identity = Digest([0; 32]);
        assert!(matches!(
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
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
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
        let mut work = budget();
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            budget: &mut work,
        };
        assert!(matches!(
            block_on(adapter.prepare_owner(&spec, owner(1), input, &mut context)),
            Err(IndexError::InvalidInput(_))
        ));
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);

        let request = RequestBudgetV1::until(Instant::now() + Duration::from_secs(30));
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
            budget: &mut cancelled,
        };
        assert!(matches!(
            block_on(adapter.prepare_owner(
                &instance(adapter.recipe()),
                owner(1),
                input,
                &mut context
            )),
            Err(IndexError::Cancelled)
        ));
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn malformed_provider_result_cannot_seal() -> Result<(), Box<dyn std::error::Error>> {
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
        let mut work = budget();
        let mut context = AttemptContext {
            attempt: attempt(),
            objects: &store,
            maps: &maps,
            custody: &custody,
            budget: &mut work,
        };
        assert!(matches!(
            block_on(adapter.prepare_owner(&instance(&recipe), owner(1), input, &mut context)),
            Err(IndexError::BackendViolation(_))
        ));
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
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
                vectors: vec![EmbeddingVector {
                    id: vec![id],
                    values: values.to_vec(),
                }],
            };
            let artifact_payload = serde_json::to_vec(&artifact)?;
            let artifact_ref =
                store.insert(artifact_payload.clone(), vec![source, artifact.payload_ref])?;
            let member = OwnerMemberBinding {
                instance: instance.instance,
                owner: owner(id),
                input,
                format: instance.format,
                artifact: artifact_ref,
                logical_commitment: commitment(&artifact_payload),
            };
            let member_ref = store.insert(serde_json::to_vec(&member)?, vec![artifact_ref])?;
            let coverage = OwnerCoverage {
                owner: owner(id),
                desired: input,
                covered: Some(input),
                searchable: Some(member_ref),
            };
            let coverage_ref = store.insert(serde_json::to_vec(&coverage)?, vec![member_ref])?;
            entries.push((owner_map_key(owner(id)), coverage_ref));
        }
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let maps = TestMap {
            entries,
            fail: false,
        };
        let coverage = CoverageBinding {
            instance: instance.instance,
            desired: quanta_index_core::index::InputBinding {
                root: MapRoot(Some(source)),
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
        let mut work = budget();
        let mut read = ReadContext {
            objects: &store,
            maps: &maps,
            budget: &mut work,
        };
        let result = search_exact(
            &SearchQuery {
                coverage: &coverage,
                projection: None,
                source_owners: MapRoot(None),
                instance: instance.instance,
                format: instance.format,
                recipe: &recipe,
                vector: &[1.0, 0.0],
                top_k: 2,
                page_size: 2,
                max_owners: 10,
            },
            &mut read,
        )?;
        assert_eq!(
            result
                .hits
                .iter()
                .map(|hit| hit.vector_id.clone())
                .collect::<Vec<_>>(),
            vec![vec![1], vec![2]]
        );
        assert!(!result.exhaustive);
        assert!(result.partial);
        let mut fully_covered = coverage.clone();
        fully_covered.covered = Some(fully_covered.desired);
        fully_covered.status = CoverageStatus::Complete;
        let complete = search_exact(
            &SearchQuery {
                coverage: &fully_covered,
                projection: None,
                source_owners: MapRoot(None),
                instance: instance.instance,
                format: instance.format,
                recipe: &recipe,
                vector: &[1.0, 0.0],
                top_k: 2,
                page_size: 2,
                max_owners: 10,
            },
            &mut read,
        )?;
        assert!(complete.exhaustive);
        assert!(!complete.partial);
        let truncated = search_exact(
            &SearchQuery {
                coverage: &coverage,
                projection: None,
                source_owners: MapRoot(None),
                instance: instance.instance,
                format: instance.format,
                recipe: &recipe,
                vector: &[1.0, 0.0],
                top_k: 2,
                page_size: 2,
                max_owners: 1,
            },
            &mut read,
        )?;
        assert_eq!(truncated.scanned_owners, 1);
        assert!(!truncated.exhaustive);
        assert!(truncated.partial);
        let mut stale_entries = maps.entries.clone();
        let stale_owner = OwnerCoverage {
            owner: owner(3),
            desired: input,
            covered: None,
            searchable: None,
        };
        let stale_ref = store.insert(serde_json::to_vec(&stale_owner)?, vec![])?;
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
            fail: false,
        };
        let mut read = ReadContext {
            objects: &store,
            maps: &stale_map,
            budget: &mut work,
        };
        let stale = search_exact(
            &SearchQuery {
                coverage: &fully_covered,
                projection: None,
                source_owners: MapRoot(None),
                instance: instance.instance,
                format: instance.format,
                recipe: &recipe,
                vector: &[1.0, 0.0],
                top_k: 2,
                page_size: 2,
                max_owners: 10,
            },
            &mut read,
        )?;
        assert_eq!(stale.skipped_stale, 1);
        assert!(!stale.exhaustive);
        assert!(stale.partial);
        let failing = TestMap {
            entries: vec![],
            fail: true,
        };
        let mut read = ReadContext {
            objects: &store,
            maps: &failing,
            budget: &mut work,
        };
        assert!(matches!(
            search_exact(
                &SearchQuery {
                    coverage: &coverage,
                    projection: None,
                    source_owners: MapRoot(None),
                    instance: instance.instance,
                    format: instance.format,
                    recipe: &recipe,
                    vector: &[1.0, 0.0],
                    top_k: 2,
                    page_size: 2,
                    max_owners: 10
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
    }

    impl CatalogReadPort for StaticCatalog {
        fn acquire_ticket(&self, _selection: ViewSelection) -> Result<TicketRecord, IndexError> {
            Ok(self.ticket.clone())
        }

        fn release_ticket(&self, id: TicketId) -> Result<(), IndexError> {
            if id != self.ticket.id {
                return Err(IndexError::BackendViolation("wrong test ticket".into()));
            }
            let _previous = self.releases.fetch_add(1, AtomicOrdering::SeqCst);
            Ok(())
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
                id: TicketId([5; 16]),
                view: view.clone(),
            },
            releases: Arc::clone(&releases),
        };
        let mut work = budget();
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
        let result = search_exact_acquired(&catalog, query(), &mut context)?;
        assert!(result.hits.is_empty());
        assert!(result.exhaustive);
        assert_eq!(releases.load(AtomicOrdering::SeqCst), 1);
        let mut bad_view = CatalogView {
            coverage: MapRoot(None),
            projections: MapRoot(None),
            ..view
        };
        bad_view.identity.digest = bad_view.identity_digest()?;
        let bad_catalog = StaticCatalog {
            ticket: TicketRecord {
                id: TicketId([6; 16]),
                view: bad_view,
            },
            releases: Arc::clone(&releases),
        };
        assert!(matches!(
            search_exact_acquired(&bad_catalog, query(), &mut context),
            Err(IndexError::BackendViolation(_))
        ));
        let bad_selection = ViewSelection::Exact(bad_catalog.ticket.view.identity);
        assert!(matches!(
            search_exact_acquired(
                &bad_catalog,
                TicketSearchQuery {
                    selection: bad_selection,
                    ..query()
                },
                &mut context,
            ),
            Err(IndexError::SnapshotUnavailable)
        ));
        assert_eq!(releases.load(AtomicOrdering::SeqCst), 3);
        Ok(())
    }
}
