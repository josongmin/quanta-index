//! Provider-independent embedding recipe identity and admission limits.

use quanta_index_contract::EmbeddingNormalization;
use quanta_index_core::index::{Digest, IndexError, ProducerId, ProjectionId, SchemaId};
use sha2::{Digest as _, Sha256};

use super::chunks::Utf8ChunkPolicy;

// Bind the mandatory persisted reuse-identity field to profile admission.
// Older recipe policies require an explicit rebuild, never a legacy reader.
const RECIPE_DOMAIN: &[u8] = b"quanta-index-native-semantic-recipe-v4\0";
const MODEL_DOMAIN: &[u8] = b"quanta-index-native-semantic-text-model-v1\0";
// One vector from the complete selected UTF-8 payload. The domain stays
// stable for SourceUtf8; input selection is separately pinned by the recipe.
const WHOLE_FILE_POLICY: &[u8] = b"quanta-index-native-semantic-utf8-whole-file-one-vector-v1\0";
const CERTIFIED_CHUNKS_POLICY: &[u8] =
    b"quanta-index-native-semantic-certified-rendered-members-v1\0";

pub(super) fn certified_chunk_output_policy() -> Digest {
    Digest(Sha256::digest(CERTIFIED_CHUNKS_POLICY).into())
}

pub(super) const MAX_DIMENSION: u32 = 4096;
pub(super) const MAX_VECTORS: u32 = 4096;
pub(super) const MAX_CELLS: u64 = 200_000;
pub(super) const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;

pub(super) fn whole_file_output_policy() -> Digest {
    Digest(Sha256::digest(WHOLE_FILE_POLICY).into())
}

pub(super) fn text_model_identity(
    model_id: &str,
    model_revision: &str,
) -> Result<Digest, IndexError> {
    if model_id.is_empty()
        || model_revision.is_empty()
        || model_id.len() > 1024
        || model_revision.len() > 1024
    {
        return Err(IndexError::InvalidInput(
            "text provider lacks a bounded model identity and revision".into(),
        ));
    }
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
    Ok(Digest(hash.finalize().into()))
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

impl From<EmbeddingNormalization> for VectorNormalization {
    fn from(normalization: EmbeddingNormalization) -> Self {
        match normalization {
            EmbeddingNormalization::None => Self::CosineRaw,
            EmbeddingNormalization::L2Unit => Self::L2Unit,
        }
    }
}

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
    /// Compose the existing text recipe from pinned, explicit model metadata.
    /// This performs no provider, transport, credential or environment access.
    #[expect(
        clippy::too_many_arguments,
        reason = "the existing recipe's independent identity and admission inputs are explicit"
    )]
    pub fn from_text_model(
        model_id: &str,
        model_revision: &str,
        dimension: u32,
        normalization: VectorNormalization,
        input_policy: EmbeddingInputPolicy,
        chunk_policy: Option<Utf8ChunkPolicy>,
        max_vectors: u32,
        max_source_bytes: u64,
    ) -> Result<Self, IndexError> {
        let model_identity = text_model_identity(model_id, model_revision)?;
        let output_policy = match input_policy {
            EmbeddingInputPolicy::ProjectionChunks { .. } => certified_chunk_output_policy(),
            EmbeddingInputPolicy::SourceUtf8 | EmbeddingInputPolicy::ProjectionUtf8 { .. } => {
                chunk_policy.map_or_else(whole_file_output_policy, Utf8ChunkPolicy::output_policy)
            }
        };
        let recipe = Self {
            input_policy,
            model_identity,
            dimension,
            normalization,
            output_policy,
            chunk_policy,
            max_vectors,
            max_source_bytes,
        };
        recipe.validate()?;
        Ok(recipe)
    }

    pub fn validate(&self) -> Result<(), IndexError> {
        if self.is_certified_chunks()
            && (self.chunk_policy.is_some()
                || self.output_policy != certified_chunk_output_policy())
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
